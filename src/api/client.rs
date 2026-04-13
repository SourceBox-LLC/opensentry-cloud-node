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
//! HTTP Client for OpenSentry Cloud API

use reqwest::Client;

use crate::error::{Error, Result};
use super::types::*;

/// API Client for communicating with OpenSentry Command Center
#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    base_url: String,
    api_key: String,
    node_id: Option<String>,
}

impl ApiClient {
    /// Create a new API client
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Api(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            node_id: None,
        })
    }

    /// Set the node ID (for heartbeat after registration)
    pub fn set_node_id(&mut self, node_id: String) {
        self.node_id = Some(node_id);
    }

    /// Register this node with the cloud
    ///
    /// Sends detected cameras and receives node ID and authentication secret.
    pub async fn register(
        &mut self,
        node_id: &str,
        name: &str,
        cameras: Vec<CameraInfo>,
        video_codec: Option<&str>,
        audio_codec: Option<&str>,
    ) -> Result<RegisterResponse> {
        tracing::info!("Registering node {} with {}...", node_id, self.base_url);

        let request = RegisterRequest {
            node_id: node_id.to_string(),
            name: name.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            cameras,
            video_codec: video_codec.map(|s| s.to_string()),
            audio_codec: audio_codec.map(|s| s.to_string()),
        };

        let response = self.client
            .post(format!("{}/api/nodes/register", self.base_url))
            .header("X-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!("Registration failed ({}): {}", status, body)));
        }

        let registration: RegisterResponse = response.json().await?;

        self.node_id = Some(registration.node_id.clone());

        tracing::info!("Node registered successfully: {}", registration.node_id);

        Ok(registration)
    }

    /// Send heartbeat to cloud
    ///
    /// Should be called every 30 seconds to indicate node is still alive.
    pub async fn heartbeat(&self, local_ip: Option<&str>, camera_statuses: Vec<(String, String)>) -> Result<HeartbeatResponse> {
        let node_id = self.node_id.as_ref()
            .ok_or_else(|| Error::Api("Node not registered".into()))?;

        tracing::debug!("Sending heartbeat for node {}", node_id);

        let cameras: Option<Vec<CameraStatus>> = if camera_statuses.is_empty() {
            None
        } else {
            Some(camera_statuses.into_iter().map(|(id, status)| CameraStatus {
                camera_id: id,
                status,
            }).collect())
        };

        let request = HeartbeatRequest {
            node_id: node_id.clone(),
            local_ip: local_ip.map(|s| s.to_string()),
            cameras,
        };

        let response = self.client
            .post(format!("{}/api/nodes/heartbeat", self.base_url))
            .header("X-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!("Heartbeat failed ({}): {}", status, body)));
        }

        let heartbeat_response: HeartbeatResponse = response.json().await?;

        tracing::debug!("Heartbeat acknowledged at {}", heartbeat_response.timestamp);

        Ok(heartbeat_response)
    }

    /// Get node ID (after registration)
    pub fn node_id(&self) -> Option<&str> {
        self.node_id.as_deref()
    }

    /// Update the API key (called when key is rotated)
    pub fn update_api_key(&mut self, new_api_key: String) {
        self.api_key = new_api_key;
    }

    /// Send heartbeat with retry logic
    pub async fn heartbeat_with_retry(
        &self,
        local_ip: Option<&str>,
        camera_statuses: Vec<(String, String)>,
        max_retries: u32,
    ) -> Result<HeartbeatResponse> {
        let mut attempts = 0;
        let mut delay = std::time::Duration::from_secs(1);

        loop {
            match self.heartbeat(local_ip, camera_statuses.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        return Err(e);
                    }
                    tracing::warn!("Heartbeat failed (attempt {}/{}): {}. Retrying in {:?}...",
                        attempts, max_retries, e, delay);
                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(delay * 2, std::time::Duration::from_secs(30));
                }
            }
        }
    }

    /// Report detected codec for a camera stream
    pub async fn report_codec(
        &self,
        camera_id: &str,
        video_codec: &str,
        audio_codec: &str,
    ) -> Result<()> {
        let _node_id = self
            .node_id
            .as_ref()
            .ok_or_else(|| Error::Api("Node not registered".into()))?;

        tracing::info!(
            "Reporting codec for camera {}: video={}, audio={}",
            camera_id,
            video_codec,
            audio_codec
        );

        let codec_data = serde_json::json!({
            "video_codec": video_codec,
            "audio_codec": audio_codec,
        });

        let response = self
            .client
            .post(format!(
                "{}/api/cameras/{}/codec",
                self.base_url, camera_id
            ))
            .header("X-Node-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&codec_data)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!(
                "Report codec failed ({}): {}",
                status, body
            )));
        }

        tracing::info!("Codec reported for camera {}", camera_id);

        Ok(())
    }

    /// Push an HLS segment directly to the backend's in-memory cache.
    /// Replaces the old Tigris presigned URL flow — no S3 involved.
    pub async fn push_segment(
        &self,
        camera_id: &str,
        filename: &str,
        data: bytes::Bytes,
    ) -> Result<()> {
        let _node_id = self.node_id.as_ref()
            .ok_or_else(|| Error::Api("Node not registered".into()))?;

        let response = self.client
            .post(format!(
                "{}/api/cameras/{}/push-segment?filename={}",
                self.base_url, camera_id, filename
            ))
            .header("X-Node-API-Key", &self.api_key)
            .header("Content-Type", "video/mp2t")
            .body(data)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!("Push segment failed ({}): {}", status, body)));
        }

        Ok(())
    }

    /// Report a motion detection event to the backend via HTTP POST.
    /// This is the reliable delivery path — works even when WebSocket is down.
    pub async fn report_motion(
        &self,
        camera_id: &str,
        score: u32,
        timestamp: &str,
        segment_seq: u64,
    ) -> Result<()> {
        let _node_id = self.node_id.as_ref()
            .ok_or_else(|| Error::Api("Node not registered".into()))?;

        let body = serde_json::json!({
            "score": score,
            "timestamp": timestamp,
            "segment_seq": segment_seq,
        });

        let response = self.client
            .post(format!(
                "{}/api/cameras/{}/motion",
                self.base_url, camera_id
            ))
            .header("X-Node-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!("Report motion failed ({}): {}", status, body)));
        }

        Ok(())
    }

    /// Update the HLS playlist on the server
    pub async fn update_playlist(&self, camera_id: &str, playlist_content: &str) -> Result<()> {
        let _node_id = self.node_id.as_ref()
            .ok_or_else(|| Error::Api("Node not registered".into()))?;

        tracing::debug!("Updating playlist for camera {}", camera_id);

        let response = self.client
            .post(format!("{}/api/cameras/{}/playlist", self.base_url, camera_id))
            .header("X-Node-API-Key", &self.api_key)
            .header("Content-Type", "text/plain")
            .body(playlist_content.to_string())
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Api(format!("Update playlist failed ({}): {}", status, body)));
        }

        tracing::debug!("Playlist updated for camera {}", camera_id);

        Ok(())
    }
}