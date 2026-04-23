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

//! SourceBox Sentry CloudNode - Camera streaming node for SourceBox Sentry Cloud
//!
//! This node runs on a local device (Raspberry Pi, Mini PC, etc.) and:
//! - Detects USB cameras
//! - Captures video frames
//! - Streams to SourceBox Sentry Command Center
//! - Stores recordings locally
//! - Serves recordings via HTTP

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use colored::Colorize;
use opensentry_cloudnode::{Config, Node, Result};
use opensentry_cloudnode::logging::DashboardLayer;
use tracing::{info, Level};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(name = "opensentry-cloudnode")]
#[command(version)]
#[command(about = "SourceBox Sentry camera node - stream cameras to SourceBox Sentry Cloud")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
    
    /// Path to config file (YAML)
    #[arg(short, long, env = "OPENSENTRY_CONFIG")]
    config: Option<String>,

    /// Node ID (required for registration)
    #[arg(long, env = "OPENSENTRY_NODE_ID")]
    node_id: Option<String>,

    /// Organization API key (overrides config)
    #[arg(long, env = "OPENSENTRY_API_KEY")]
    api_key: Option<String>,

    /// Command Center URL (overrides config)
    #[arg(long, env = "OPENSENTRY_API_URL")]
    api_url: Option<String>,

    /// Run once and exit (for testing)
    #[arg(long)]
    once: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Start CloudNode (default)
    Run {
        /// Node ID (required for registration)
        #[arg(long, env = "OPENSENTRY_NODE_ID")]
        node_id: Option<String>,
        
        /// Organization API key (overrides config)
        #[arg(long, env = "OPENSENTRY_API_KEY")]
        api_key: Option<String>,
        
        /// Command Center URL (overrides config)
        #[arg(long, env = "OPENSENTRY_API_URL")]
        api_url: Option<String>,

        /// Run once and exit (for testing)
        #[arg(long)]
        once: bool,
    },
    
    /// Setup CloudNode (interactive wizard, or one-command with --url/--node-id/--key)
    Setup {
        /// Command Center URL
        #[arg(long, env = "OPENSENTRY_API_URL")]
        url: Option<String>,

        /// Node ID from Command Center
        #[arg(long, env = "OPENSENTRY_NODE_ID")]
        node_id: Option<String>,

        /// API key from Command Center
        #[arg(long, env = "OPENSENTRY_API_KEY")]
        key: Option<String>,
    },
    
    /// Uninstall CloudNode
    Uninstall {
        /// Force uninstall without confirmation
        #[arg(long)]
        force: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        // Already-reported errors came through a formatted TUI path
        // (e.g. show_registration_error); do not print a second debug line.
        Err(opensentry_cloudnode::Error::AlreadyReported)
        | Err(opensentry_cloudnode::Error::ResetRequested) => ExitCode::from(1),
        Err(e) => {
            eprintln!();
            eprintln!("  {} {}", "Error:".red().bold(), e);
            eprintln!();
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    // On Windows, check if we have a proper terminal attached
    // If not (double-clicked from Explorer), launch in a new terminal window
    #[cfg(target_os = "windows")]
    {
        if !has_terminal() {
            // Try to launch in a new terminal window
            if let Ok(true) = launch_in_terminal() {
                // Successfully launched in new terminal - exit cleanly
                // The new window will run the setup
                return Ok(());
            }
            // If launching failed, we'll continue in this process
            // but show a clear message first
            show_terminal_required_message();
            // Pause so user sees the message
            pause_on_exit();
        }
    }

    // Load .env file if it exists (legacy — config now stored in data/node.db)
    dotenvy::dotenv().ok();

    let args = Args::parse();

    // Determine if we're launching setup or running the node.
    // IMPORTANT: Do NOT initialize the tracing subscriber until AFTER setup
    // completes — tracing logs would bleed into the TUI and destroy the layout.
    let needs_setup = match &args.command {
        Some(Commands::Setup { .. }) => true,
        Some(Commands::Run { .. }) | None => {
            !Config::load(args.config.as_deref())
                .map(|c| !c.cloud.api_key.is_empty() && c.node.node_id.is_some())
                .unwrap_or(false)
        }
        _ => false,
    };

    if needs_setup {
        // Check if this is a quick (non-interactive) setup with all args provided
        let quick_args = match &args.command {
            Some(Commands::Setup { url, node_id, key }) => {
                match (url.as_deref(), node_id.as_deref(), key.as_deref()) {
                    (Some(u), Some(n), Some(k)) => Some((u.to_string(), n.to_string(), k.to_string())),
                    // Partial args — tell the user what's missing
                    _ if url.is_some() || node_id.is_some() || key.is_some() => {
                        eprintln!("Error: Quick setup requires all three flags: --url, --node-id, and --key");
                        eprintln!("  Example: opensentry-cloudnode setup --url https://... --node-id abc12345 --key xxxxxxxx-...");
                        std::process::exit(1);
                    }
                    _ => None,
                }
            }
            _ => None,
        };

        if let Some((url, node_id, key)) = quick_args {
            // Non-interactive quick setup
            init_logging(&args.log_level);
            opensentry_cloudnode::setup::run_quick_setup(&url, &node_id, &key)?;
            return run_cloudnode(None, None, None, args.once, args.config);
        }

        // Interactive TUI setup with logging completely suppressed.
        let auto_start = opensentry_cloudnode::setup::run_setup()?;
        if !auto_start {
            println!("\n  Press Enter to start CloudNode...");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
        }
        // Now safe to init logging before running the node.
        init_logging(&args.log_level);
        return run_cloudnode(args.node_id, args.api_key, args.api_url, args.once, args.config);
    }

    // No setup needed — init logging and handle remaining subcommands.
    init_logging(&args.log_level);

    match args.command {
        Some(Commands::Run { node_id, api_key, api_url, once }) => {
            run_cloudnode(node_id.or(args.node_id), api_key.or(args.api_key), api_url.or(args.api_url), once, args.config)?;
        }
        Some(Commands::Uninstall { force }) => {
            uninstall_cloudnode(force)?;
        }
        Some(Commands::Setup { .. }) => {
            // Already handled above via needs_setup path.
        }
        None => {
            run_cloudnode(args.node_id, args.api_key, args.api_url, args.once, args.config)?;
        }
    }

    Ok(())
}

fn run_cloudnode(
    node_id: Option<String>,
    api_key: Option<String>,
    api_url: Option<String>,
    once: bool,
    config_path: Option<String>,
) -> Result<()> {
    info!("Starting SourceBox Sentry CloudNode v{}", env!("CARGO_PKG_VERSION"));

    // Retry loop: if the user confirms a credential reset after a registration
    // failure, the node returns `Error::ResetRequested`. We then re-run the
    // setup wizard and loop to re-attempt with the fresh credentials. Ok /
    // other errors exit immediately.
    loop {
        match run_cloudnode_once(
            node_id.clone(),
            api_key.clone(),
            api_url.clone(),
            once,
            config_path.clone(),
        ) {
            Err(opensentry_cloudnode::Error::ResetRequested) => {
                // Unhook the previous node's dashboard from the tracing layer
                // so the setup wizard's events don't flow to an orphaned TUI.
                opensentry_cloudnode::logging::clear_dashboard();
                // Relaunch the interactive setup wizard synchronously. On
                // success it writes fresh credentials to data/node.db, which
                // the next loop iteration picks up via Config::load.
                opensentry_cloudnode::setup::run_setup()?;
                continue;
            }
            other => return other,
        }
    }
}

fn run_cloudnode_once(
    node_id: Option<String>,
    api_key: Option<String>,
    api_url: Option<String>,
    once: bool,
    config_path: Option<String>,
) -> Result<()> {
    // Load configuration
    let config = Config::load(config_path.as_deref())?;

    // Apply CLI overrides
    let config = config.with_overrides(opensentry_cloudnode::config::CliOverrides {
        node_id,
        api_key,
        api_url,
    });

    // Validate configuration
    if config.cloud.api_key.is_empty() {
        return Err(opensentry_cloudnode::Error::Config(
            "API key required. Set OPENSENTRY_API_KEY env var or use --api-key flag".to_string()
        ));
    }

    if config.node.node_id.is_none() {
        return Err(opensentry_cloudnode::Error::Config(
            "Node ID required. Set OPENSENTRY_NODE_ID env var or use --node-id flag".to_string()
        ));
    }

    info!("Node name: {}", config.node.name);
    info!("API URL: {}", config.cloud.api_url);

    // Create and run node on its own tokio runtime. If this returns
    // ResetRequested, the runtime drops here and the outer loop re-enters
    // with a fresh one.
    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        let node = Node::new(config).await?;

        if once {
            node.run_once().await?;
        } else {
            node.run().await?;
        }

        Ok(())
    })
}

fn init_logging(log_level: &str) {
    let level = match log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "warn"  => Level::WARN,
        "error" => Level::ERROR,
        _       => Level::INFO,
    };

    // Route tracing events through DashboardLayer → TUI + SQLite.
    // Before a Dashboard is installed via logging::set_dashboard(), events
    // are silently discarded (startup messages use println! directly).
    let filter = tracing_subscriber::filter::LevelFilter::from_level(level);

    tracing_subscriber::registry()
        .with(filter)
        .with(DashboardLayer)
        .init();
}

fn uninstall_cloudnode(force: bool) -> Result<()> {
    use colored::Colorize;
    
    println!("{}", "╔════════════════════════════════════════════════════╗".red());
    println!("{}", "║          SourceBox Sentry CloudNode Uninstall           ║".red());
    println!("{}", "╚════════════════════════════════════════════════════╝".red());
    println!();
    
    // Check for files to remove
    let env_path = std::env::current_dir()?.join(".env");
    let data_dir = std::env::current_dir()?.join("data");
    let ffmpeg_dir = std::env::current_dir()?.join("ffmpeg");

    println!("  The following files will be removed:");
    if env_path.exists() {
        println!("    - {} (legacy)", env_path.display());
    }
    if data_dir.exists() {
        println!("    - {}", data_dir.display());
    }
    if ffmpeg_dir.exists() {
        println!("    - {}", ffmpeg_dir.display());
    }
    println!();
    
    if !force {
        use inquire::Confirm;
        
        let confirm = Confirm::new("Continue with uninstall?")
            .with_default(false)
            .prompt()
            .map_err(|e| anyhow::anyhow!("Prompt error: {}", e))?;
        
        if !confirm {
            println!("  Uninstall cancelled.");
            return Ok(());
        }
    }
    
    println!("  Removing files...");
    
    if env_path.exists() {
        std::fs::remove_file(&env_path)?;
        println!("  {} Removed legacy .env file", "✓".green());
    }
    
    if data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)?;
        println!("  {} Removed data directory", "✓".green());
    }
    
    if ffmpeg_dir.exists() {
        std::fs::remove_dir_all(&ffmpeg_dir)?;
        println!("  {} Removed FFmpeg directory", "✓".green());
    }
    
    println!();
    println!("  {}", "Uninstall complete.".green());
    println!("  To reinstall:");
    println!("    {} opensentry-cloudnode setup", "→".cyan());
    
    Ok(())
}

/// Check if we have a proper terminal attached
#[cfg(target_os = "windows")]
fn has_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Launch CloudNode in a new terminal window on Windows
/// Returns Ok(true) if successfully launched, Ok(false) if failed
#[cfg(target_os = "windows")]
fn launch_in_terminal() -> std::result::Result<bool, anyhow::Error> {
    use std::process::Command;
    
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to get executable path: {}", e))?;
    
    // Launch in a new cmd window with /K to keep it open after completion
    let result = Command::new("cmd")
        .args(["/C", "start", "cmd", "/K"])
        .arg(&exe)
        .current_dir(std::env::current_dir().unwrap_or_default())
        .spawn();

    match result {
        Ok(_) => Ok(true),
        Err(e) => {
            tracing::warn!("Failed to open terminal: {}", e);
            Ok(false)
        }
    }
}

/// Show a Windows message if terminal cannot be opened
#[cfg(target_os = "windows")]
fn show_terminal_required_message() {
    eprintln!();
    eprintln!("  SourceBox Sentry CloudNode");
    eprintln!("  ────────────────────────────────────────");
    eprintln!();
    eprintln!("  This application requires a terminal window.");
    eprintln!();
    eprintln!("  Opening in a new terminal window...");
    eprintln!();
}

/// Pause before exit on Windows to prevent window from closing immediately
#[cfg(target_os = "windows")]
fn pause_on_exit() {
    use std::io::{self, BufRead};
    
    // Give the new window time to start if we spawned one
    std::thread::sleep(std::time::Duration::from_secs(2));
    
    eprintln!();
    eprintln!("  Press Enter to continue...");
    let _ = io::stdin().lock().read_line(&mut String::new());
}