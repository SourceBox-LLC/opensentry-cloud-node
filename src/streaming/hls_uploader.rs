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
//! Watches HLS output directory and pushes new segments to the backend.
//! Maintains a rolling buffer locally while streaming to cloud.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::sync::Semaphore;

use crate::api::ApiClient;
use crate::config::MotionConfig;
use crate::dashboard::{CameraStatus, Dashboard};
use crate::error::Result;
use crate::storage::NodeDatabase;
use super::motion_detector;
use super::segment_uploader::{SegmentUploader, UploadTask, UploaderConfig};

/// Motion event emitted when scene change exceeds the configured threshold.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MotionEvent {
    pub camera_id: String,
    pub score: u32,       // 0-100 (normalised from 0.0-1.0)
    pub timestamp: String, // ISO 8601
    pub segment_seq: u64,
}

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
    /// Number of segments to keep locally after upload
    pub local_buffer_size: u32,
}

impl HlsUploaderConfig {
    pub fn new(camera_id: String, output_dir: PathBuf) -> Self {
        Self {
            camera_id,
            output_dir,
            retry_count: 3,
            local_buffer_size: 5, // Keep 5 segments locally (~5 seconds with 1s segments)
        }
    }
}

/// HLS Segment Uploader
pub struct HlsUploader {
    config: HlsUploaderConfig,
    api_client: ApiClient,
    /// Track sequence number for ordering
    last_uploaded_seq: Arc<std::sync::atomic::AtomicU64>,
    /// Track whether codec has been detected
    codec_detected: Arc<std::sync::atomic::AtomicBool>,
    /// Which camera IDs are currently recording (shared with WS command handler)
    recording_state: Arc<RwLock<HashSet<String>>>,
    /// SQLite database for storing recorded segments
    db: NodeDatabase,
    /// Motion detection configuration
    motion_config: MotionConfig,
    /// Channel to send motion events to the WebSocket client
    motion_tx: tokio::sync::mpsc::Sender<MotionEvent>,
}

impl HlsUploader {
    /// Create a new HLS uploader
    pub fn new(
        config: HlsUploaderConfig,
        api_client: ApiClient,
        recording_state: Arc<RwLock<HashSet<String>>>,
        db: NodeDatabase,
        motion_config: MotionConfig,
        motion_tx: tokio::sync::mpsc::Sender<MotionEvent>,
    ) -> Self {
        Self {
            config,
            api_client,
            last_uploaded_seq: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            codec_detected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            recording_state,
            db,
            motion_config,
            motion_tx,
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
        // Per-camera cooldown — this uploader is single-camera, so one Instant suffices
        let last_motion: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

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

                // Clone everything needed for the background task
                let uploader_config = UploaderConfig {
                    retry_count: self.config.retry_count,
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
                let motion_cfg = self.motion_config.clone();
                let motion_tx = self.motion_tx.clone();
                let last_motion = last_motion.clone();

                // Spawn upload as a concurrent task so it doesn't block
                // the next segment. This prevents one slow upload from
                // stalling the entire pipeline. The semaphore limits
                // concurrent uploads to avoid unbounded task growth.
                let sem = UPLOAD_SEMAPHORE.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    let uploader = SegmentUploader::new(uploader_config);
                    let file_size = tokio::fs::metadata(&segment_path).await.map(|m| m.len()).unwrap_or(0);

                    let task = UploadTask {
                        camera_id: camera_id.clone(),
                        segment_path: segment_path.clone(),
                        sequence: seq,
                    };

                    match uploader.push_segment(task, &api_client).await {
                        Ok(true) => {
                            let kb = file_size / 1024;
                            dash.record_upload(&camera_name, file_size);
                            dash.update_camera_status(&camera_name, CameraStatus::Streaming);
                            dash.log_debug(format!("Segment {:05} pushed ({} KB)", seq, kb));

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

                            // Motion detection (non-blocking, with per-camera cooldown)
                            if motion_cfg.enabled {
                                spawn_motion_detection(
                                    segment_path.clone(),
                                    camera_id.clone(),
                                    seq,
                                    &motion_cfg,
                                    motion_tx.clone(),
                                    last_motion.clone(),
                                );
                            }

                            // Playlist push (background, non-blocking)
                            let api = api_client.clone();
                            let cam = cam_id_for_playlist;
                            let dir = output_dir;
                            tokio::spawn(async move {
                                let playlist_path = dir.join("stream.m3u8");
                                if let Ok(content) = tokio::fs::read_to_string(&playlist_path).await {
                                    if content.starts_with("#EXTM3U") && content.contains("#EXTINF") {
                                        if let Err(e) = api.update_playlist(&cam, &content).await {
                                            tracing::warn!("Playlist push failed: {}", e);
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
                            dash.log_warn(format!("Segment {} push failed: {}", seq, e));
                        }
                    }
                });
            }

            tokio::time::sleep(poll_interval).await;
        }
    }
}

/// Spawn a background task that runs FFmpeg scene-change detection on a
/// segment and, if motion exceeds the threshold and the per-camera cooldown
/// has elapsed, sends the event to the WebSocket client.
fn spawn_motion_detection(
    segment_path: PathBuf,
    camera_id: String,
    seq: u64,
    motion_cfg: &MotionConfig,
    tx: tokio::sync::mpsc::Sender<MotionEvent>,
    last_motion: Arc<Mutex<Option<Instant>>>,
) {
    let threshold = motion_cfg.threshold;
    let cooldown = std::time::Duration::from_secs(motion_cfg.cooldown_secs);

    tokio::spawn(async move {
        if let Some(score) = motion_detector::detect_motion(&segment_path, threshold).await {
            // Check cooldown before sending
            let now = Instant::now();
            let mut guard = last_motion.lock().unwrap();
            if let Some(last) = *guard {
                if now.duration_since(last) < cooldown {
                    return;
                }
            }
            *guard = Some(now);
            drop(guard);

            let event = MotionEvent {
                camera_id,
                score: (score * 100.0).round() as u32,
                timestamp: chrono::Utc::now().to_rfc3339(),
                segment_seq: seq,
            };
            if let Err(e) = tx.try_send(event) {
                tracing::debug!("Motion channel full, dropping event: {}", e);
            }
        }
    });
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
        assert_eq!(config.local_buffer_size, 5);
        assert_eq!(config.retry_count, 3);
    }
}
