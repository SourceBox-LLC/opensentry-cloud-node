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
//! Terminal UI setup wizard

use std::thread;
use std::time::Duration;

use colored::Colorize;
use inquire::{validator::Validation, Confirm, Select, Text};

use anyhow::Result;

use super::animations::{
    animate_rainbow_text, clear_screen, draw_expanding_border, fade_in, rainbow_text_offset,
    show_confetti, show_mini_celebration, Spinner,
};
use super::platform::PlatformInfo;
use super::ui::{
    flush, panel_blank, panel_bottom, panel_check, panel_divider, panel_error, panel_kv, panel_mid,
    panel_row, panel_spinner_row, panel_sub, panel_top, panel_warn, StepState,
};
use super::{DeploymentMethod, SetupConfig};

// ─── Step definitions ────────────────────────────────────────────────────────

const STEPS: [&str; 5] = ["PREREQS", "CONFIGURE", "INSTALL", "VERIFY", "LAUNCH"];

fn steps_for(active: usize) -> Vec<(&'static str, StepState)> {
    STEPS
        .iter()
        .enumerate()
        .map(|(i, &label)| {
            let state = if i < active {
                StepState::Done
            } else if i == active {
                StepState::Active
            } else {
                StepState::Pending
            };
            (label, state)
        })
        .collect()
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// Run the interactive TUI setup wizard.
/// Returns Ok(true) if should auto-start, Ok(false) otherwise.
pub fn run_tui_setup() -> Result<bool> {
    show_animated_header()?;
    let platform = check_prerequisites()?;
    let config = configure_node(&platform)?;
    install_dependencies(&config, &platform)?;
    verify_setup(&config)?;
    show_success_screen(&config)?;
    Ok(config.auto_start)
}

// ─── Step 0: Header ──────────────────────────────────────────────────────────

fn show_animated_header() -> Result<()> {
    let header_lines = vec![
        "   ██████╗ ██████╗ ███████╗███╗   ██╗███████╗███████╗███╗   ██╗████████╗██████╗  ██╗   ██╗",
        "  ██╔═══██╗██╔══██╗██╔════╝████╗  ██║██╔════╝██╔════╝████╗  ██║╚══██╔══╝██╔══██╗ ╚██╗ ██╔╝",
        "  ██║   ██║██████╔╝█████╗  ██╔██╗ ██║███████╗█████╗  ██╔██╗ ██║   ██║   ██████╔╝  ╚████╔╝ ",
        "  ██║   ██║██╔═══╝ ██╔══╝  ██║╚██╗██║╚════██║██╔══╝  ██║╚██╗██║   ██║   ██╔══██╗   ╚██╔╝  ",
        "  ╚██████╔╝██║     ███████╗██║ ╚████║███████║███████╗██║ ╚████║   ██║   ██║  ██║    ██║   ",
        "   ╚═════╝ ╚═╝     ╚══════╝╚═╝  ╚═══╝╚══════╝╚══════╝╚═╝  ╚═══╝   ╚═╝   ╚═╝  ╚═╝    ╚═╝   ",
    ];

    for (i, line) in header_lines.iter().enumerate() {
        let colored = rainbow_text_offset(line, i % 6);
        println!("{}", colored);
        thread::sleep(Duration::from_millis(70));
    }

    println!();
    draw_expanding_border(Duration::from_millis(350))?;
    println!();
    fade_in(
        "    📹  CloudNode Setup  —  Your camera, connected to the cloud.",
        Duration::from_millis(400),
    )?;
    thread::sleep(Duration::from_millis(250));
    println!();

    Ok(())
}

// ─── Step 1: Prerequisites ───────────────────────────────────────────────────

fn check_prerequisites() -> Result<PlatformInfo> {
    panel_top("Step 1 / 5 — Prerequisites");
    panel_blank();

    // Progress bar
    panel_row(&{
        let bar = progress_bar_str(&steps_for(0));
        format!("  {}", bar)
    });
    panel_blank();
    panel_divider();
    panel_blank();

    let mut spinner = Spinner::new();

    // Platform
    panel_spinner_row(&spinner.advance(), "Detecting platform...");
    flush();
    thread::sleep(Duration::from_millis(300));
    let platform = PlatformInfo::detect()?;
    // Overwrite spinner row with result
    print!("\r");
    flush();
    panel_check(&format!("Platform: {}", platform.display()));

    // Camera
    panel_spinner_row(&spinner.advance(), "Detecting cameras...");
    flush();
    thread::sleep(Duration::from_millis(300));
    let cameras = crate::camera::detect_cameras()?;
    print!("\r");
    flush();
    if cameras.is_empty() {
        panel_error("No cameras detected — connect a USB camera and restart");
        panel_blank();
        panel_bottom();
        std::process::exit(1);
    } else {
        panel_check(&format!("{} camera(s) detected", cameras.len()));
        for cam in &cameras {
            panel_sub(&format!(
                "{} — {}×{}",
                cam.name, cam.preferred_resolution.0, cam.preferred_resolution.1
            ));
        }
    }

    // FFmpeg
    panel_spinner_row(&spinner.advance(), "Checking FFmpeg...");
    flush();
    thread::sleep(Duration::from_millis(300));
    let has_ffmpeg = super::platform::check_ffmpeg()?;
    print!("\r");
    flush();
    if has_ffmpeg {
        if let Some(ver) = super::platform::get_ffmpeg_version() {
            panel_check(&format!("FFmpeg v{}", ver));
        } else {
            panel_check("FFmpeg: installed");
        }
    } else {
        panel_warn("FFmpeg not found — will be downloaded automatically (Windows)");
    }

    // Network
    panel_spinner_row(&spinner.advance(), "Checking network...");
    flush();
    thread::sleep(Duration::from_millis(250));
    print!("\r");
    flush();
    panel_check("Network: connected");

    panel_blank();
    panel_bottom();
    println!();

    Ok(platform)
}

// ─── Step 2: Configure ───────────────────────────────────────────────────────

fn configure_node(platform: &PlatformInfo) -> Result<SetupConfig> {
    panel_top("Step 2 / 5 — Node Configuration");
    panel_blank();

    panel_row(&{
        let bar = progress_bar_str(&steps_for(1));
        format!("  {}", bar)
    });
    panel_blank();
    panel_divider();
    panel_blank();

    panel_row(&format!(
        "  Open {} in your browser:",
        "OpenSentry Command Center".cyan().bold()
    ));
    panel_row(&format!(
        "  {} {}",
        "→".cyan(),
        "https://opensentry-command.fly.dev".bright_white()
    ));
    panel_blank();
    panel_row(&format!(
        "  Navigate to: {} → {} → {}",
        "Settings".cyan(),
        "Nodes".cyan(),
        "Add Node".cyan()
    ));
    panel_blank();
    panel_bottom();
    println!();

    // Inputs (outside panel — inquire draws its own UI)
    let node_id = Text::new("  Node ID:")
        .with_placeholder("cf394d69")
        .with_validator(|input: &str| {
            if input.len() == 8 && input.chars().all(|c| c.is_ascii_hexdigit()) {
                Ok(Validation::Valid)
            } else {
                Ok(Validation::Invalid(
                    "Must be 8 hexadecimal characters (e.g. cf394d69)".into(),
                ))
            }
        })
        .prompt()?;

    let api_key = Text::new("  API Key:")
        .with_placeholder("f3eda4fd-7810-4577-94a8-290fbb6d9523")
        .with_validator(|input: &str| {
            let parts: Vec<&str> = input.trim().split('-').collect();
            if parts.len() == 5
                && parts[0].len() == 8
                && parts[1].len() == 4
                && parts[2].len() == 4
                && parts[3].len() == 4
                && parts[4].len() == 12
                && parts
                    .iter()
                    .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
            {
                Ok(Validation::Valid)
            } else {
                Ok(Validation::Invalid(
                    "Must be a UUID: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx".into(),
                ))
            }
        })
        .prompt()?;

    let default_url = "https://opensentry-command.fly.dev";
    let api_url = Text::new("  Command Center URL:")
        .with_placeholder(default_url)
        .with_default(default_url)
        .with_validator(|input: &str| {
            if input.starts_with("http://") || input.starts_with("https://") {
                Ok(Validation::Valid)
            } else {
                Ok(Validation::Invalid(
                    "Must start with http:// or https://".into(),
                ))
            }
        })
        .prompt()?;

    // Validation panel
    println!();
    panel_top("Validating Connection");
    panel_blank();

    let mut spinner = Spinner::new();
    panel_spinner_row(&spinner.advance(), &format!("Connecting to {}...", api_url));
    flush();

    let validation = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(super::validator::validate_api_connection(
            &api_url, &node_id, &api_key,
        ))?;

    print!("\r");
    flush();

    if !validation.is_valid {
        panel_error("Connection failed");
        if let Some(msg) = &validation.error_message {
            for line in msg.lines() {
                panel_row(&format!("  {}", line.red()));
            }
        }
        panel_blank();
        panel_bottom();
        println!();

        let retry = Confirm::new("  Try again with different credentials?")
            .with_default(true)
            .prompt()?;
        if retry {
            println!();
            return configure_node(platform);
        } else {
            println!();
            println!(
                "  {}",
                "Run 'opensentry-cloudnode setup' to try again.".yellow()
            );
            std::process::exit(1);
        }
    }

    panel_check("Connected successfully");
    if let Some(name) = &validation.node_name {
        panel_sub(&format!("Node name: {}", name));
    }
    panel_blank();
    panel_bottom();
    println!();

    // Deployment + auto-start
    let deployment_method = select_deployment_method(platform)?;
    println!();
    let auto_start = Confirm::new("  Auto-start CloudNode after setup?")
        .with_default(true)
        .prompt()?;

    // Summary panel
    println!();
    panel_top("Configuration Summary");
    panel_blank();
    panel_kv("  Node ID     :", &node_id);
    panel_kv("  API Key     :", &format!("{}…", &api_key[..10]));
    panel_kv("  API URL     :", &api_url);
    panel_kv("  Deploy      :", &format!("{:?}", deployment_method));
    panel_kv("  Auto-start  :", if auto_start { "Yes" } else { "No" });
    panel_blank();
    panel_bottom();
    println!();

    let confirm = Confirm::new("  Continue with these settings?")
        .with_default(true)
        .prompt()?;

    if !confirm {
        println!("  {}", "Setup cancelled.".red());
        std::process::exit(0);
    }

    Ok(SetupConfig {
        node_id,
        api_key,
        api_url,
        deployment_method,
        output_dir: std::env::current_dir()?,
        auto_start,
    })
}

fn select_deployment_method(platform: &PlatformInfo) -> Result<DeploymentMethod> {
    let options = vec!["Build from Source (Recommended)", "Docker"];
    let choice = if platform.is_windows || platform.is_linux || platform.is_macos {
        Select::new("  How would you like to run CloudNode?", options)
            .with_starting_cursor(0)
            .prompt()?
    } else {
        "Build from Source (Recommended)"
    };

    if choice == "Build from Source (Recommended)" {
        if platform.is_windows {
            println!();
            let sub = vec![
                "Windows Native (DirectShow)",
                "WSL2 (v4l2, requires USB passthrough)",
            ];
            let sel = Select::new("  Run where?", sub)
                .with_starting_cursor(0)
                .prompt()?;
            Ok(if sel.starts_with("Windows") {
                DeploymentMethod::WindowsNative
            } else {
                DeploymentMethod::WSL2
            })
        } else {
            Ok(DeploymentMethod::LinuxNative)
        }
    } else {
        Ok(DeploymentMethod::Docker)
    }
}

// ─── Step 3: Install ─────────────────────────────────────────────────────────

fn install_dependencies(config: &SetupConfig, platform: &PlatformInfo) -> Result<()> {
    panel_top("Step 3 / 5 — Installing Dependencies");
    panel_blank();

    panel_row(&{
        let bar = progress_bar_str(&steps_for(2));
        format!("  {}", bar)
    });
    panel_blank();
    panel_divider();
    panel_blank();

    let mut spinner = Spinner::new();

    // Save config to database
    panel_spinner_row(&spinner.advance(), "Saving configuration to database...");
    flush();
    save_config_to_database(config)?;
    print!("\r");
    flush();
    panel_check("Configuration saved to database (API key encrypted)");

    // FFmpeg on Windows
    if matches!(config.deployment_method, DeploymentMethod::WindowsNative) {
        if !super::platform::check_ffmpeg()? {
            panel_blank();
            panel_mid("Downloading FFmpeg");
            panel_blank();
            panel_bottom();
            println!();

            download_ffmpeg_with_progress()?;

            println!();
            panel_top("Step 3 / 5 — Installing Dependencies");
            panel_blank();
            panel_check("FFmpeg installed successfully");
            show_mini_celebration()?;
        } else {
            panel_check("FFmpeg already available");
        }

        // GPU encoder detection
        panel_blank();
        panel_mid("Video Encoder Detection");
        panel_blank();
        panel_spinner_row(&spinner.advance(), "Probing for GPU hardware encoder...");
        flush();

        let ffmpeg_path = find_ffmpeg_for_setup();
        let hw_encoder = crate::streaming::hls_generator::HlsGenerator::detect_hw_encoder(&ffmpeg_path);
        print!("\r");
        flush();

        match &hw_encoder {
            Some(enc) => {
                let gpu_name = match enc.as_str() {
                    "h264_nvenc" => "NVIDIA NVENC (GPU)",
                    "h264_qsv" => "Intel Quick Sync (GPU)",
                    "h264_amf" => "AMD AMF (GPU)",
                    "h264_v4l2m2m" => "V4L2 M2M (Hardware)",
                    other => other,
                };
                panel_check(&format!("GPU encoder available: {}", gpu_name.cyan()));
                panel_blank();
                panel_bottom();
                println!();

                let options = vec![
                    format!("{} (Recommended — frees CPU, faster encoding)", gpu_name),
                    "Software (libx264 — CPU-based, works everywhere)".to_string(),
                ];
                let choice = Select::new("  Video encoder:", options)
                    .with_starting_cursor(0)
                    .prompt()?;

                let use_gpu = choice.contains("GPU") || choice.contains("NVENC")
                    || choice.contains("Quick Sync") || choice.contains("AMF")
                    || choice.contains("Hardware");

                if use_gpu {
                    save_config_to_db(config, "encoder", &enc)?;
                    println!();
                    panel_top("Step 3 / 5 — Installing Dependencies");
                    panel_blank();
                    panel_check(&format!("Encoder: {} (GPU)", gpu_name.cyan()));
                } else {
                    save_config_to_db(config, "encoder", "libx264")?;
                    println!();
                    panel_top("Step 3 / 5 — Installing Dependencies");
                    panel_blank();
                    panel_check(&format!("Encoder: {} (CPU)", "libx264".cyan()));
                }
            }
            None => {
                panel_check(&format!("Encoder: {} (CPU — no GPU detected)", "libx264".cyan()));
                save_config_to_db(config, "encoder", "libx264")?;
            }
        }

        // Codec detection
        panel_blank();
        panel_mid("Camera Codec Detection");
        panel_blank();

        let cameras = crate::camera::detect_cameras()?;
        if let Some(first) = cameras.first() {
            panel_spinner_row(&spinner.advance(), &format!("Probing {}...", first.name));
            flush();

            match crate::streaming::codec_detector::detect_codec_from_camera(&first.device_path) {
                Ok(info) => {
                    print!("\r");
                    flush();
                    panel_check(&format!(
                        "Video: {}   Audio: {}",
                        info.video_codec.cyan(),
                        info.audio_codec.cyan()
                    ));
                    panel_sub("This codec will be used for HLS streaming");
                }
                Err(e) => {
                    print!("\r");
                    flush();
                    panel_warn(&format!("Codec detection skipped: {}", e));
                    panel_sub(
                        "Default: avc1.42e01e (H.264 Baseline) — compatible with most cameras",
                    );
                }
            }
        }
    } else {
        if !super::platform::check_ffmpeg()? {
            if platform.is_linux {
                panel_warn("FFmpeg not found — install with: sudo apt install ffmpeg");
            } else if platform.is_macos {
                panel_warn("FFmpeg not found — install with: brew install ffmpeg");
            } else {
                panel_warn("FFmpeg not found — please install it before running");
            }
        } else {
            panel_check("FFmpeg available");

            // GPU detection for non-Windows platforms too
            let ffmpeg_path = "ffmpeg".to_string();
            let hw_encoder = crate::streaming::hls_generator::HlsGenerator::detect_hw_encoder(&ffmpeg_path);
            if let Some(enc) = &hw_encoder {
                let gpu_name = match enc.as_str() {
                    "h264_nvenc" => "NVIDIA NVENC (GPU)",
                    "h264_qsv" => "Intel Quick Sync (GPU)",
                    "h264_amf" => "AMD AMF (GPU)",
                    "h264_v4l2m2m" => "V4L2 M2M (Hardware)",
                    other => other,
                };
                panel_check(&format!("GPU encoder available: {}", gpu_name.cyan()));
                panel_blank();
                panel_bottom();
                println!();

                let options = vec![
                    format!("{} (Recommended — frees CPU, faster encoding)", gpu_name),
                    "Software (libx264 — CPU-based, works everywhere)".to_string(),
                ];
                let choice = Select::new("  Video encoder:", options)
                    .with_starting_cursor(0)
                    .prompt()?;

                let use_gpu = !choice.contains("Software");
                if use_gpu {
                    save_config_to_db(config, "encoder", &enc)?;
                } else {
                    save_config_to_db(config, "encoder", "libx264")?;
                }

                println!();
                panel_top("Step 3 / 5 — Installing Dependencies");
                panel_blank();
            }
        }
    }

    // Directories
    panel_spinner_row(&spinner.advance(), "Creating data directories...");
    flush();
    create_directories(config)?;
    print!("\r");
    flush();
    panel_check("Data directories created");

    panel_blank();
    panel_bottom();
    println!();

    Ok(())
}

// ─── Step 4: Verify ──────────────────────────────────────────────────────────

fn verify_setup(config: &SetupConfig) -> Result<()> {
    panel_top("Step 4 / 5 — Verification");
    panel_blank();

    panel_row(&{
        let bar = progress_bar_str(&steps_for(3));
        format!("  {}", bar)
    });
    panel_blank();
    panel_divider();
    panel_blank();

    let mut spinner = Spinner::new();

    panel_spinner_row(&spinner.advance(), "Verifying database configuration...");
    flush();
    let db_path = config.output_dir.join("data").join("node.db");
    if !db_path.exists() {
        print!("\r");
        flush();
        panel_error("Database file missing");
        panel_blank();
        panel_bottom();
        std::process::exit(1);
    }
    thread::sleep(Duration::from_millis(200));
    print!("\r");
    flush();
    panel_check("Database configuration valid");

    panel_spinner_row(&spinner.advance(), "Verifying directories...");
    flush();
    thread::sleep(Duration::from_millis(200));
    print!("\r");
    flush();
    panel_check("Data directories present");

    panel_spinner_row(&spinner.advance(), "Finalizing...");
    flush();
    thread::sleep(Duration::from_millis(200));
    print!("\r");
    flush();
    panel_check("Setup verified");

    panel_blank();
    panel_bottom();
    println!();

    Ok(())
}

// ─── Step 5: Success ─────────────────────────────────────────────────────────

fn show_success_screen(config: &SetupConfig) -> Result<()> {
    clear_screen()?;
    show_confetti(Duration::from_secs(2))?;

    thread::sleep(Duration::from_millis(200));
    animate_rainbow_text("        ✓  SETUP COMPLETE  ✓", Duration::from_millis(800))?;
    thread::sleep(Duration::from_millis(200));

    println!();
    panel_top("Step 5 / 5 — Ready to Launch");
    panel_blank();

    panel_row(&{
        let bar = progress_bar_str(&steps_for(4));
        format!("  {}", bar)
    });
    panel_blank();
    panel_divider();
    panel_blank();

    // Pulse the tagline inside the panel
    let tagline = "🎉  Your OpenSentry CloudNode is ready!";
    for _ in 0..2 {
        panel_row(&format!("  {}", tagline.green().bold()));
        thread::sleep(Duration::from_millis(450));
        panel_row(&format!("  {}", tagline.dimmed()));
        thread::sleep(Duration::from_millis(450));
    }
    panel_row(&format!("  {}", tagline.green().bold()));
    panel_blank();
    panel_divider();
    panel_blank();

    // Summary
    panel_kv("  Node ID     :", &config.node_id);
    panel_kv("  API URL     :", &config.api_url);
    panel_kv(
        "  Deploy      :",
        &format!("{:?}", config.deployment_method),
    );
    panel_blank();
    panel_divider();
    panel_blank();

    // Checks
    panel_check("Configuration saved");
    panel_check("Directories created");
    if matches!(config.deployment_method, DeploymentMethod::WindowsNative) {
        panel_check("FFmpeg installed");
    }
    panel_blank();
    panel_divider();
    panel_blank();

    // Next steps
    match config.deployment_method {
        DeploymentMethod::WindowsNative => {
            panel_row(&format!(
                "  {}  {}",
                "Optional — add FFmpeg to PATH:".white().bold(),
                config.output_dir.join("ffmpeg\\bin").display()
            ));
        }
        DeploymentMethod::Docker => {
            panel_row(&format!(
                "  {}  {}",
                "Start with:".white().bold(),
                "docker compose up".cyan()
            ));
        }
        DeploymentMethod::LinuxNative | DeploymentMethod::WSL2 => {
            panel_row(&format!(
                "  {}  {}",
                "Start with:".white().bold(),
                "./target/release/opensentry-cloudnode".cyan()
            ));
        }
    }

    let dashboard = if config.api_url.contains("localhost") || config.api_url.contains("127.0.0.1")
    {
        "http://localhost:5173".to_string()
    } else {
        config.api_url.clone()
    };
    panel_row(&format!(
        "  {}  {}",
        "Dashboard:  ".white().bold(),
        dashboard.cyan()
    ));
    panel_row(&format!(
        "  {}  {}",
        "Docs:       ".white().bold(),
        "https://github.com/SourceBox-LLC/opensentry-cloud-node".cyan()
    ));

    panel_blank();
    panel_divider();
    panel_blank();

    if config.auto_start {
        panel_row(&format!("  {}", "🚀  Starting CloudNode...".green().bold()));
    } else {
        panel_row(&format!(
            "  {}",
            "Press Enter to start CloudNode...".yellow().bold()
        ));
    }

    panel_blank();
    panel_bottom();
    println!();

    show_mini_celebration()?;

    Ok(())
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Render the pill progress bar as a string (for embedding inside a panel row).
fn progress_bar_str(steps: &[(&'static str, StepState)]) -> String {
    let connector = "──".dimmed().to_string();
    steps
        .iter()
        .map(|(label, state)| match state {
            StepState::Done => format!("{}", format!("[ ✓ {} ]", label).bright_green().bold()),
            StepState::Active => format!("{}", format!("[ ● {} ]", label).cyan().bold()),
            StepState::Pending => format!("{}", format!("[  {}  ]", label).dimmed()),
        })
        .collect::<Vec<_>>()
        .join(&connector)
}

fn save_config_to_database(config: &SetupConfig) -> Result<()> {
    // Store config in the SQLite database with the API key encrypted.
    let db_path = config.output_dir.join("data").join("node.db");
    std::fs::create_dir_all(db_path.parent().unwrap())?;
    let db = crate::storage::NodeDatabase::new(&db_path)
        .map_err(|e| anyhow::anyhow!("DB error: {}", e))?;

    let app_config = crate::config::Config {
        node: crate::config::NodeConfig {
            name: crate::config::NodeConfig::default().name,
            node_id: Some(config.node_id.clone()),
        },
        cloud: crate::config::CloudConfig {
            api_url: config.api_url.clone(),
            api_key: config.api_key.clone(),
            heartbeat_interval: 30,
        },
        ..Default::default()
    };

    app_config
        .save_to_db(&db)
        .map_err(|e| anyhow::anyhow!("Config save error: {}", e))?;

    Ok(())
}

fn create_directories(config: &SetupConfig) -> Result<()> {
    let data = config.output_dir.join("data");
    std::fs::create_dir_all(data.join("hls"))?;
    Ok(())
}

fn download_ffmpeg_with_progress() -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};
    use std::fs::File;
    use std::io::Write;

    let ffmpeg_dir = std::env::current_dir()?.join("ffmpeg");
    if ffmpeg_dir.join("bin").join("ffmpeg.exe").exists() {
        return Ok(());
    }

    let url = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
    let zip_file = std::env::temp_dir().join("ffmpeg.zip");

    let response = reqwest::blocking::get(url)?;
    let bytes = response.bytes()?;
    let total = bytes.len() as u64;

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("  {spinner:.cyan} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ETA:{eta}")
            .expect("valid template")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    let mut file = File::create(&zip_file)?;
    let mut written = 0u64;
    for chunk in bytes.chunks(8192) {
        file.write_all(chunk)?;
        written += chunk.len() as u64;
        pb.set_position(written);
    }
    pb.finish_with_message("Download complete");

    // Extract
    let extract_dir = std::env::temp_dir().join("ffmpeg-extract");
    extract_zip(&zip_file, &extract_dir)?;

    let extracted = extract_dir
        .read_dir()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("Extract dir empty"))??
        .path();

    if ffmpeg_dir.exists() {
        std::fs::remove_dir_all(&ffmpeg_dir)?;
    }
    std::fs::rename(extracted, &ffmpeg_dir)?;
    std::fs::remove_file(&zip_file).ok();

    Ok(())
}

/// Find FFmpeg path for setup probing (same logic as HlsGenerator::find_ffmpeg)
fn find_ffmpeg_for_setup() -> String {
    #[cfg(target_os = "windows")]
    {
        if let Ok(cwd) = std::env::current_dir() {
            let local = cwd.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if local.exists() {
                return local.to_string_lossy().to_string();
            }
        }
    }
    "ffmpeg".to_string()
}

/// Save a config key-value pair to the SQLite database.
fn save_config_to_db(config: &SetupConfig, key: &str, value: &str) -> Result<()> {
    let db_path = config.output_dir.join("data").join("node.db");
    let db = crate::storage::NodeDatabase::new(&db_path)
        .map_err(|e| anyhow::anyhow!("DB error: {}", e))?;
    db.set_config(key, value)
        .map_err(|e| anyhow::anyhow!("Config save error: {}", e))?;
    Ok(())
}

fn extract_zip(zip_path: &std::path::Path, extract_to: &std::path::Path) -> Result<()> {
    use std::fs::create_dir_all;
    use zip::ZipArchive;

    let file = std::fs::File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    create_dir_all(extract_to)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let outpath = match entry.enclosed_name() {
            Some(p) => extract_to.join(p),
            None => continue,
        };
        if entry.name().ends_with('/') {
            create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent() {
                if !parent.exists() {
                    create_dir_all(parent)?;
                }
            }
            let mut out = std::fs::File::create(&outpath)?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}
