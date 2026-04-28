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
use sourcebox_sentry_cloudnode::{Config, Node, Result};
use sourcebox_sentry_cloudnode::logging::DashboardLayer;
use tracing::{info, Level};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(name = "sourcebox-sentry-cloudnode")]
#[command(version)]
#[command(about = "SourceBox Sentry camera node - stream cameras to SourceBox Sentry Cloud")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
    
    /// Path to config file (YAML)
    #[arg(short, long, env = "SOURCEBOX_SENTRY_CONFIG")]
    config: Option<String>,

    /// Node ID (required for registration)
    #[arg(long, env = "SOURCEBOX_SENTRY_NODE_ID")]
    node_id: Option<String>,

    /// Organization API key (overrides config)
    #[arg(long, env = "SOURCEBOX_SENTRY_API_KEY")]
    api_key: Option<String>,

    /// Command Center URL (overrides config)
    #[arg(long, env = "SOURCEBOX_SENTRY_API_URL")]
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
        #[arg(long, env = "SOURCEBOX_SENTRY_NODE_ID")]
        node_id: Option<String>,
        
        /// Organization API key (overrides config)
        #[arg(long, env = "SOURCEBOX_SENTRY_API_KEY")]
        api_key: Option<String>,
        
        /// Command Center URL (overrides config)
        #[arg(long, env = "SOURCEBOX_SENTRY_API_URL")]
        api_url: Option<String>,

        /// Run once and exit (for testing)
        #[arg(long)]
        once: bool,
    },
    
    /// Setup CloudNode (interactive wizard, or one-command with --url/--node-id/--key)
    Setup {
        /// Command Center URL
        #[arg(long, env = "SOURCEBOX_SENTRY_API_URL")]
        url: Option<String>,

        /// Node ID from Command Center
        #[arg(long, env = "SOURCEBOX_SENTRY_NODE_ID")]
        node_id: Option<String>,

        /// API key from Command Center
        #[arg(long, env = "SOURCEBOX_SENTRY_API_KEY")]
        key: Option<String>,
    },
    
    /// Uninstall CloudNode
    Uninstall {
        /// Force uninstall without confirmation
        #[arg(long)]
        force: bool,
    },

    /// Run as a Windows Service. Invoked by the Service Control Manager —
    /// not intended for direct use. The MSI registers
    /// `sourcebox-sentry-cloudnode service` as the service binary path.
    /// See src/service.rs for the SCM handshake details.
    #[command(hide = true)]
    Service,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        // Already-reported errors came through a formatted TUI path
        // (e.g. show_registration_error); do not print a second debug line.
        Err(sourcebox_sentry_cloudnode::Error::AlreadyReported)
        | Err(sourcebox_sentry_cloudnode::Error::ResetRequested) => ExitCode::from(1),
        Err(e) => {
            eprintln!();
            eprintln!("  {} {}", "Error:".red().bold(), e);
            eprintln!();
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    // Load .env file if it exists (legacy — config now stored in data/node.db).
    // Parsing args before the terminal-check is intentional: the `service`
    // subcommand must short-circuit BEFORE `launch_in_terminal()` would
    // try to spawn a console (the SCM has no console for us to attach to,
    // and any spawned cmd window gets orphaned anyway).
    dotenvy::dotenv().ok();
    let args = Args::parse();

    // ── Windows Service short-circuit ────────────────────────────────
    // SCM invokes us as `sourcebox-sentry-cloudnode.exe service`. From here we
    // hand off to the windows-service dispatcher which blocks until the
    // service exits. Don't reach the terminal-check or interactive flow.
    //
    // Install a panic hook + dispatcher-error capture BEFORE doing anything
    // else: if the dispatcher itself fails, or anything in the service
    // body panics across the FFI boundary, the only error-reporting paths
    // are stderr (no console) and SCM's "Win32 exit code 1" (useless).
    // Writing to fatal-startup-error.txt before that happens means an
    // operator can actually diagnose what went wrong.
    #[cfg(target_os = "windows")]
    if matches!(args.command, Some(Commands::Service)) {
        // Best-effort write to ProgramData fatal-startup-error.txt with
        // the same dual-fallback strategy service.rs uses, but reachable
        // even when the service-body never ran.
        fn write_service_diag(message: &str) {
            use std::io::Write;
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let line = format!(
                "[{}] sourcebox-sentry-cloudnode service path: {}\n",
                timestamp, message
            );
            let candidates = [
                std::path::PathBuf::from(r"C:\ProgramData\SourceBoxSentry")
                    .join("fatal-startup-error.txt"),
                std::env::temp_dir()
                    .join("sourcebox-sentry-cloudnode-fatal-startup-error.txt"),
            ];
            for path in &candidates {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    if f.write_all(line.as_bytes()).is_ok() {
                        return;
                    }
                }
            }
        }

        write_service_diag(&format!(
            "service subcommand reached main.rs (v{})",
            env!("CARGO_PKG_VERSION")
        ));

        // Panic hook: rust panics across the FFI boundary inside
        // service_dispatcher are UB and can abort the process before
        // service_main's own error handler ever runs. Catch panics
        // here and dump them to disk so the operator has SOMETHING.
        std::panic::set_hook(Box::new(|info| {
            let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            let location = info
                .location()
                .map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_default();
            write_service_diag(&format!("PANIC: {}{}", msg, location));
        }));

        match sourcebox_sentry_cloudnode::service::run() {
            Ok(()) => {
                write_service_diag("service::run returned Ok — clean shutdown");
                return Ok(());
            }
            Err(e) => {
                let msg = format!("service::run returned Err: {}", e);
                write_service_diag(&msg);
                return Err(sourcebox_sentry_cloudnode::Error::Unknown(msg));
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    if matches!(args.command, Some(Commands::Service)) {
        return Err(sourcebox_sentry_cloudnode::Error::Config(
            "The `service` subcommand is Windows-only — \
             use systemd / launchd / your platform's service manager \
             to run CloudNode as a daemon on Linux/macOS."
                .to_string(),
        ));
    }

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
                        eprintln!("  Example: sourcebox-sentry-cloudnode setup --url https://... --node-id abc12345 --key xxxxxxxx-...");
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
            sourcebox_sentry_cloudnode::setup::run_quick_setup(&url, &node_id, &key)?;
            // Same MSI-vs-foreground decision as the interactive path below.
            #[cfg(target_os = "windows")]
            {
                if is_msi_install() {
                    return start_msi_service_after_setup();
                }
            }
            return run_cloudnode(None, None, None, args.once, args.config);
        }

        // Interactive TUI setup with logging completely suppressed.
        let auto_start = sourcebox_sentry_cloudnode::setup::run_setup()?;

        // On Windows MSI installs, the binary the user just ran is also
        // registered as the SourceBoxSentryCloudNode Windows Service.
        // Setup wrote the config; the *service* is what actually streams
        // video to Command Center. Running this same binary in the
        // foreground from the post-setup console would:
        //   - Conflict with the service if it ever starts
        //   - Die when the user closes the console window
        //   - Not survive a reboot
        // None of those match what the operator expects after clicking
        // through an MSI installer. So for MSI installs we kick off the
        // service via sc.exe and exit cleanly — the service takes over
        // from there. For source / cargo / Linux / Docker installs the
        // foreground path below is still correct.
        #[cfg(target_os = "windows")]
        {
            if is_msi_install() {
                return start_msi_service_after_setup();
            }
        }

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
        Some(Commands::Service) => {
            // Already handled at the top of run() via the Windows
            // short-circuit. Reaching this arm means the cfg gate
            // skipped (non-Windows) — but we already returned an
            // error for that case, so this branch is unreachable.
            unreachable!("Service subcommand handled before reaching this match");
        }
        None => {
            // Bare invocation with credentials already on disk. Same
            // logic as the post-setup branch above: on an MSI install
            // the *service* is what should stream, so kick that off
            // instead of running the node in this transient console.
            #[cfg(target_os = "windows")]
            {
                if is_msi_install() {
                    return start_msi_service_after_setup();
                }
            }
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
            Err(sourcebox_sentry_cloudnode::Error::ResetRequested) => {
                // Unhook the previous node's dashboard from the tracing layer
                // so the setup wizard's events don't flow to an orphaned TUI.
                sourcebox_sentry_cloudnode::logging::clear_dashboard();
                // Relaunch the interactive setup wizard synchronously. On
                // success it writes fresh credentials to data/node.db, which
                // the next loop iteration picks up via Config::load.
                sourcebox_sentry_cloudnode::setup::run_setup()?;
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
    let config = config.with_overrides(sourcebox_sentry_cloudnode::config::CliOverrides {
        node_id,
        api_key,
        api_url,
    });

    // Validate configuration
    if config.cloud.api_key.is_empty() {
        return Err(sourcebox_sentry_cloudnode::Error::Config(
            "API key required. Set SOURCEBOX_SENTRY_API_KEY env var or use --api-key flag".to_string()
        ));
    }

    if config.node.node_id.is_none() {
        return Err(sourcebox_sentry_cloudnode::Error::Config(
            "Node ID required. Set SOURCEBOX_SENTRY_NODE_ID env var or use --node-id flag".to_string()
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

/// Detect whether the running binary was installed by the Windows MSI.
///
/// Heuristic: the MSI installs `sourcebox-sentry-cloudnode.exe` under
/// `C:\Program Files\SourceBox Sentry CloudNode\` (or the `(x86)` mirror
/// on 32-bit emulation, though we only build x86_64 today). Legacy
/// v0.1.x installs landed under `C:\Program Files\OpenSentry CloudNode\`
/// — both paths are matched below for diagnostic continuity.
///
/// Used by `uninstall_cloudnode` to redirect MSI-installed users to
/// Settings → Apps instead of running the dev-cleanup logic, which
/// would do nothing useful for an MSI install (cwd is unrelated to
/// the install path; the service stays registered; ProgramData isn't
/// touched).
#[cfg(target_os = "windows")]
fn is_msi_install() -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    // Windows path comparison is case-insensitive; normalise to lowercase
    // before substring matching.
    //
    // Match BOTH new (SourceBox Sentry CloudNode) and legacy (OpenSentry
    // CloudNode) install paths. Pre-launch we don't really have to care
    // about the legacy path, but the cost of probing both is one extra
    // string compare per setup invocation, and it preserves the
    // diagnostic for anyone who hung onto a v0.1.x test install.
    let path = exe.to_string_lossy().to_lowercase();
    path.contains(r"\program files\sourcebox sentry cloudnode\")
        || path.contains(r"\program files (x86)\sourcebox sentry cloudnode\")
        || path.contains(r"\program files\opensentry cloudnode\")
        || path.contains(r"\program files (x86)\opensentry cloudnode\")
}

#[cfg(not(target_os = "windows"))]
fn is_msi_install() -> bool {
    // No MSI on Linux/macOS — `cloudnode uninstall` always falls
    // through to the dev-cleanup path on those platforms.
    false
}

/// Kick off the Windows Service after a successful MSI-context setup.
///
/// The MSI registers `SourceBoxSentryCloudNode` as a Windows Service
/// pointing at the same exe the operator just ran setup with. After
/// setup writes credentials to `%ProgramData%\SourceBoxSentry\node.db`,
/// the service is the right place for the actual node logic to live —
/// it runs as LocalSystem (USB camera + ProgramData write access),
/// it can be set to auto-start on boot, and it doesn't die when the
/// post-install console window closes.
///
/// The previous behaviour was to fall through to `run_cloudnode` after
/// setup, which spun up a foreground node in the install console. That
/// console gets closed approximately one second after the user reads
/// the success screen, killing the node — and the actual service was
/// still in Stopped state, so the dashboard never saw a heartbeat. The
/// effect was "setup completes, dashboard says PENDING forever." This
/// function fixes that.
///
/// On non-MSI installs (cargo build, Linux, Docker) the foreground
/// `run_cloudnode` is correct — the binary IS the node, there's no
/// service to delegate to. The caller gates on `is_msi_install()`
/// before invoking this.
#[cfg(target_os = "windows")]
fn start_msi_service_after_setup() -> Result<()> {
    use colored::Colorize;
    use sourcebox_sentry_cloudnode::service::SERVICE_NAME;

    println!();
    println!(
        "  {}  Starting {} service...",
        "⚙".cyan(),
        SERVICE_NAME.cyan().bold()
    );
    println!();

    // sc.exe start <name>: tells the SCM to transition the service to
    // START_PENDING / RUNNING. Returns 0 on success, non-zero with a
    // Win32 error code in stdout/stderr on failure.
    //
    // Using sc.exe (built-in, always present) rather than the
    // windows-service crate's ServiceManager because:
    //   - sc.exe is the canonical operator-facing tool; the error
    //     messages it prints match what users will see if they later
    //     run `sc query` themselves.
    //   - The windows-service crate would add a dependency on
    //     ServiceManagerAccess + ServiceAccess flag plumbing for what
    //     amounts to one shell-out.
    let output = std::process::Command::new("sc.exe")
        .args(["start", SERVICE_NAME])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            // sc.exe start returns success the moment SCM accepts the
            // start request — i.e., the service transitioned to
            // START_PENDING. NOT when it transitioned to RUNNING. If
            // the service crashes during init, sc.exe still returns 0
            // and we'd report "✓ Service started" while the service is
            // actually dead. Fixed in v0.1.29 by polling sc query for
            // the RUNNING state before declaring success.
            //
            // Poll budget: 10 seconds total, 500ms intervals. The
            // service's `init_file_logging` + `load_and_validate_config`
            // + initial Node::new steps complete in <2s on a healthy
            // install; 10s is generous. If we don't see RUNNING by
            // then, something's wrong and we should say so.
            let running = poll_service_until_running(SERVICE_NAME, 20, 500);
            if running {
                println!("  {}  Service started and running.", "✓".green().bold());
                println!();
                println!("  Your camera should appear in the Command Center dashboard");
                println!("  within ~30 seconds. To make the service survive reboots,");
                println!("  flip it to auto-start (one time, from an admin PowerShell):");
                println!();
                println!(
                    "    {}",
                    format!(
                        "Set-Service -Name {} -StartupType Automatic",
                        SERVICE_NAME
                    )
                    .cyan()
                );
                println!();
                Ok(())
            } else {
                // sc.exe accepted the start but the service didn't
                // transition to RUNNING within the poll window. It
                // might still be starting (slow disk, AV scan), or it
                // might have crashed silently. Either way, don't lie
                // about success — point the operator at the diagnostic
                // file we now write defensively in service.rs.
                println!(
                    "  {}  sc.exe accepted the start request but the service has not",
                    "⚠".yellow().bold()
                );
                println!("    transitioned to Running within 10s.");
                println!();
                println!("  Diagnostic checklist:");
                println!();
                println!(
                    "    1. Check the service status:  {}",
                    format!("Get-Service {}", SERVICE_NAME).cyan()
                );
                println!(
                    "    2. Read the early trace file:  {}",
                    "Get-Content C:\\sourcebox-service-trace.txt".cyan()
                );
                println!(
                    "    3. Read the structured diag:  {}",
                    "Get-Content C:\\ProgramData\\SourceBoxSentry\\fatal-startup-error.txt".cyan()
                );
                println!(
                    "    4. Check the SCM event log:   {}",
                    format!(
                        "Get-WinEvent -FilterHashtable @{{LogName='System'; ProviderName='Service Control Manager'}} -MaxEvents 5 | Where-Object {{$_.Message -like '*{}*'}}",
                        SERVICE_NAME
                    )
                    .cyan()
                );
                println!();
                Ok(())
            }
        }
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            // 1056 = ERROR_SERVICE_ALREADY_RUNNING. Common when the
            // operator re-runs setup on a node that's already up — not
            // really an error, just guidance on how to pick up the new
            // config.
            let already_running =
                stdout.contains("1056") || stderr.contains("1056");

            if already_running {
                println!(
                    "  {}  Service is already running.",
                    "ℹ".cyan().bold()
                );
                println!();
                println!("  Restart it to pick up the new configuration:");
                println!();
                println!(
                    "    {}",
                    format!("Restart-Service {}", SERVICE_NAME).cyan()
                );
                println!();
            } else {
                println!(
                    "  {}  Could not start the service automatically.",
                    "⚠".yellow().bold()
                );
                println!();
                let msg = format!("{}{}", stdout.trim(), stderr.trim());
                if !msg.is_empty() {
                    println!("  sc.exe said: {}", msg.dimmed());
                    println!();
                }
                println!("  Start it manually from an admin PowerShell:");
                println!();
                println!(
                    "    {}",
                    format!("Start-Service {}", SERVICE_NAME).cyan()
                );
                println!();
            }
            Ok(())
        }
        Err(e) => {
            // sc.exe is a built-in Windows tool — failing to invoke
            // it almost always means PATH was clobbered or the user is
            // on a stripped-down Windows variant. Surface the error and
            // fall back to manual instructions.
            println!(
                "  {}  Could not invoke sc.exe: {}",
                "⚠".yellow().bold(),
                e
            );
            println!();
            println!("  Start the service manually from an admin PowerShell:");
            println!();
            println!(
                "    {}",
                format!("Start-Service {}", SERVICE_NAME).cyan()
            );
            println!();
            Ok(())
        }
    }
}

/// Poll `sc query <name>` up to `max_attempts` times with `interval_ms`
/// between checks, returning true once the service shows STATE 4
/// (RUNNING). Returns false if it stays in any other state through the
/// whole budget.
///
/// Why poll vs. blocking on a service-status notification: SCM offers
/// `NotifyServiceStatusChangeW` for asynchronous status delivery, but
/// it requires an APC pump on the calling thread which complicates the
/// otherwise-synchronous setup-completion code path. Polling sc query
/// is ~10ms per call and the post-setup window is brief enough that
/// the cost is invisible.
///
/// Why parse stdout instead of using `service_manager` from
/// windows-service: that crate's `query_status` requires SC_MANAGER
/// handles + ServiceAccess plumbing for what's a one-shot read of a
/// status enum. sc.exe is already in the binary's call path
/// (start_msi_service_after_setup uses it) and its output format is
/// stable across all supported Windows versions.
#[cfg(target_os = "windows")]
fn poll_service_until_running(
    service_name: &str,
    max_attempts: u32,
    interval_ms: u64,
) -> bool {
    for _ in 0..max_attempts {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        let output = std::process::Command::new("sc.exe")
            .args(["query", service_name])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // sc query stdout includes a line like:
            //   "        STATE              : 4  RUNNING"
            // — the leading number (1=STOPPED, 2=START_PENDING,
            // 3=STOP_PENDING, 4=RUNNING, 5=CONTINUE_PENDING,
            // 6=PAUSE_PENDING, 7=PAUSED) is what we're matching. The
            // textual name is also parseable but the numeric form is
            // less locale-sensitive.
            if stdout.contains("STATE") && stdout.contains("4  RUNNING") {
                return true;
            }
            // Detect terminal failure states early so we don't waste
            // the rest of the budget polling a service that's already
            // gone Stopped.
            if stdout.contains("1  STOPPED") {
                // Service crashed during start; no point continuing.
                return false;
            }
        }
    }
    false
}

fn uninstall_cloudnode(force: bool) -> Result<()> {
    use colored::Colorize;

    // Detect the MSI-install case first and bail with a Settings →
    // Apps pointer. The dev-cleanup logic below is meaningful for
    // `cargo build` / `cargo install` users; for an MSI-installed
    // binary it would just remove unrelated files in the user's cwd
    // (or, worst case, fail to find anything and leave the user
    // wondering whether the uninstall worked).
    if is_msi_install() {
        println!();
        println!("  This is an MSI install — uninstall via Windows Settings:");
        println!();
        println!("    Settings → Apps → Installed apps → SourceBox Sentry CloudNode → Uninstall");
        println!();
        println!("  Settings → Apps cleanly stops the service, removes the binary,");
        println!("  removes the Windows Service registration, and (on a real");
        println!("  uninstall) wipes data under C:\\ProgramData\\SourceBoxSentry\\.");
        println!();

        // Best-effort: try to open Settings → Apps directly via the
        // ms-settings: URI scheme. Saves the user from clicking through
        // the Start menu. Falls through silently if the spawn fails —
        // the printed instructions above are still useful.
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "ms-settings:appsfeatures"])
                .spawn();
        }

        return Ok(());
    }

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
    println!("    {} sourcebox-sentry-cloudnode setup", "→".cyan());
    
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