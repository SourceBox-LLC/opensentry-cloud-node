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
//! API Request/Response Types

use serde::{Deserialize, Serialize};

/// Camera information sent during registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraInfo {
    /// Device path (e.g., /dev/video0)
    pub device_path: String,

    /// Camera name
    pub name: String,

    /// Width in pixels
    pub width: u32,

    /// Height in pixels
    pub height: u32,

    /// Supported capabilities
    pub capabilities: Vec<String>,
}

impl From<crate::camera::DetectedCamera> for CameraInfo {
    fn from(cam: crate::camera::DetectedCamera) -> Self {
        Self {
            device_path: cam.device_path,
            name: cam.name,
            width: cam.preferred_resolution.0,
            height: cam.preferred_resolution.1,
            capabilities: vec!["streaming".to_string()],
        }
    }
}

/// Node registration request
#[derive(Debug, Serialize)]
pub struct RegisterRequest {
    /// Node ID (assigned by cloud)
    pub node_id: String,

    /// Node name
    pub name: String,

    /// Node software version
    pub version: String,

    /// Detected cameras
    pub cameras: Vec<CameraInfo>,

    /// Video codec (detected during setup)
    pub video_codec: Option<String>,

    /// Audio codec (detected during setup)
    pub audio_codec: Option<String>,
}

/// Node registration response
#[derive(Debug, Deserialize)]
pub struct RegisterResponse {
    /// Assigned node ID
    pub node_id: String,

    /// Node secret for subsequent API calls
    #[serde(default)]
    pub node_secret: String,

    /// Status (updated, pending, etc.)
    #[serde(default)]
    pub status: String,

    /// Camera ID mapping (device_path -> camera_id)
    #[serde(default)]
    pub cameras: std::collections::HashMap<String, String>,
}

/// Node registration info (stored locally)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegistration {
    /// Node ID
    pub node_id: String,

    /// Node name
    pub name: String,

    /// Node secret
    pub secret: String,

    /// Organization ID
    pub org_id: String,

    /// Registration timestamp
    pub registered_at: i64,
}

/// Heartbeat request
#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    /// Node ID
    pub node_id: String,

    /// Local IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_ip: Option<String>,

    /// Camera statuses
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cameras: Option<Vec<CameraStatus>>,
}

/// Camera status for heartbeat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraStatus {
    /// Camera ID
    pub camera_id: String,

    /// Camera status (online, offline, error)
    pub status: String,
}

/// Heartbeat response
#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    /// Success
    pub success: bool,

    /// Server timestamp
    pub timestamp: String,

    /// Key rotation notification (if API key was rotated)
    #[serde(default)]
    pub key_rotated: bool,

    /// New API key (if rotated)
    #[serde(default)]
    pub new_api_key: Option<String>,
}

fn default_upload_expiry() -> u32 {
    300
}

/// Single URL entry in a batch response
#[derive(Debug, Clone, Deserialize)]
pub struct BatchUploadUrl {
    /// Segment sequence number
    pub sequence: u64,

    /// Segment filename (e.g., "segment_00000.ts")
    pub filename: String,

    /// Presigned upload URL
    pub upload_url: String,
}

/// Batch upload URLs response
#[derive(Debug, Deserialize)]
pub struct BatchUploadUrlsResponse {
    /// Presigned URLs for segments
    pub urls: Vec<BatchUploadUrl>,

    /// Presigned URL for uploading the playlist directly to Tigris
    pub playlist_upload_url: String,

    /// URL expiry time in seconds
    #[serde(default = "default_upload_expiry")]
    pub expires_in: u32,
}
