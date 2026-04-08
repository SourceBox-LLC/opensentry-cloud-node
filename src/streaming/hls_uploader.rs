// OpenSentry CloudNode - Camera streaming node for OpenSentry Cloud
// Copyright (C) 2026  SourceBox LLC
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.
//! HLS Segment Uploader
//!
//! Watches HLS output directory and uploads new segments to cloud storage.
//! Maintains a rolling buffer locally while streaming to cloud.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::sync::{Mutex, Semaphore};

use crate::api::ApiClient;
use crate::dashboard::{CameraStatus, Dashboard};
use crate::error::Result;
use crate::storage::NodeDatabase;
use super::segment_uploader::{SegmentUploader, UploadTask, UploaderConfig};

/// Maximum concurrent segment uploads. Prevents unbounded task spawning
/// if uploads are slower than segment production (e.g., slow network).
/// 4 concurrent uploads is enough for 2s segments — even if each upload
/// takes 8 seconds, we still keep up.
static UPLOAD_SEMAPHORE: std::sync::LazyLock<Arc<Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(Semaphore::new(4)));

/// HLS Uploader configuration
#[derive(Debug, Clone)]
pub struct HlsUploaderConfig {
    /// Camera ID for this stream
    pub camera_id: String,
    /// Directory containing HLS files
    pub output_dir: PathBuf,
    /// Upload retry count
    pub retry_count: u32,
    /// Upload timeout in seconds
    pub timeout_seconds: u32,
    /// Number of segments to keep locally after upload
    pub local_buffer_size: u32,
}

impl HlsUploaderConfig {
    pub fn new(camera_id: String, output_dir: PathBuf) -> Self {
        Self {
            camera_id,
            output_dir,
            retry_count: 3,
            timeout_seconds: 30,
            local_buffer_size: 5, // Keep 5 segments locally (~5 seconds with 1s segments)
        }
    }
}

/// Pre-fetched batch of presigned URLs for segment uploads.
/// Eliminates per-segment backend round-trips — the #1 cause of the buffer wall.
struct UrlPool {
    /// Map of sequence number → presigned upload URL
    urls: HashMap<u64, String>,
    /// Next sequence to request when refilling
    next_start_seq: u64,
}

/// HLS Segment Uploader
pub struct HlsUploader {
    config: HlsUploaderConfig,
    api_client: ApiClient,
    /// Track sequence number for ordering
    last_uploaded_seq: Arc<std::sync::atomic::AtomicU64>,
    /// Track whether codec has been detected
    codec_detected: Arc<std::sync::atomic::AtomicBool>,
    /// Pre-fetched presigned URLs — shared across async tasks
    url_pool: Arc<Mutex<Option<UrlPool>>>,
    /// Which camera IDs are currently recording (shared with WS command handler)
    recording_state: Arc<RwLock<HashSet<String>>>,
    /// SQLite database for storing recorded segments
    db: NodeDatabase,
}

/// How many URLs to request per batch
const BATCH_SIZE: u32 = 30;
/// Refill when fewer than this many URLs remain
const REFILL_THRESHOLD: usize = 10;

impl HlsUploader {
    /// Create a new HLS uploader
    pub fn new(
        config: HlsUploaderConfig,
        api_client: ApiClient,
        recording_state: Arc<RwLock<HashSet<String>>>,
        db: NodeDatabase,
    ) -> Self {
        Self {
            config,
            api_client,
            last_uploaded_seq: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            codec_detected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            url_pool: Arc::new(Mutex::new(None)),
            recording_state,
            db,
        }
    }

    /// Fetch a batch of presigned URLs from the backend.
    /// Called once on stream start, then again when running low.
    async fn refill_url_pool(&self, start_seq: u64) -> Result<()> {
        let batch = self.api_client
            .get_batch_upload_urls(&self.config.camera_id, start_seq, BATCH_SIZE)
            .await?;

        let mut urls = HashMap::new();
        let mut max_seq = start_seq;
        for entry in &batch.urls {
            urls.insert(entry.sequence, entry.upload_url.clone());
            if entry.sequence >= max_seq {
                max_seq = entry.sequence + 1;
            }
        }

        let mut pool = self.url_pool.lock().await;
        match pool.as_mut() {
            Some(existing) => {
                // Merge new URLs into existing pool (don't discard unused ones)
                existing.urls.extend(urls);
                existing.next_start_seq = max_seq;
            }
            None => {
                *pool = Some(UrlPool {
                    urls,
                    next_start_seq: max_seq,
                });
            }
        }

        tracing::info!(
            "URL pool refilled: {} URLs starting at seq {} for camera {}",
            batch.urls.len(), start_seq, self.config.camera_id
        );

        Ok(())
    }

    /// Get the presigned URL for a given sequence number.
    /// Returns None if the sequence isn't in the pool (triggers refill).
    async fn take_url(&self, sequence: u64) -> Option<String> {
        let mut pool = self.url_pool.lock().await;
        pool.as_mut().and_then(|p| p.urls.remove(&sequence))
    }

    /// Check if the pool needs refilling
    async fn pool_needs_refill(&self) -> (bool, u64) {
        let pool = self.url_pool.lock().await;
        match pool.as_ref() {
            Some(p) => (p.urls.len() < REFILL_THRESHOLD, p.next_start_seq),
            None => (true, 0),
        }
    }

    /// Start with dashboard reporting (used by node runner).
    ///
    /// Uses polling instead of a file-system watcher. The `notify` crate's
    /// `ReadDirectoryChangesW` backend on Windows silently stops delivering
    /// events after ~95 segments, even though FFmpeg keeps producing them.
    /// Polling every second is simple, reliable, and adds at most 1s latency.
    pub async fn start_with_dashboard(
        self,
        dash: Dashboard,
        camera_name: String,
        _camera_id: String,
    ) -> Result<()> {
        let poll_interval = tokio::time::Duration::from_secs(1);
        let mut seen: HashSet<String> = HashSet::new();
        let mut stale_cycles: u32 = 0;

        loop {
            // Scan the output directory for new .ts segments
            let mut new_segments: Vec<(u64, PathBuf)> = Vec::new();

            match std::fs::read_dir(&self.config.output_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let name = entry.file_name().to_string_lossy().to_string();

                        if !name.ends_with(".ts") || seen.contains(&name) {
                            continue;
                        }

                        // Only enqueue if the file is large enough (FFmpeg finished writing)
                        if let Ok(meta) = std::fs::metadata(&path) {
                            if meta.len() >= 188 {
                                if let Some(seq) = extract_sequence_number(&name) {
                                    seen.insert(name);
                                    new_segments.push((seq, path));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read HLS directory: {}", e);
                }
            }

            // Process new segments in sequence order
            new_segments.sort_by_key(|(seq, _)| *seq);

            // Prune the seen set to prevent unbounded growth.
            // Keep only recent entries — old segments have been deleted from disk
            // and their names will never appear again.
            if seen.len() > 200 {
                let last_seq = self.last_uploaded_seq.load(std::sync::atomic::Ordering::SeqCst);
                let cutoff = last_seq.saturating_sub(50);
                seen.retain(|name| {
                    extract_sequence_number(name).map_or(true, |seq| seq >= cutoff)
                });
            }

            if new_segments.is_empty() {
                stale_cycles += 1;
                // Warn after 10 seconds of no new segments
                if stale_cycles == 10 {
                    dash.log_warn("No new segment in 10s — camera may have disconnected");
                    tracing::warn!(
                        "No new segments for camera {} in 10s",
                        self.config.camera_id
                    );
                    dash.update_camera_status(&camera_name, CameraStatus::Error(
                        "No segments (camera disconnected?)".into()
                    ));
                }
            } else {
                if stale_cycles >= 10 {
                    // We were stale but segments resumed — camera reconnected
                    dash.log_info("Segments resumed");
                    dash.update_camera_status(&camera_name, CameraStatus::Streaming);
                }
                stale_cycles = 0;
            }

            for (seq, segment_path) in &new_segments {
                let seq = *seq;
                let segment_path = segment_path.clone();

                // Ensure URL pool is filled before spawning
                self.ensure_url_pool(seq).await;

                // Get presigned URL now (needs &self which isn't Send)
                let presigned_url = match self.take_url(seq).await {
                    Some(url) => url,
                    None => {
                        tracing::warn!("URL pool empty for seq {}, forcing refill", seq);
                        if let Err(e) = self.refill_url_pool(seq).await {
                            dash.log_warn(format!("Refill failed for seq {}: {}", seq, e));
                            continue;
                        }
                        match self.take_url(seq).await {
                            Some(url) => url,
                            None => {
                                dash.log_warn(format!("No URL for seq {} after refill", seq));
                                continue;
                            }
                        }
                    }
                };

                // Clone everything needed for the background task
                let uploader_config = UploaderConfig {
                    retry_count: self.config.retry_count,
                    timeout_seconds: self.config.timeout_seconds,
                };
                let camera_id = self.config.camera_id.clone();
                let last_uploaded = self.last_uploaded_seq.clone();
                let codec_detected = self.codec_detected.clone();
                let api_client = self.api_client.clone();
                let cam_id_for_playlist = self.config.camera_id.clone();
                let output_dir = self.config.output_dir.clone();
                let dash = dash.clone();
                let camera_name = camera_name.to_string();
                let local_buffer_size = self.config.local_buffer_size;
                let hls_output_dir = self.config.output_dir.clone();
                let rec_state = self.recording_state.clone();
                let db = self.db.clone();

                // Spawn upload as a concurrent task so it doesn't block
                // the next segment. This prevents one slow upload from
                // stalling the entire pipeline. The semaphore limits
                // concurrent uploads to avoid unbounded task growth.
                let sem = UPLOAD_SEMAPHORE.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    let bg_uploader = SegmentUploader::new(uploader_config);
                    let file_size = tokio::fs::metadata(&segment_path).await.map(|m| m.len()).unwrap_or(0);

                    let task = UploadTask {
                        camera_id: camera_id.clone(),
                        segment_path: segment_path.clone(),
                        sequence: seq,
                    };

                    match bg_uploader.upload_segment_direct(task, &presigned_url).await {
                        Ok(true) => {
                            let kb = file_size / 1024;
                            dash.record_upload(&camera_name, file_size);
                            dash.update_camera_status(&camera_name, CameraStatus::Streaming);
                            dash.log_debug(format!("Segment {:05} uploaded ({} KB)", seq, kb));

                            // Codec detection (only first successful segment)
                            if !codec_detected.load(std::sync::atomic::Ordering::SeqCst) {
                                if let Ok(info) = super::codec_detector::detect_codec(&segment_path) {
                                    dash.set_codec(&camera_name, &info.video_codec, &info.audio_codec);
                                    if let Ok(_) = api_client.report_codec(&camera_id, &info.video_codec, &info.audio_codec).await {
                                        codec_detected.store(true, std::sync::atomic::Ordering::SeqCst);
                                        dash.log_info("Codec reported to cloud");
                                    }
                                }
                            }

                            // Playlist upload (background, non-blocking)
                            let api = api_client.clone();
                            let cam = cam_id_for_playlist;
                            let dir = output_dir;
                            tokio::spawn(async move {
                                let playlist_path = dir.join("stream.m3u8");
                                if let Ok(content) = tokio::fs::read_to_string(&playlist_path).await {
                                    if content.starts_with("#EXTM3U") && content.contains("#EXTINF") {
                                        if let Err(e) = api.update_playlist(&cam, &content).await {
                                            tracing::warn!("Playlist upload failed: {}", e);
                                        }
                                    }
                                }
                            });

                            last_uploaded.store(seq, std::sync::atomic::Ordering::SeqCst);

                            // Local cleanup: save to DB if recording, otherwise just delete
                            let buffer_size = local_buffer_size as u64;
                            if seq > buffer_size {
                                let oldest_to_keep = seq.saturating_sub(buffer_size);
                                let scan_start = oldest_to_keep.saturating_sub(20);
                                let is_recording = rec_state.read()
                                    .map(|s| s.contains(&camera_id))
                                    .unwrap_or(false);

                                let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                                for s in scan_start..oldest_to_keep {
                                    let filename = format!("segment_{:05}.ts", s);
                                    let src = hls_output_dir.join(&filename);
                                    if is_recording {
                                        if let Ok(data) = tokio::fs::read(&src).await {
                                            let _ = db.save_recording_segment(
                                                &camera_id, s, &today, &data,
                                            );
                                        }
                                    }
                                    let _ = tokio::fs::remove_file(&src).await;
                                }
                            }
                        }
                        Ok(false) => {
                            tracing::debug!("Skipped segment {} (too small)", seq);
                        }
                        Err(e) => {
                            dash.log_warn(format!("Segment {} upload failed: {}", seq, e));
                        }
                    }
                });
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Ensure the URL pool has enough presigned URLs for this sequence.
    /// Fetches a new batch if pool is empty or running low.
    async fn ensure_url_pool(&self, current_seq: u64) {
        let (needs_refill, next_seq) = self.pool_needs_refill().await;
        if needs_refill {
            let start = if next_seq > 0 { next_seq } else { current_seq };
            if let Err(e) = self.refill_url_pool(start).await {
                tracing::warn!("Batch URL fetch failed: {}", e);
            }
        }
    }

}

/// Extract sequence number from segment filename
fn extract_sequence_number(filename: &str) -> Option<u64> {
    // Format: segment_00001.ts
    let parts: Vec<&str> = filename.split('_').collect();
    if parts.len() != 2 {
        return None;
    }
    
    let num_part = parts[1].trim_end_matches(".ts");
    num_part.parse().ok()
}

// File watcher (notify crate) was removed because ReadDirectoryChangesW
// on Windows silently stops delivering events after ~95 segments.
// Replaced by simple 1-second polling in the upload loop above.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sequence_number() {
        assert_eq!(extract_sequence_number("segment_00001.ts"), Some(1));
        assert_eq!(extract_sequence_number("segment_00042.ts"), Some(42));
        assert_eq!(extract_sequence_number("segment_12345.ts"), Some(12345));
        assert_eq!(extract_sequence_number("invalid.ts"), None);
        assert_eq!(extract_sequence_number("segment_.ts"), None);
        assert_eq!(extract_sequence_number("playlist.m3u8"), None);
    }

    #[test]
    fn test_hls_uploader_config() {
        let config = HlsUploaderConfig::new("camera_123".into(), PathBuf::from("/data/hls/camera_123"));
        assert_eq!(config.camera_id, "camera_123");
        assert_eq!(config.local_buffer_size, 3); // Default is 3 segments (~6 seconds with 2s segments)
        assert_eq!(config.retry_count, 3);
    }
}