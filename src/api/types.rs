// SourceBox Sentry CloudNode - Camera streaming node for SourceBox Sentry Cloud
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

    /// Node software version.  Wire name is `node_version` to match the
    /// backend's Pydantic schema — Pydantic's default `extra="ignore"`
    /// would silently drop a field called `version`, which is how this
    /// was broken before.
    #[serde(rename = "node_version")]
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

    /// CloudNode build version (`env!("CARGO_PKG_VERSION")`).
    ///
    /// The backend uses this to gate too-old nodes (HTTP 426) and to flag
    /// "update available" when we ship a newer release.  Always sent — old
    /// backends that don't know the field just ignore it via Pydantic's
    /// extra-field tolerance.
    ///
    /// Wire name is `node_version` to match the backend schema.  If this
    /// serializes as plain `version`, Pydantic drops it and every node
    /// looks legacy to the gate.
    #[serde(rename = "node_version")]
    pub version: String,
}

/// Camera status for heartbeat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraStatus {
    /// Camera ID
    pub camera_id: String,

    /// Pipeline state: one of `starting`, `streaming`, `restarting`,
    /// `failed`, `error`, `offline`. Replaces the hardcoded "streaming"
    /// that used to make every node look healthy even with a dead
    /// FFmpeg pipeline.
    pub status: String,

    /// Human-readable failure reason for `restarting` / `failed` /
    /// `error` states. `None` when healthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
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

    /// Newer CloudNode release available (e.g. "0.2.0").
    ///
    /// Set when the backend's `LATEST_NODE_VERSION` is ahead of what we
    /// reported.  CloudNode logs a one-line "update available" warning when
    /// this changes; we deliberately do NOT auto-update because operators
    /// are running this on their own hardware.  `None` means we're current.
    #[serde(default)]
    pub update_available: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_request_serializes_version_field() {
        // Backend's Pydantic schema declares `node_version` and defaults to
        // extra="ignore", so if we ever drop the #[serde(rename)] this
        // payload would serialize as `version`, Pydantic would silently
        // drop it, and every node would look legacy to the update gate.
        // Pin the exact wire key here.
        let req = HeartbeatRequest {
            node_id: "nd_42".into(),
            local_ip: None,
            cameras: None,
            version: "0.1.0".into(),
        };
        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json.get("node_version").and_then(|v| v.as_str()), Some("0.1.0"));
        assert!(json.get("version").is_none(), "must serialize as node_version, not version");
        assert_eq!(json.get("node_id").and_then(|v| v.as_str()), Some("nd_42"));
    }

    #[test]
    fn heartbeat_response_parses_with_update_available() {
        let raw = r#"{
            "success": true,
            "timestamp": "2026-04-14T12:00:00",
            "update_available": "0.2.0"
        }"#;
        let parsed: HeartbeatResponse = serde_json::from_str(raw).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.update_available.as_deref(), Some("0.2.0"));
        assert!(!parsed.key_rotated);
    }

    #[test]
    fn heartbeat_response_parses_without_update_available() {
        // Backwards compat: an old backend that doesn't set the new field
        // must still produce a valid response.  #[serde(default)] makes
        // this work — this test pins the contract.
        let raw = r#"{
            "success": true,
            "timestamp": "2026-04-14T12:00:00"
        }"#;
        let parsed: HeartbeatResponse = serde_json::from_str(raw).unwrap();
        assert!(parsed.success);
        assert!(parsed.update_available.is_none());
    }

    #[test]
    fn register_request_includes_version() {
        // Wire key MUST be `node_version` so the backend's Pydantic schema
        // picks it up.  The historical `version` key was silently dropped
        // by Pydantic's default extra="ignore", so every CloudNode looked
        // legacy at register time and the 426 gate never fired.  Pin the
        // correct name here so the bug can't come back.
        let req = RegisterRequest {
            node_id: "nd_42".into(),
            name: "Test".into(),
            version: "0.1.0".into(),
            cameras: vec![],
            video_codec: None,
            audio_codec: None,
        };
        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json.get("node_version").and_then(|v| v.as_str()), Some("0.1.0"));
        assert!(json.get("version").is_none(), "must serialize as node_version, not version");
    }
}

