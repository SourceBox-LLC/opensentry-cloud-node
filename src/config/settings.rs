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
//! Configuration settings structures

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub node: NodeConfig,
    pub cloud: CloudConfig,
    pub cameras: CamerasConfig,
    pub streaming: StreamingConfig,
    pub recording: RecordingConfig,
    pub storage: StorageConfig,
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub motion: MotionConfig,
}

impl Default for Config {
    fn default() -> Self {
        let _hostname =
            sysinfo::System::host_name().unwrap_or_else(|| "sourcebox-sentry-node".to_string());

        Self {
            node: NodeConfig::default(),
            cloud: CloudConfig::default(),
            cameras: CamerasConfig::default(),
            streaming: StreamingConfig::default(),
            recording: RecordingConfig::default(),
            storage: StorageConfig::default(),
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            motion: MotionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Friendly name shown in dashboard
    pub name: String,

    /// Node ID (assigned by cloud)
    #[serde(skip_serializing)]
    pub node_id: Option<String>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        let _hostname =
            sysinfo::System::host_name().unwrap_or_else(|| "sourcebox-sentry-node".to_string());

        Self {
            name: format!("Node-{}", _hostname),
            node_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// SourceBox Sentry Command Center URL
    pub api_url: String,

    /// Organization API key for authentication
    #[serde(skip_serializing)]
    pub api_key: String,

    /// Heartbeat interval in seconds
    pub heartbeat_interval: u64,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            api_url: "http://localhost:8000".to_string(),
            api_key: String::new(),
            heartbeat_interval: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CamerasConfig {
    /// Auto-detect USB cameras on startup
    pub auto_detect: bool,

    /// Manual camera device paths (used if auto_detect is false)
    #[serde(default)]
    pub devices: Vec<String>,
}

impl Default for CamerasConfig {
    fn default() -> Self {
        Self {
            auto_detect: true,
            devices: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    /// Target frames per second
    pub fps: u32,

    /// JPEG quality for snapshots/stream (1-100)
    pub jpeg_quality: u8,

    /// Video encoder (e.g. "h264_nvenc", "libx264", or empty for auto-detect)
    pub encoder: String,

    /// HLS streaming configuration
    pub hls: HlsConfig,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            jpeg_quality: 85,
            encoder: String::new(),
            hls: HlsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HlsConfig {
    /// Enable HLS streaming
    pub enabled: bool,

    /// Segment duration in seconds
    pub segment_duration: u32,

    /// Number of segments to keep in playlist
    pub playlist_size: u32,

    /// Video bitrate (e.g., "2500k")
    pub bitrate: String,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            segment_duration: 1,
            playlist_size: 15,
            bitrate: "2500k".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// Enable local recording
    pub enabled: bool,

    /// Recording format: "mp4" or "mkv"
    pub format: String,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            format: "mp4".to_string(),
        }
    }
}

/// Storage limits.  The actual storage *location* is not configurable
/// — `paths::data_dir()` is the single source of truth for where the
/// SQLite DB, HLS segments, and recordings live (env var override
/// `SOURCEBOX_SENTRY_DATA_DIR` exists for Docker, otherwise platform
/// default).  A `path` field used to live here, defaulting to
/// `./data`, but it caused the v0.1.39 bug where `Node::new` resolved
/// it relative to cwd and segments landed in unexpected directories
/// depending on launch context (Start menu shortcut → Program Files,
/// admin PowerShell from System32 → C:\Windows\System32, …).
/// Removing the field forecloses that whole bug class.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Maximum storage size in GB (oldest deleted when exceeded)
    pub max_size_gb: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            max_size_gb: 64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionConfig {
    /// Enable motion detection (on by default)
    pub enabled: bool,

    /// Scene-change threshold (0.0 – 1.0). Lower = more sensitive.
    /// FFmpeg's scene score: 0.0 = identical frames, 1.0 = completely different.
    pub threshold: f64,

    /// Minimum seconds between motion events per camera
    pub cooldown_secs: u64,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.02,
            cooldown_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// HTTP server port
    pub port: u16,

    /// HTTP server bind address.
    ///
    /// Defaults to `127.0.0.1` — the local server has no auth and exposes
    /// HLS segments, so binding to `0.0.0.0` would let anyone on the LAN
    /// pull live video. Only change this if you explicitly want LAN-local
    /// HLS playback and understand the implications.
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            bind: "127.0.0.1".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level: trace, debug, info, warn, error
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}
