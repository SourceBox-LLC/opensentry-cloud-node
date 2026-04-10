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
//! Segment Uploader - Pushes HLS segments to the backend
//!
//! Reads segment files from disk and pushes them to the Command Center
//! via POST /push-segment. Retries transient errors with exponential backoff.

use std::path::PathBuf;
use std::time::Duration;

use crate::api::ApiClient;
use crate::error::Result;

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
}

impl Default for UploaderConfig {
    fn default() -> Self {
        Self {
            retry_count: 3,
        }
    }
}

/// Segment uploader — pushes segments to the backend's in-memory cache.
pub struct SegmentUploader {
    config: UploaderConfig,
}

impl SegmentUploader {
    /// Create a new segment uploader
    pub fn new(config: UploaderConfig) -> Self {
        Self { config }
    }

    /// Push a segment to the backend. Returns true if pushed, false if skipped (too small).
    pub async fn push_segment(&self, task: UploadTask, api_client: &ApiClient) -> Result<bool> {
        tracing::debug!(
            "Pushing segment {} for camera {}",
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

        let filename = format!("segment_{:05}.ts", task.sequence);
        let data_bytes: bytes::Bytes = data.into();
        let mut attempts = 0;
        let max_attempts = self.config.retry_count;

        loop {
            match api_client.push_segment(&task.camera_id, &filename, data_bytes.clone()).await {
                Ok(()) => break,
                Err(e) if Self::is_retryable(&e) && attempts < max_attempts => {
                    attempts += 1;
                    let delay_ms = 50 * (1u64 << attempts.min(2)); // 100, 200, 200ms
                    tracing::warn!(
                        "Push attempt {}/{} failed ({}), retrying in {}ms...",
                        attempts, max_attempts, e, delay_ms,
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                Err(e) => return Err(e),
            }
        }

        tracing::info!(
            "Pushed segment {} for camera {}",
            task.sequence,
            task.camera_id
        );

        Ok(true)
    }

    /// Check if an error is retryable (transient network/server error).
    fn is_retryable(err: &crate::error::Error) -> bool {
        match err {
            crate::error::Error::Api(msg) => {
                let retryable_prefixes = [
                    "Push segment failed: 408",
                    "Push segment failed: 429",
                    "Push segment failed: 500",
                    "Push segment failed: 502",
                    "Push segment failed: 503",
                    "Push segment failed: 504",
                ];
                retryable_prefixes.iter().any(|p| msg.starts_with(p))
            }
            crate::error::Error::HttpClient(_) => true, // reqwest errors (timeouts, connection resets)
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uploader_config_default() {
        let config = UploaderConfig::default();
        assert_eq!(config.retry_count, 3);
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
