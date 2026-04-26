//! Small shared data types used across the dashboard module.
//!
//! Pure data — no methods that touch state, no rendering. Lives in its own
//! file so `state.rs`, `handle.rs`, `render.rs`, and external consumers
//! (`api::websocket`, `node::runner`, `streaming::*`) can import these
//! without pulling in the rest of the dashboard machinery.

use chrono::Local;

#[derive(Clone)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
}

#[derive(Clone)]
pub struct LogEntry {
    pub time: String,
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            time: Local::now().format("%H:%M:%S").to_string(),
            level,
            message: message.into(),
        }
    }
}

#[derive(Clone)]
pub struct CameraState {
    pub name: String,
    /// Cloud-side camera_id (e.g. `<node_id>_<sanitized_device>`). Used when
    /// the heartbeat needs to look up this camera's pipeline state by ID.
    /// Empty string for cameras that haven't been registered with the cloud
    /// yet (e.g. offline / detection-only rows in the TUI).
    pub camera_id: String,
    pub resolution: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub status: CameraStatus,
    pub segments_uploaded: u64,
    pub bytes_uploaded: u64,
}

#[derive(Clone, PartialEq)]
pub enum CameraStatus {
    /// Pipeline is coming up — FFmpeg has been spawned but we haven't
    /// confirmed segments are being produced.
    Starting,
    /// Pipeline is healthy: FFmpeg alive, segments flowing.
    Streaming,
    /// Legacy "something's wrong" bucket. Keep for backwards compat with
    /// call sites that don't distinguish the new supervised states.
    Error(String),
    /// Node thinks the camera exists but nothing is streaming from it.
    Offline,
    /// FFmpeg just died and the supervisor is about to respawn it.
    /// `attempt` is the number of restarts in the current sliding window.
    Restarting { attempt: u32, last_error: String },
    /// Supervisor hit the restart cap (too many crashes in a short window)
    /// and gave up. The pipeline stays down until someone intervenes.
    Failed { last_error: String },
}

impl CameraStatus {
    /// Short status string + optional error message, for sending over the
    /// wire to the backend heartbeat. Keeps the existing "streaming" /
    /// "offline" vocabulary and adds "starting" / "restarting" / "failed"
    /// for the supervised-pipeline states.
    pub fn to_wire(&self) -> (&'static str, Option<String>) {
        match self {
            CameraStatus::Starting          => ("starting",  None),
            CameraStatus::Streaming         => ("streaming", None),
            CameraStatus::Offline           => ("offline",   None),
            CameraStatus::Error(e)          => ("error",     Some(e.clone())),
            CameraStatus::Restarting { last_error, .. }
                                            => ("restarting", Some(last_error.clone())),
            CameraStatus::Failed { last_error }
                                            => ("failed",    Some(last_error.clone())),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum View {
    Main,
    Settings,
}

/// Config values displayed on the settings page.
#[derive(Clone, Default)]
pub struct SettingsInfo {
    pub node_name: String,
    pub storage_path: String,
    pub max_size_gb: u64,
    pub segment_duration: u32,
    pub fps: u32,
    pub encoder: String,
    pub hls_enabled: bool,
    pub heartbeat_interval: u64,
    pub motion_enabled: bool,
    pub motion_sensitivity: f64,
    pub motion_cooldown: u64,
}
