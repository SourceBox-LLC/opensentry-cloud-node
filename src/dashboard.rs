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
//! Live node dashboard
//!
//! Renders a persistent full-screen dashboard that updates in place.
//! Replaces raw tracing log output while the node is running.
//!
//! Layout:
//!
//! ```
//! ╔══ ▸ OPENSENTRY CLOUDNODE ═══════════════════════════════════════════════╗
//! ║  Node: abc12345  │  API: opensentry-command.fly.dev  │  ↑ 142 segments ║
//! ╠══ CAMERAS ══════════════════════════════════════════════════════════════╣
//! ║  ● MEE USB Camera    1920×1080   avc1.42e01e / mp4a.40.2   streaming   ║
//! ╠══ LOG ══════════════════════════════════════════════════════════════════╣
//! ║  06:31:12  ✓  Segment 00142 uploaded (188 KB)                          ║
//! ║  06:31:08  ✓  Codec reported: avc1.42e01e, mp4a.40.2                  ║
//! ║  06:31:05  ✓  Registered with cloud                                    ║
//! ╚════════════════════════════════════════════════════════════════════════╝
//! ```

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use std::path::PathBuf;

use chrono::Local;
use colored::Colorize;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal,
};

use crate::storage::NodeDatabase;

// ─── Box drawing ────────────────────────────────────────────────────────────
const TL: &str = "╔";
const TR: &str = "╗";
const BL: &str = "╚";
const BR: &str = "╝";
const H: &str = "═";
const V: &str = "║";
const ML: &str = "╠";
const MR: &str = "╣";

// ─── Log entry ───────────────────────────────────────────────────────────────

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

// ─── Camera state ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CameraState {
    pub name: String,
    pub resolution: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub status: CameraStatus,
    pub segments_uploaded: u64,
    pub bytes_uploaded: u64,
}

#[derive(Clone, PartialEq)]
pub enum CameraStatus {
    Starting,
    Streaming,
    Error(String),
    Offline,
}

// ─── Views & settings ───────────────────────────────────────────────────────

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

// ─── Shared dashboard state ──────────────────────────────────────────────────

pub struct DashboardState {
    pub node_id: String,
    pub api_url: String,
    pub cameras: Vec<CameraState>,
    pub logs: VecDeque<LogEntry>,
    pub total_segments: u64,
    pub uptime_start: Instant,
    /// Maximum log lines to keep in memory
    log_capacity: usize,
    /// Current input bar text
    pub input_text: String,
    /// Cursor position within input text
    pub input_cursor: usize,
    /// Suppress debug logs until this instant (lets command output stay visible)
    pub suppress_debug_until: Option<Instant>,
    /// Persistent command output shown in a box above the input bar
    pub command_output: Vec<String>,
    /// Which view/screen is currently active
    pub current_view: View,
    /// Config info for the settings page
    pub settings: SettingsInfo,
    /// Database handle (for action commands like /wipe)
    pub db: Option<NodeDatabase>,
    /// HLS output directory (for cleanup on /wipe)
    pub hls_dir: Option<PathBuf>,
}

impl DashboardState {
    pub fn new(node_id: impl Into<String>, api_url: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            api_url: api_url.into(),
            cameras: Vec::new(),
            logs: VecDeque::new(),
            total_segments: 0,
            uptime_start: Instant::now(),
            log_capacity: 200,
            input_text: String::new(),
            input_cursor: 0,
            suppress_debug_until: None,
            command_output: Vec::new(),
            current_view: View::Main,
            settings: SettingsInfo::default(),
            db: None,
            hls_dir: None,
        }
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        // Suppress debug-level noise briefly after a command so output stays visible
        if matches!(level, LogLevel::Debug) {
            if let Some(until) = self.suppress_debug_until {
                if Instant::now() < until {
                    return;
                }
                self.suppress_debug_until = None;
            }
        }
        let msg = message.into();

        // Persist to database before creating the display entry
        if let Some(ref db) = self.db {
            let ts = Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
            let lvl = match level {
                LogLevel::Info  => "INFO",
                LogLevel::Warn  => "WARN",
                LogLevel::Error => "ERROR",
                LogLevel::Debug => "DEBUG",
            };
            let _ = db.save_log(&ts, lvl, &msg);
        }

        let entry = LogEntry::new(level, msg);
        self.logs.push_back(entry);
        while self.logs.len() > self.log_capacity {
            self.logs.pop_front();
        }
    }

    pub fn add_camera(&mut self, state: CameraState) {
        self.cameras.push(state);
    }

    pub fn update_camera_status(&mut self, name: &str, status: CameraStatus) {
        if let Some(cam) = self.cameras.iter_mut().find(|c| c.name == name) {
            cam.status = status;
        }
    }

    pub fn record_upload(&mut self, camera_name: &str, bytes: u64) {
        self.total_segments += 1;
        if let Some(cam) = self.cameras.iter_mut().find(|c| c.name == camera_name) {
            cam.segments_uploaded += 1;
            cam.bytes_uploaded += bytes;
        }
    }

    pub fn set_codec(&mut self, camera_name: &str, video: &str, audio: &str) {
        if let Some(cam) = self.cameras.iter_mut().find(|c| c.name == camera_name) {
            cam.video_codec = video.to_string();
            cam.audio_codec = audio.to_string();
        }
    }

    fn uptime(&self) -> String {
        let secs = self.uptime_start.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        if h > 0 {
            format!("{}h {}m {}s", h, m, s)
        } else if m > 0 {
            format!("{}m {}s", m, s)
        } else {
            format!("{}s", s)
        }
    }
}

// ─── Shared handle ────────────────────────────────────────────────────────────

/// Thread-safe handle to the dashboard state.
#[derive(Clone)]
pub struct Dashboard(pub Arc<Mutex<DashboardState>>);

impl Dashboard {
    pub fn new(node_id: impl Into<String>, api_url: impl Into<String>) -> Self {
        Self(Arc::new(Mutex::new(DashboardState::new(node_id, api_url))))
    }

    pub fn log_info(&self, msg: impl Into<String>) {
        if let Ok(mut s) = self.0.lock() {
            s.log(LogLevel::Info, msg);
        }
    }

    pub fn log_warn(&self, msg: impl Into<String>) {
        if let Ok(mut s) = self.0.lock() {
            s.log(LogLevel::Warn, msg);
        }
    }

    pub fn log_error(&self, msg: impl Into<String>) {
        if let Ok(mut s) = self.0.lock() {
            s.log(LogLevel::Error, msg);
        }
    }

    pub fn set_settings(&self, info: SettingsInfo) {
        if let Ok(mut s) = self.0.lock() {
            s.settings = info;
        }
    }

    pub fn set_db(&self, db: NodeDatabase, hls_dir: PathBuf) {
        if let Ok(mut s) = self.0.lock() {
            s.db = Some(db);
            s.hls_dir = Some(hls_dir);
        }
    }

    /// Pre-populate the TUI log buffer with entries from the database so logs
    /// survive restarts.
    pub fn load_logs_from_db(&self) {
        if let Ok(mut s) = self.0.lock() {
            let db = match s.db {
                Some(ref db) => db.clone(),
                None => return,
            };
            let rows = match db.load_recent_logs(s.log_capacity) {
                Ok(r) => r,
                Err(_) => return,
            };
            for (timestamp, level_str, message) in rows {
                let level = match level_str.as_str() {
                    "WARN"  => LogLevel::Warn,
                    "ERROR" => LogLevel::Error,
                    "DEBUG" => LogLevel::Debug,
                    _       => LogLevel::Info,
                };
                // Use the stored timestamp's time portion for display
                let time = if timestamp.len() >= 19 {
                    timestamp[11..19].to_string()
                } else {
                    timestamp.clone()
                };
                s.logs.push_back(LogEntry { time, level, message });
            }
        }
    }

    pub fn log_debug(&self, msg: impl Into<String>) {
        if let Ok(mut s) = self.0.lock() {
            s.log(LogLevel::Debug, msg);
        }
    }

    pub fn add_camera(&self, state: CameraState) {
        if let Ok(mut s) = self.0.lock() {
            s.add_camera(state);
        }
    }

    pub fn update_camera_status(&self, name: &str, status: CameraStatus) {
        if let Ok(mut s) = self.0.lock() {
            s.update_camera_status(name, status);
        }
    }

    pub fn record_upload(&self, camera_name: &str, bytes: u64) {
        if let Ok(mut s) = self.0.lock() {
            s.record_upload(camera_name, bytes);
        }
    }

    pub fn set_codec(&self, camera_name: &str, video: &str, audio: &str) {
        if let Ok(mut s) = self.0.lock() {
            s.set_codec(camera_name, video, audio);
        }
    }

    /// Render one full frame to stdout. Redraws from top each time.
    pub fn render(&self) {
        let state = match self.0.lock() {
            Ok(s) => s,
            Err(_) => return,
        };

        let w = term_width();
        let mut out = String::with_capacity(4096);

        // Hide cursor during redraw to prevent flicker, then move to top-left.
        // Do NOT clear screen (\x1B[2J) — overwrite lines in place instead.
        out.push_str("\x1B[?25l\x1B[H");

        // ── Header ───────────────────────────────────────────────────────────
        let title = " ▸ OPENSENTRY CLOUDNODE ";
        let title_len = title.chars().count();
        let fill = w.saturating_sub(2 + title_len);
        out.push_str(&format!(
            "{}{}{}{}\x1B[K\n",
            cyan_bold(TL),
            cyan_bold(&format!("{}{}", H, title)),
            cyan_bold(&H.repeat(fill.saturating_sub(1))),
            cyan_bold(TR),
        ));

        // Status bar
        let api_short = truncate(
            &state.api_url.replace("https://", "").replace("http://", ""),
            30,
        );
        let total_bytes: u64 = state.cameras.iter().map(|c| c.bytes_uploaded).sum();
        let data_str = format_bytes(total_bytes);
        let status_content = format!(
            "  Node: {}   │   {}   │   ↑ {} segs  {}   │   ⏱ {}",
            state.node_id.cyan().bold(),
            api_short.white(),
            state.total_segments.to_string().cyan(),
            format!("({})", data_str).dimmed(),
            state.uptime().white(),
        );
        out.push_str(&panel_row_str(&status_content, w));
        out.push('\n');

        if state.current_view == View::Settings {
            // ── Settings page ────────────────────────────────────────────────
            out.push_str(&section_header("SETTINGS", w));
            out.push('\n');

            let (_, term_h) = terminal::size().unwrap_or((80, 30));
            let content_rows = (term_h as usize).saturating_sub(5);
            let divider_w = w.saturating_sub(10); // inner divider width

            let s = &state.settings;
            let kw = 20; // key column width
            let mut lines: Vec<String> = Vec::new();

            // ── NODE section
            lines.push(String::new());
            lines.push(settings_divider("NODE", divider_w));
            lines.push(settings_kv("Node ID", &state.node_id, kw));
            lines.push(settings_kv("Name", &s.node_name, kw));
            lines.push(settings_kv("API URL",
                &state.api_url.replace("https://", "").replace("http://", ""), kw));
            lines.push(settings_kv("Heartbeat", &format!("{} s", s.heartbeat_interval), kw));

            // ── STORAGE section
            lines.push(String::new());
            lines.push(settings_divider("STORAGE", divider_w));
            lines.push(settings_kv("Path", &s.storage_path, kw));
            lines.push(settings_kv("Max Size", &format!("{} GB", s.max_size_gb), kw));

            // ── STREAMING section
            lines.push(String::new());
            lines.push(settings_divider("STREAMING", divider_w));
            lines.push(settings_kv("Segment", &format!("{} s", s.segment_duration), kw));
            lines.push(settings_kv("FPS", &s.fps.to_string(), kw));
            lines.push(format!("     {}   {}",
                pad_right(&"Encoder".white().to_string(), 7, kw),
                if s.encoder.is_empty() { "auto-detect".dimmed().to_string() }
                else { s.encoder.bright_green().to_string() }));
            lines.push(format!("     {}   {}",
                pad_right(&"HLS".white().to_string(), 3, kw),
                if s.hls_enabled { "enabled".bright_green().to_string() }
                else { "disabled".bright_red().to_string() }));

            // ── MOTION section
            lines.push(String::new());
            lines.push(settings_divider("MOTION", divider_w));
            lines.push(format!("     {}   {}",
                pad_right(&"Detection".white().to_string(), 9, kw),
                if s.motion_enabled { "enabled".bright_green().to_string() }
                else { "disabled".dimmed().to_string() }));
            lines.push(settings_kv("Sensitivity", &format!("{:.1}", s.motion_sensitivity), kw));
            lines.push(settings_kv("Cooldown", &format!("{} s", s.motion_cooldown), kw));

            // ── CAMERAS section
            lines.push(String::new());
            lines.push(settings_divider(
                &format!("CAMERAS  {}", format!("({})", state.cameras.len()).dimmed()), divider_w));
            for cam in &state.cameras {
                let status_str = match &cam.status {
                    CameraStatus::Streaming => "streaming".bright_green().to_string(),
                    CameraStatus::Starting  => "starting".yellow().to_string(),
                    CameraStatus::Offline   => "offline".dimmed().to_string(),
                    CameraStatus::Error(e)  => truncate(e, 16).bright_red().to_string(),
                };
                lines.push(format!("     {}  {}  {}",
                    pad_right(&cam.name.white().to_string(), visible_len(&cam.name), kw),
                    pad_right(&cam.resolution.dimmed().to_string(), visible_len(&cam.resolution), 12),
                    status_str,
                ));
            }

            // ── ACTIONS section
            lines.push(String::new());
            lines.push(settings_divider("ACTIONS", divider_w));
            lines.push(settings_action("/set <key> <val>", "Change a setting"));
            lines.push(settings_action("/export-logs", "Save all logs to a file"));
            lines.push(settings_action("/wipe", "Erase all data and reset this node"));
            lines.push(settings_action("/reauth", "Clear credentials and re-run setup"));
            lines.push(String::new());

            // Render with padding
            for line in &lines {
                out.push_str(&panel_row_str(line, w));
                out.push('\n');
            }
            for _ in lines.len()..content_rows {
                out.push_str(&panel_row_str("", w));
                out.push('\n');
            }
        } else {
            // ── Main view: Cameras + Log ─────────────────────────────────────
            out.push_str(&section_header("CAMERAS", w));
            out.push('\n');

            if state.cameras.is_empty() {
                out.push_str(&panel_row_str(
                    &"  No cameras detected".dimmed().to_string(),
                    w,
                ));
                out.push('\n');
            } else {
                // Column headers
                let header = format!(
                    "  {}   {}   {}   {}   {}",
                    pad_right(&"CAMERA".dimmed().to_string(), 6, 28),
                    pad_right(&"RESOLUTION".dimmed().to_string(), 10, 12),
                    pad_right(&"CODEC".dimmed().to_string(), 5, 30),
                    pad_right(&"STATUS".dimmed().to_string(), 6, 14),
                    "SEGS".dimmed(),
                );
                out.push_str(&panel_row_str(&header, w));
                out.push('\n');

                for cam in &state.cameras {
                    let status_str = match &cam.status {
                        CameraStatus::Streaming => "● streaming".bright_green().bold().to_string(),
                        CameraStatus::Starting => "◌ starting…".yellow().to_string(),
                        CameraStatus::Offline => "○ offline".dimmed().to_string(),
                        CameraStatus::Error(e) => {
                            format!("✗ {}", truncate(e, 18)).bright_red().to_string()
                        }
                    };
                    let codec = if cam.video_codec.is_empty() {
                        "detecting…".dimmed().to_string()
                    } else {
                        format!("{} / {}", cam.video_codec.cyan(), cam.audio_codec.cyan())
                    };
                    let line = format!(
                        "  {}   {}   {}   {}   {}",
                        pad_right(
                            &cam.name.white().bold().to_string(),
                            visible_len(&cam.name.white().bold().to_string()),
                            28,
                        ),
                        pad_right(
                            &cam.resolution.dimmed().to_string(),
                            visible_len(&cam.resolution),
                            12,
                        ),
                        pad_right(&codec, visible_len(&codec), 30),
                        pad_right(&status_str, visible_len(&status_str), 14),
                        cam.segments_uploaded.to_string().cyan(),
                    );
                    out.push_str(&panel_row_str(&line, w));
                    out.push('\n');
                }
            }

            // ── Log section ──────────────────────────────────────────────────
            out.push_str(&section_header("LOG", w));
            out.push('\n');

            // How many log lines fit?
            let cam_rows = state.cameras.len().max(1) + 1;
            let cmd_output_rows = if state.command_output.is_empty() {
                0
            } else {
                state.command_output.len() + 1
            };
            let reserved_rows = 7 + cam_rows + cmd_output_rows;
            let (_, term_h) = terminal::size().unwrap_or((80, 30));
            let log_rows = (term_h as usize).saturating_sub(reserved_rows).max(3);

            let visible_logs: Vec<&LogEntry> = state
                .logs
                .iter()
                .rev()
                .take(log_rows)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();

            // Pad with blank lines if fewer logs than space
            let blank_rows = log_rows.saturating_sub(visible_logs.len());
            for _ in 0..blank_rows {
                out.push_str(&panel_row_str("", w));
                out.push('\n');
            }

            for entry in &visible_logs {
                let (icon, colored_msg) = match entry.level {
                    LogLevel::Info => ("✓", entry.message.white().to_string()),
                    LogLevel::Warn => ("⚠", entry.message.yellow().to_string()),
                    LogLevel::Error => ("✗", entry.message.bright_red().to_string()),
                    LogLevel::Debug => ("·", entry.message.dimmed().to_string()),
                };
                let icon_colored = match entry.level {
                    LogLevel::Info => icon.bright_green().to_string(),
                    LogLevel::Warn => icon.yellow().to_string(),
                    LogLevel::Error => icon.bright_red().to_string(),
                    LogLevel::Debug => icon.dimmed().to_string(),
                };
                let line = format!(
                    "  {}  {}  {}",
                    entry.time.dimmed(),
                    icon_colored,
                    colored_msg,
                );
                let truncated = truncate_ansi(&line, w.saturating_sub(4));
                out.push_str(&panel_row_str(&truncated, w));
                out.push('\n');
            }

            // ── Command output panel (persistent, above footer) ─────────────
            if !state.command_output.is_empty() {
                out.push_str(&format!(
                    "{}{}{}\x1B[K\n",
                    cyan_bold(ML),
                    cyan_bold(&H.repeat(w.saturating_sub(2))),
                    cyan_bold(MR),
                ));
                for line in &state.command_output {
                    let content = format!("  {}", line);
                    let truncated = truncate_ansi(&content, w.saturating_sub(4));
                    out.push_str(&panel_row_str(&truncated, w));
                    out.push('\n');
                }
            }
        }

        // ── Footer ───────────────────────────────────────────────────────────
        out.push_str(&format!(
            "{}{}{}\x1B[K",
            cyan_bold(BL),
            cyan_bold(&H.repeat(w.saturating_sub(2))),
            cyan_bold(BR),
        ));

        // Input bar below the box
        if state.input_text.is_empty() {
            let hint = if state.current_view == View::Settings {
                "Esc to go back"
            } else {
                "Type / for commands"
            };
            out.push_str(&format!(
                "\n  {}  {}\x1B[K",
                ">".cyan().bold(),
                hint.dimmed(),
            ));
        } else {
            out.push_str(&format!(
                "\n  {}  {}\x1B[K",
                ">".cyan().bold(),
                state.input_text,
            ));
        }

        // Clear any remaining lines below the TUI from previous frames
        out.push_str("\x1B[J");

        // Save cursor position before dropping lock
        let cursor_col = 5 + state.input_cursor;

        // Drop lock before writing to stdout
        drop(state);

        // Replace \n with \r\n for raw mode compatibility
        let out = out.replace('\n', "\r\n");

        // Write frame, then position cursor at input bar and show it
        print!("{}\r\x1B[{}C\x1B[?25h", out, cursor_col);
        io::stdout().flush().ok();
    }

    /// Export all logs to a text file.
    /// Pulls from the SQLite database for a complete history, falling back to
    /// the in-memory buffer if the DB is unavailable.
    pub fn export_logs(&self, path: &std::path::Path) {
        let state = match self.0.lock() {
            Ok(s) => s,
            Err(_) => return,
        };

        let mut lines = Vec::new();
        lines.push("OpenSentry CloudNode — Log Export".to_string());
        lines.push(format!("Node: {}  |  API: {}", state.node_id, state.api_url));
        lines.push(format!("Total segments: {}  |  Uptime: {}", state.total_segments, state.uptime()));
        lines.push(String::new());

        // Try to load the full log history from the database
        let db_logs = state.db.as_ref().and_then(|db| db.load_recent_logs(10_000).ok());

        if let Some(rows) = db_logs {
            for (timestamp, level, message) in &rows {
                lines.push(format!("{} [{}] {}", timestamp, level, message));
            }
        } else {
            // Fallback: export in-memory buffer only
            for entry in &state.logs {
                let level = match entry.level {
                    LogLevel::Info  => "INFO ",
                    LogLevel::Warn  => "WARN ",
                    LogLevel::Error => "ERROR",
                    LogLevel::Debug => "DEBUG",
                };
                lines.push(format!("{} [{}] {}", entry.time, level, entry.message));
            }
        }

        drop(state);

        if let Err(e) = std::fs::write(path, lines.join("\n")) {
            eprintln!("Failed to export logs: {}", e);
        }
    }

    /// Start the render loop in the current thread. Redraws every `interval`.
    /// Enables raw mode for character-by-character input. Blocks until `stop`.
    pub fn run_render_loop(&self, interval: Duration, stop: Arc<std::sync::atomic::AtomicBool>) {
        let _ = terminal::enable_raw_mode();

        // Clear screen initially
        print!("\x1B[2J\x1B[H");
        io::stdout().flush().ok();

        let mut input = String::new();
        let mut cursor_pos: usize = 0;
        let mut history: Vec<String> = Vec::new();
        let mut history_idx: Option<usize> = None;

        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            // Push input state for rendering
            if let Ok(mut s) = self.0.lock() {
                s.input_text.clone_from(&input);
                s.input_cursor = cursor_pos;
            }

            self.render();

            // Poll for keyboard events (replaces thread::sleep)
            if event::poll(interval).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    // Ignore key release events (Windows sends both press + release)
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        KeyCode::Char(c) => {
                            input.insert(cursor_pos, c);
                            cursor_pos += 1;
                            history_idx = None;
                        }
                        KeyCode::Backspace => {
                            if cursor_pos > 0 {
                                cursor_pos -= 1;
                                input.remove(cursor_pos);
                            }
                        }
                        KeyCode::Delete => {
                            if cursor_pos < input.len() {
                                input.remove(cursor_pos);
                            }
                        }
                        KeyCode::Left => {
                            cursor_pos = cursor_pos.saturating_sub(1);
                        }
                        KeyCode::Right => {
                            if cursor_pos < input.len() {
                                cursor_pos += 1;
                            }
                        }
                        KeyCode::Home => cursor_pos = 0,
                        KeyCode::End => cursor_pos = input.len(),
                        KeyCode::Up => {
                            if !history.is_empty() {
                                let idx = match history_idx {
                                    Some(i) => i.saturating_sub(1),
                                    None => history.len() - 1,
                                };
                                input = history[idx].clone();
                                cursor_pos = input.len();
                                history_idx = Some(idx);
                            }
                        }
                        KeyCode::Down => {
                            if let Some(idx) = history_idx {
                                if idx + 1 < history.len() {
                                    let new_idx = idx + 1;
                                    input = history[new_idx].clone();
                                    cursor_pos = input.len();
                                    history_idx = Some(new_idx);
                                } else {
                                    input.clear();
                                    cursor_pos = 0;
                                    history_idx = None;
                                }
                            }
                        }
                        KeyCode::Enter => {
                            if !input.is_empty() {
                                let cmd = input.clone();
                                history.push(cmd.clone());
                                history_idx = None;
                                input.clear();
                                cursor_pos = 0;
                                self.execute_command(&cmd, &stop);
                            }
                        }
                        KeyCode::Esc => {
                            input.clear();
                            cursor_pos = 0;
                            history_idx = None;
                            // Navigate back from settings, or clear output on main
                            if let Ok(mut s) = self.0.lock() {
                                if s.current_view != View::Main {
                                    s.current_view = View::Main;
                                } else {
                                    s.command_output.clear();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Cleanup
        let _ = terminal::disable_raw_mode();

        // Clear input for final render
        if let Ok(mut s) = self.0.lock() {
            s.input_text.clear();
            s.input_cursor = 0;
        }
        self.render();

        print!("\r\n");
        io::stdout().flush().ok();
    }

    /// Set the persistent command output panel content.
    fn set_output(&self, lines: Vec<String>) {
        if let Ok(mut s) = self.0.lock() {
            s.command_output = lines;
        }
    }

    /// Clear the command output panel.
    fn clear_output(&self) {
        if let Ok(mut s) = self.0.lock() {
            s.command_output.clear();
        }
    }

    /// Parse and execute a slash command from the input bar.
    fn execute_command(&self, input: &str, stop: &Arc<std::sync::atomic::AtomicBool>) {
        let input = input.trim();

        if !input.starts_with('/') {
            self.set_output(vec!["Commands start with /  — try /help".to_string()]);
            return;
        }

        let parts: Vec<&str> = input[1..].split_whitespace().collect();
        let cmd = parts.first().copied().unwrap_or("");
        let args = if parts.len() > 1 { &parts[1..] } else { &parts[..0] };

        // Check current view for settings-only commands
        let on_settings = self.0.lock().map(|s| s.current_view == View::Settings).unwrap_or(false);

        match cmd {
            "quit" | "exit" | "q" => {
                self.clear_output();
                self.log_warn("Shutting down…");
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            "" | "help" | "?" => {
                if on_settings {
                    self.set_output(vec![
                        "Settings commands:".to_string(),
                        "  /set <key> <value>   Change a setting".to_string(),
                        "  /export-logs         Save logs to file".to_string(),
                        "  /wipe                Erase all data".to_string(),
                        "  /reauth              Reset credentials".to_string(),
                        "  /back                Return to dashboard".to_string(),
                        "  /quit                Stop the node".to_string(),
                        String::new(),
                        "Settings keys: fps, encoder, segment, bitrate,".to_string(),
                        "  motion (on/off), sensitivity, cooldown".to_string(),
                    ]);
                } else {
                    self.set_output(vec![
                        "Available commands:".to_string(),
                        "  /settings      Open settings page".to_string(),
                        "  /status        Show node status".to_string(),
                        "  /clear         Clear the log".to_string(),
                        "  /quit          Stop the node".to_string(),
                    ]);
                }
            }
            "settings" => {
                if let Ok(mut s) = self.0.lock() {
                    s.current_view = View::Settings;
                    s.command_output.clear();
                }
            }
            "back" => {
                if let Ok(mut s) = self.0.lock() {
                    s.current_view = View::Main;
                    s.command_output.clear();
                }
            }
            "clear" | "cls" => {
                if let Ok(mut s) = self.0.lock() {
                    s.logs.clear();
                    s.command_output.clear();
                }
            }
            "status" => {
                let info = if let Ok(s) = self.0.lock() {
                    let total_bytes: u64 = s.cameras.iter().map(|c| c.bytes_uploaded).sum();
                    Some((
                        s.cameras.len(),
                        s.total_segments,
                        format_bytes(total_bytes),
                        s.uptime(),
                    ))
                } else {
                    None
                };
                if let Some((cams, segs, bytes, uptime)) = info {
                    self.set_output(vec![
                        format!("Cameras:  {}", cams),
                        format!("Segments: {}", segs),
                        format!("Uploaded: {}", bytes),
                        format!("Uptime:   {}", uptime),
                    ]);
                }
            }
            "set" if on_settings => {
                if args.len() < 2 {
                    self.set_output(vec![
                        "Usage: /set <key> <value>".to_string(),
                        String::new(),
                        "Keys:".to_string(),
                        "  fps          Frames per second (1-60)".to_string(),
                        "  encoder      Video encoder (libx264, h264_nvenc, …)".to_string(),
                        "  segment      Segment duration in seconds".to_string(),
                        "  bitrate      Encoding bitrate (e.g. 2500k)".to_string(),
                        "  motion       on / off".to_string(),
                        "  sensitivity  Motion threshold 0.0-1.0".to_string(),
                        "  cooldown     Motion cooldown seconds".to_string(),
                    ]);
                } else {
                    let key = args[0];
                    let val = args[1..].join(" ");
                    let (db_key, display_val, ok) = match key {
                        "fps" => {
                            match val.parse::<u32>() {
                                Ok(v) if (1..=60).contains(&v) => ("fps", val.clone(), true),
                                _ => ("", String::new(), false),
                            }
                        }
                        "encoder" => ("encoder", val.clone(), true),
                        "segment" => {
                            match val.parse::<u32>() {
                                Ok(v) if (1..=30).contains(&v) => ("segment_duration", val.clone(), true),
                                _ => ("", String::new(), false),
                            }
                        }
                        "bitrate" => ("bitrate", val.clone(), true),
                        "motion" => {
                            let enabled = matches!(val.as_str(), "on" | "true" | "1" | "yes");
                            let disabled = matches!(val.as_str(), "off" | "false" | "0" | "no");
                            if enabled || disabled {
                                ("motion_enabled", (if enabled { "true" } else { "false" }).to_string(), true)
                            } else {
                                ("", String::new(), false)
                            }
                        }
                        "sensitivity" => {
                            match val.parse::<f64>() {
                                Ok(v) if (0.0..=1.0).contains(&v) => ("motion_sensitivity", val.clone(), true),
                                _ => ("", String::new(), false),
                            }
                        }
                        "cooldown" => {
                            match val.parse::<u64>() {
                                Ok(_) => ("motion_cooldown", val.clone(), true),
                                _ => ("", String::new(), false),
                            }
                        }
                        _ => {
                            self.set_output(vec![
                                format!("Unknown setting: {}", key),
                                "Type /set for a list of keys.".to_string(),
                            ]);
                            return;
                        }
                    };
                    if !ok {
                        self.set_output(vec![format!("Invalid value for {}: {}", key, val)]);
                        return;
                    }
                    let saved = if let Ok(s) = self.0.lock() {
                        if let Some(ref db) = s.db {
                            db.set_config(db_key, &display_val).is_ok()
                        } else { false }
                    } else { false };
                    if saved {
                        // Update the in-memory SettingsInfo so it refreshes immediately
                        if let Ok(mut s) = self.0.lock() {
                            match key {
                                "fps" => s.settings.fps = display_val.parse().unwrap_or(s.settings.fps),
                                "encoder" => s.settings.encoder = display_val.clone(),
                                "segment" => s.settings.segment_duration = display_val.parse().unwrap_or(s.settings.segment_duration),
                                "motion" => s.settings.motion_enabled = display_val == "true",
                                "sensitivity" => s.settings.motion_sensitivity = display_val.parse().unwrap_or(s.settings.motion_sensitivity),
                                "cooldown" => s.settings.motion_cooldown = display_val.parse().unwrap_or(s.settings.motion_cooldown),
                                _ => {}
                            }
                        }
                        self.set_output(vec![
                            format!("Set {} = {} (takes effect on next segment)", key, display_val),
                        ]);
                        self.log_info(format!("Setting changed: {} = {}", key, display_val));
                    } else {
                        self.set_output(vec!["Failed to save setting.".to_string()]);
                    }
                }
            }
            "export-logs" if on_settings => {
                let timestamp = Local::now().format("%Y-%m-%d_%H%M%S");
                let filename = format!("opensentry-logs-{}.txt", timestamp);
                let path = std::path::PathBuf::from(&filename);
                self.export_logs(&path);
                self.set_output(vec![
                    format!("Logs exported to {}", filename),
                ]);
                self.log_info(format!("Logs exported to {}", filename));
            }
            "wipe" if on_settings => {
                let confirm = args.first().copied() == Some("confirm");
                if !confirm {
                    self.set_output(vec![
                        "This will permanently delete ALL stored data:".to_string(),
                        "  - Snapshots, recordings, config".to_string(),
                        "  - HLS segment cache".to_string(),
                        String::new(),
                        "Type /wipe confirm to proceed.".to_string(),
                    ]);
                } else {
                    let result = if let Ok(s) = self.0.lock() {
                        let mut ok = true;
                        if let Some(ref db) = s.db {
                            if let Err(e) = db.wipe_all() {
                                self.log_error(format!("DB wipe failed: {}", e));
                                ok = false;
                            }
                        }
                        if let Some(ref hls) = s.hls_dir {
                            if hls.exists() {
                                let _ = std::fs::remove_dir_all(hls);
                            }
                        }
                        ok
                    } else {
                        false
                    };
                    if result {
                        self.set_output(vec![
                            "All data erased. Shutting down…".to_string(),
                            "Run setup again to reconfigure.".to_string(),
                        ]);
                        self.log_warn("Data wiped — shutting down");
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    } else {
                        self.set_output(vec!["Wipe failed — check logs.".to_string()]);
                    }
                }
            }
            "reauth" if on_settings => {
                let confirm = args.first().copied() == Some("confirm");
                if !confirm {
                    self.set_output(vec![
                        "This will clear your credentials and stop the node.".to_string(),
                        "You will need to run setup again with new credentials.".to_string(),
                        String::new(),
                        "Type /reauth confirm to proceed.".to_string(),
                    ]);
                } else {
                    if let Ok(s) = self.0.lock() {
                        if let Some(ref db) = s.db {
                            // Delete the config rows — can't use set_config("api_key", "")
                            // because api_key is stored encrypted and loading would fail
                            // trying to decrypt an empty plaintext string.
                            let _ = db.delete_config("node_id");
                            let _ = db.delete_config("api_key");
                        }
                    }
                    self.set_output(vec![
                        "Credentials cleared. Shutting down…".to_string(),
                        "Run: opensentry-cloudnode setup".to_string(),
                    ]);
                    self.log_warn("Credentials cleared — shutting down");
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            // Settings-only commands used from main view
            "wipe" | "reauth" | "export-logs" => {
                self.set_output(vec![
                    format!("/{} is only available on the settings page.", cmd),
                    "Type /settings to open it.".to_string(),
                ]);
            }
            _ => {
                self.set_output(vec![
                    format!("Unknown command: /{} — type / for help", cmd),
                ]);
            }
        }
    }
}

// ─── Rendering helpers ────────────────────────────────────────────────────────

fn term_width() -> usize {
    terminal::size().map(|(w, _)| w).unwrap_or(80).max(60) as usize
}

fn cyan_bold(s: &str) -> String {
    s.cyan().bold().to_string()
}

/// Settings page: thin divider with a section label.
fn settings_divider(label: &str, fill_w: usize) -> String {
    let label_vis = visible_len(label);
    let fill = fill_w.saturating_sub(label_vis + 2);
    format!("   {} {}",
        label.cyan().bold(),
        "\u{2500}".repeat(fill).dimmed())
}

/// Settings page: key-value row.
fn settings_kv(key: &str, value: &str, key_width: usize) -> String {
    format!("     {}   {}",
        pad_right(&key.white().to_string(), visible_len(key), key_width),
        value.cyan())
}

/// Settings page: action row.
fn settings_action(cmd: &str, desc: &str) -> String {
    format!("     {}   {}",
        pad_right(&cmd.cyan().bold().to_string(), visible_len(cmd), 16),
        desc.dimmed())
}

fn section_header(label: &str, w: usize) -> String {
    let label_str = format!(" {} ", label);
    let label_len = label_str.chars().count();
    let fill = w.saturating_sub(2 + label_len);
    format!(
        "{}{}{}{}\x1B[K",
        cyan_bold(ML),
        cyan_bold(&label_str),
        cyan_bold(&H.repeat(fill)),
        cyan_bold(MR),
    )
}

fn panel_row_str(content: &str, w: usize) -> String {
    // Border: ║ + space + content + clear-to-EOL + jump-to-col-w + ║
    let inner = w.saturating_sub(4);
    let fitted = truncate_ansi(content, inner);
    // Clear the line FIRST, then use cursor positioning to place
    // the right border. This avoids \x1B[K erasing the border
    // (which happens on terminals with deferred line wrap).
    format!(
        "{} {}\x1B[K\x1B[{}G{}",
        cyan_bold(V),
        fitted,
        w,
        cyan_bold(V),
    )
}

/// Visible character count (strips ANSI escape sequences).
/// Handles all CSI sequences (not just SGR/color codes).
fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1B' {
            // Skip the escape sequence
            match chars.next() {
                Some('[') => {
                    // CSI sequence — consume until a letter (final byte 0x40–0x7E)
                    for nc in chars.by_ref() {
                        if nc.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC sequence — consume until ST (BEL or ESC\)
                    for nc in chars.by_ref() {
                        if nc == '\x07' || nc == '\x1B' {
                            break;
                        }
                    }
                }
                _ => {} // other escape — skip one char
            }
        } else {
            len += 1;
        }
    }
    len
}

/// Pad a string to `width` visible characters.
fn pad_right(s: &str, visible: usize, width: usize) -> String {
    if visible >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - visible))
    }
}

/// Format a byte count as a human-readable string.
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Truncate plain text to `max` chars.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

/// Truncate a string with ANSI codes to `max` visible characters.
fn truncate_ansi(s: &str, max: usize) -> String {
    let mut result = String::new();
    let mut visible = 0;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1B' {
            result.push(c);
            match chars.next() {
                Some('[') => {
                    result.push('[');
                    for nc in chars.by_ref() {
                        result.push(nc);
                        if nc.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(other) => {
                    result.push(other);
                }
                None => break,
            }
        } else if visible < max {
            result.push(c);
            visible += 1;
        } else {
            // Truncated — close any open color sequences
            result.push_str("\x1B[0m");
            break;
        }
    }

    result
}
