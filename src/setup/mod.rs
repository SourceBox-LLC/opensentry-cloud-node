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
//! Setup wizard for OpenSentry CloudNode
//!
//! Beautiful animated terminal-based setup experience

pub mod animations;
pub mod platform;
pub mod recovery;
pub mod tui;
pub mod ui;
pub mod validator;

use anyhow::Result;
use std::path::PathBuf;

pub use animations::{
    animate_rainbow_text, clear_screen, draw_box, draw_expanding_border, fade_in, fade_in_lines,
    print_centered, pulse_text, rainbow_text, rainbow_text_offset, show_confetti,
    show_mini_celebration, Spinner,
};
pub use platform::PlatformInfo;
pub use recovery::{show_registration_error, show_success, show_warning, RegistrationError};
pub use tui::run_tui_setup;
pub use validator::ValidationResult;

/// Setup configuration
#[derive(Debug, Clone)]
pub struct SetupConfig {
    pub node_id: String,
    pub api_key: String,
    pub api_url: String,
    pub deployment_method: DeploymentMethod,
    pub output_dir: PathBuf,
    pub auto_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeploymentMethod {
    WindowsNative,
    WSL2,
    LinuxNative,
    Docker,
}

/// Run interactive setup wizard
/// Returns: Ok(true) if should auto-start, Ok(false) if should wait for user
pub fn run_setup() -> Result<bool> {
    // Always try TUI setup - works when double-clicked on Windows
    // The TUI uses inquire which handles both terminal and pipe scenarios
    match tui::run_tui_setup() {
        Ok(auto_start) => Ok(auto_start),
        Err(e) => {
            // If TUI fails, show simple prompt-based setup
            eprintln!("\n  Interactive setup failed: {}", e);
            eprintln!("  This can happen when running without a proper terminal.");
            eprintln!("\n  To run setup, open a terminal and run:");
            eprintln!("    opensentry-cloudnode.exe setup");
            eprintln!("\n  Config is stored in data/node.db (run setup to configure).");

            // Pause on Windows so user sees the error before window closes
            #[cfg(target_os = "windows")]
            {
                eprintln!("\n  Press Enter to exit...");
                let _ = std::io::stdin().read_line(&mut String::new());
            }

            std::process::exit(1);
        }
    }
}

/// Run non-interactive quick setup with pre-supplied credentials.
///
/// Validates credentials against Command Center, saves config to DB,
/// auto-detects GPU encoder, and creates data directories — all without
/// any user prompts. Designed to be invoked as:
///
///   opensentry-cloudnode setup --url <URL> --node-id <ID> --key <KEY>
pub fn run_quick_setup(api_url: &str, node_id: &str, api_key: &str) -> Result<()> {
    use colored::Colorize;

    println!();
    println!(
        "  {} OpenSentry CloudNode — Quick Setup",
        "⚡".cyan()
    );
    println!("  ────────────────────────────────────────");
    println!();

    // ── Validate inputs ──────────────────────────────────────────
    if node_id.len() != 8 || !node_id.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("Invalid node ID: must be 8 hex characters (got '{}')", node_id);
    }
    let parts: Vec<&str> = api_key.split('-').collect();
    if parts.len() != 5 {
        anyhow::bail!("Invalid API key: must be UUID format (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)");
    }
    if !api_url.starts_with("http://") && !api_url.starts_with("https://") {
        anyhow::bail!("Invalid URL: must start with http:// or https://");
    }

    // ── Validate connection ──────────────────────────────────────
    print!("  Validating credentials... ");
    let rt = tokio::runtime::Runtime::new()?;
    let validation = rt.block_on(validator::validate_api_connection(api_url, node_id, api_key))?;

    if !validation.is_valid {
        println!("{}", "FAILED".red().bold());
        if let Some(msg) = &validation.error_message {
            for line in msg.lines() {
                eprintln!("  {}", line);
            }
        }
        std::process::exit(1);
    }

    let node_name = validation
        .node_name
        .as_deref()
        .unwrap_or(node_id);
    println!("{} ({})", "OK".green().bold(), node_name);

    // ── Save config to database ──────────────────────────────────
    print!("  Saving configuration...   ");
    let output_dir = std::env::current_dir()?;
    let db_path = output_dir.join("data").join("node.db");
    std::fs::create_dir_all(db_path.parent().unwrap())?;

    let db = crate::storage::NodeDatabase::new(&db_path)
        .map_err(|e| anyhow::anyhow!("DB error: {}", e))?;

    let app_config = crate::config::Config {
        node: crate::config::NodeConfig {
            name: crate::config::NodeConfig::default().name,
            node_id: Some(node_id.to_string()),
        },
        cloud: crate::config::CloudConfig {
            api_url: api_url.to_string(),
            api_key: api_key.to_string(),
            heartbeat_interval: 30,
        },
        ..Default::default()
    };
    app_config
        .save_to_db(&db)
        .map_err(|e| anyhow::anyhow!("Config save error: {}", e))?;
    println!("{}", "OK".green().bold());

    // ── Auto-detect GPU encoder ──────────────────────────────────
    print!("  Detecting video encoder... ");

    let ffmpeg_path = {
        #[cfg(target_os = "windows")]
        {
            let local = output_dir.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if local.exists() {
                local.to_string_lossy().to_string()
            } else {
                "ffmpeg".to_string()
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            "ffmpeg".to_string()
        }
    };

    let hw_encoder =
        crate::streaming::hls_generator::HlsGenerator::detect_hw_encoder(&ffmpeg_path);

    match &hw_encoder {
        Some(enc) => {
            let label = match enc.as_str() {
                "h264_nvenc" => "NVIDIA NVENC (GPU)",
                "h264_qsv" => "Intel Quick Sync (GPU)",
                "h264_amf" => "AMD AMF (GPU)",
                "h264_v4l2m2m" => "V4L2 M2M (Hardware)",
                other => other,
            };
            db.set_config("encoder", enc)
                .map_err(|e| anyhow::anyhow!("Config save error: {}", e))?;
            println!("{}", label.green().bold());
        }
        None => {
            db.set_config("encoder", "libx264")
                .map_err(|e| anyhow::anyhow!("Config save error: {}", e))?;
            println!("{}", "libx264 (CPU)".yellow());
        }
    }

    // ── Create data directories ──────────────────────────────────
    print!("  Creating directories...   ");
    std::fs::create_dir_all(output_dir.join("data").join("hls"))?;
    println!("{}", "OK".green().bold());

    // ── Check FFmpeg availability ────────────────────────────────
    let has_ffmpeg = platform::check_ffmpeg().unwrap_or(false);
    if !has_ffmpeg {
        println!();
        println!(
            "  {} FFmpeg not found — install it before starting.",
            "⚠".yellow()
        );
        #[cfg(target_os = "linux")]
        println!("    sudo apt install ffmpeg");
        #[cfg(target_os = "macos")]
        println!("    brew install ffmpeg");
    }

    // ── Done ─────────────────────────────────────────────────────
    println!();
    println!("  {} Setup complete — starting CloudNode...", "✓".green().bold());
    println!();

    Ok(())
}
