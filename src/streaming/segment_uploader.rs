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
//! Segment Uploader - Uploads HLS segments to cloud storage
//!
//! Manages upload queue and coordinates with the API to get upload URLs.
//! Uses a pooled HTTP client for connection reuse and retries transient errors.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::Result;

/// Reusable HTTP client for uploading segments to presigned URLs.
/// Shared across all uploads to leverage TCP connection pooling,
/// avoiding the overhead of a new TLS handshake per segment.
static UPLOAD_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .pool_max_idle_per_host(4)
        .pool_idle_timeout(Duration::from_secs(60))
        .tcp_keepalive(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create upload HTTP client")
});

/// Upload task
pub struct UploadTask {
    /// Camera ID
    pub camera_id: String,
    /// Segment file path
    pub segment_path: PathBuf,
    /// Segment sequence number
    pub sequence: u64,
}

/// Segment uploader configuration
#[derive(Debug, Clone)]
pub struct UploaderConfig {
    /// Upload retry count
    pub retry_count: u32,
    /// Upload timeout in seconds
    pub timeout_seconds: u32,
}

impl Default for UploaderConfig {
    fn default() -> Self {
        Self {
            retry_count: 3,
            timeout_seconds: 30,
        }
    }
}

/// Segment uploader
pub struct SegmentUploader {
    config: UploaderConfig,
}

impl SegmentUploader {
    /// Create a new segment uploader
    pub fn new(config: UploaderConfig) -> Self {
        Self { config }
    }

    /// Upload a segment directly to a presigned URL (batch flow).
    /// No per-segment backend call — the URL was pre-fetched in a batch.
    /// Returns true if uploaded, false if skipped (too small).
    pub async fn upload_segment_direct(&self, task: UploadTask, presigned_url: &str) -> Result<bool> {
        tracing::debug!(
            "Uploading segment {} for camera {} (direct)",
            task.sequence,
            task.camera_id
        );

        let data = tokio::fs::read(&task.segment_path).await?;

        if data.len() < 188 {
            tracing::warn!(
                "Segment {} too small ({} bytes), skipping",
                task.sequence,
                data.len()
            );
            return Ok(false);
        }

        tracing::debug!("Segment {} size: {} bytes", task.sequence, data.len());

        let data_bytes: bytes::Bytes = data.into();
        let mut attempts = 0;
        let max_attempts = self.config.retry_count;

        loop {
            match self.upload_to_storage(presigned_url, &data_bytes).await {
                Ok(()) => break,
                Err(e) if Self::is_retryable(&e) && attempts < max_attempts => {
                    attempts += 1;
                    let delay_ms = 50 * (1u64 << attempts.min(2)); // 100, 200, 200ms
                    tracing::warn!(
                        "Upload attempt {}/{} failed ({}), retrying in {}ms...",
                        attempts, max_attempts, e, delay_ms,
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                Err(e) => return Err(e),
            }
        }

        tracing::info!(
            "Uploaded segment {} for camera {}",
            task.sequence,
            task.camera_id
        );

        Ok(true)
    }

    /// Check if an error is retryable (transient network/server error).
    /// Uses prefix matching on "Upload failed: NNN" to avoid false positives
    /// from Tigris XML bodies that contain digits in RequestId fields.
    fn is_retryable(err: &crate::error::Error) -> bool {
        match err {
            crate::error::Error::Streaming(msg) => {
                // Extract the HTTP status code from the beginning of the error.
                // Format: "Upload failed: 502 Bad Gateway" or similar.
                let retryable_prefixes = [
                    "Upload failed: 408",
                    "Upload failed: 411",
                    "Upload failed: 429",
                    "Upload failed: 500",
                    "Upload failed: 502",
                    "Upload failed: 503",
                    "Upload failed: 504",
                ];
                retryable_prefixes.iter().any(|p| msg.starts_with(p))
            }
            crate::error::Error::HttpClient(_) => true, // reqwest errors (timeouts, connection resets)
            _ => false,
        }
    }

    /// Upload to signed URL using pooled HTTP client.
    /// Bytes::clone() is a cheap Arc ref-count bump, not a data copy.
    async fn upload_to_storage(
        &self,
        url: &str,
        data: &bytes::Bytes,
    ) -> Result<()> {
        let response = UPLOAD_CLIENT
            .put(url)
            .header("Content-Type", "video/mp2t")
            .header("Content-Length", data.len())
            .body(data.clone()) // Bytes::clone is O(1) - just bumps Arc refcount
            .timeout(Duration::from_secs(self.config.timeout_seconds.min(10) as u64))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(crate::error::Error::Streaming(format!(
                "Upload failed: {}",
                response.status()
            )));
        }

        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uploader_config_default() {
        let config = UploaderConfig::default();
        assert_eq!(config.retry_count, 3);
        assert_eq!(config.timeout_seconds, 30);
    }

    #[test]
    fn test_upload_task_fields() {
        let task = UploadTask {
            camera_id: "camera_123".to_string(),
            segment_path: PathBuf::from("/data/hls/camera_123/segment_00042.ts"),
            sequence: 42,
        };
        assert_eq!(task.camera_id, "camera_123");
        assert_eq!(task.sequence, 42);
    }
}
