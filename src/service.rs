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

//! Windows Service entry point.
//!
//! Compiled only on Windows (see `lib.rs`). The MSI registers the
//! service to launch this binary with the `service` subcommand; on
//! that path `main.rs` calls [`run`] which performs the SCM handshake
//! and dispatches into [`service_main`].
//!
//! ## Design notes
//!
//! 1. **No TUI.** Services run with `cwd = C:\Windows\System32` and no
//!    attached terminal. We bypass the dashboard render thread (via
//!    `Node::run_headless`) and route tracing into a daily-rolling
//!    file under `%ProgramData%\OpenSentry\logs\` instead.
//!
//! 2. **Config lives outside the install dir.** The setup wizard (run
//!    by an admin from a console *before* starting the service) writes
//!    `node.db` to the path returned by [`crate::paths::config_db_path`].
//!    On a fresh MSI install with no console-side setup, the service
//!    fails on `Config::load` and exits — the user runs setup, then
//!    `Start-Service OpenSentryCloudNode`.
//!
//! 3. **Graceful shutdown is best-effort.** SCM Stop flips the shared
//!    `stop_flag` that node supervisors poll. In-flight HLS segment
//!    uploads may abort when the tokio runtime drops. The on-disk DB
//!    state is kept consistent because every DB write is synchronous
//!    on the path that issues it; only network sends in flight can lose
//!    a fragment, and Command Center handles missing segments via the
//!    next playlist push.
//!
//! 4. **Exit codes.** SCM treats any non-zero exit as a service failure
//!    and consults the recovery actions in `services.msc` (default:
//!    none). We return `0` on clean shutdown, `1` on any error so the
//!    operator can surface the failure via Event Viewer or the recovery
//!    actions configured at install time.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState,
        ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

/// Service identifier. Must match the `Name` attribute on the WiX
/// `<ServiceInstall>` element in `wix/main.wxs`. Renaming requires a
/// coordinated change in both places — and a clean uninstall on the
/// installer side, since SCM uses this string as the primary key.
pub const SERVICE_NAME: &str = "OpenSentryCloudNode";

/// One process owns the service. We don't run multiple SCM-managed
/// services from a single binary, so `OWN_PROCESS` is correct.
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

define_windows_service!(ffi_service_main, service_main);

/// Entry point invoked from `main.rs::run` when the user (or SCM) runs
/// `opensentry-cloudnode service`.
///
/// `service_dispatcher::start` blocks for the lifetime of the service:
/// it spins up SCM communication, calls into [`service_main`] via the
/// `ffi_service_main` shim, and returns when the service exits.
pub fn run() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

/// Body of the service. Called by the dispatcher in a thread that owns
/// the SCM control channel. Errors here can't be `?`-ed up to a caller
/// — the dispatcher just notes the exit — so we log to file before
/// returning.
fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service() {
        // Two-layer error reporting because the failure mode of the
        // first layer (`init_file_logging` itself failing) silently
        // dropped errors before this fallback existed:
        //
        //   1. tracing::error!  — works only if init_file_logging
        //      succeeded. If it failed, this macro has no installed
        //      subscriber and the line goes nowhere.
        //   2. eprintln!         — services have no console, so this
        //      also goes nowhere unless a debugger is attached.
        //   3. write_fatal_startup_error  — synchronous file write,
        //      no tracing dependency, guaranteed audit trail.
        //
        // Prior to layer 3, an init_file_logging failure made the
        // service silently exit with services.msc reporting "started
        // and immediately stopped" and nothing in any log to explain.
        tracing::error!("Service exited with error: {}", e);
        eprintln!("Service error: {}", e);
        write_fatal_startup_error(&format!("{}", e));
    }
}

/// Append a one-line crash report to a guaranteed-writable location.
///
/// Order tried:
///   1. `%ProgramData%\OpenSentry\fatal-startup-error.txt` — the
///      "right" place; any operator already looking at our data dir
///      will find it. Fails if the dir creation itself was the cause
///      of the original error (ACL denial on ProgramData write).
///   2. `%TEMP%\opensentry-cloudnode-fatal-startup-error.txt` — the
///      "always works" fallback; %TEMP% is writable for every account
///      including LocalSystem.
///
/// Best-effort: silently swallows file errors (an unwritable %TEMP%
/// is a system pathology beyond our scope to diagnose). Append-only:
/// the operator gets a chronological list across multiple failed
/// starts.
fn write_fatal_startup_error(message: &str) {
    use std::io::Write;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let line = format!(
        "[{}] OpenSentry CloudNode service failed to start: {}\n",
        timestamp, message
    );

    let candidates = [
        crate::paths::data_dir().join("fatal-startup-error.txt"),
        std::env::temp_dir().join("opensentry-cloudnode-fatal-startup-error.txt"),
    ];

    for path in &candidates {
        // Try to ensure the parent dir exists for the ProgramData
        // candidate. The %TEMP% candidate's parent always exists.
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            if f.write_all(line.as_bytes()).is_ok() {
                // Wrote successfully; stop iterating.
                return;
            }
        }
    }
}

/// The actual service body. Returns `Ok(())` on graceful shutdown
/// (SCM Stop), or an error if anything fails before/during running.
fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. File logging ──────────────────────────────────────────
    // Init this FIRST so any failure in the rest of startup is
    // captured. The dashboard's tracing layer is also installed later
    // (by Node::run_headless via the dashboard wiring), but it only
    // persists to SQLite — we want a plain text file an operator can
    // tail with `Get-Content -Wait`.
    let _log_guard = init_file_logging()?;

    tracing::info!(
        "Starting OpenSentry CloudNode service (version {})",
        env!("CARGO_PKG_VERSION")
    );

    // ── 2. SCM event channel ─────────────────────────────────────
    // The control handler runs on a dispatcher thread. We forward
    // Stop events to the run loop via an mpsc channel so all the
    // shutdown sequencing happens in one place.
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                tracing::info!("SCM requested {:?}", control_event);
                let _ = shutdown_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            // Pause/Continue not implemented — pausing a security camera
            // node is rarely what an operator actually wants. They can
            // Stop the service if they need it offline.
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle =
        service_control_handler::register(SERVICE_NAME, event_handler)?;

    // ── 3. Tell SCM we're starting ──────────────────────────────
    // Required before any time-consuming work; SCM uses this to
    // distinguish "still starting" from "stuck" via wait_hint.
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::from_secs(30),
        process_id: None,
    })?;

    // ── 4. Build runtime + spawn the node ───────────────────────
    let stop_flag = Arc::new(AtomicBool::new(false));

    // Bridge SCM Stop → stop_flag in a thread that watches the mpsc
    // channel. Using a thread (not a tokio task) means the SCM event
    // is delivered even if the tokio runtime is wedged.
    let stop_flag_for_bridge = stop_flag.clone();
    let _bridge = std::thread::spawn(move || {
        if shutdown_rx.recv().is_ok() {
            tracing::info!("Shutdown signal forwarded to node");
            stop_flag_for_bridge.store(true, Ordering::Relaxed);
        }
    });

    // ── 4b. Validate config BEFORE we tell SCM we're Running ────
    //
    // 🔴 fix #2 from yesterday's code review. Previously Config::load
    // happened inside run_node_blocking, AFTER status=Running was
    // sent — so a misconfigured install would briefly flicker Running
    // → Stopped within milliseconds. SCM (and its consumers like the
    // services.msc UI) momentarily believe the service started
    // successfully and then crashed, which is misleading: the service
    // never even reached "starting normally."
    //
    // Now: load + validate while still in StartPending. On failure,
    // skip Running entirely and go directly StartPending → Stopped(1).
    // SCM never reports Running for a service that can't actually run.
    let config = match load_and_validate_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Service config validation failed: {}", e);
            status_handle.set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::Stopped,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(1),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })?;
            return Err(e);
        }
    };

    // Service is up — accept Stop notifications now. (Pause/Continue
    // intentionally not in the accepted list; see event_handler above.)
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    // ── 5. Run the node ─────────────────────────────────────────
    let result = run_node_blocking(config, stop_flag);

    // ── 6. Tell SCM we stopped ──────────────────────────────────
    // Always send Stopped, even on error — SCM otherwise thinks the
    // service is still running and won't let it be restarted.
    let exit_code = match &result {
        Ok(()) => ServiceExitCode::Win32(0),
        Err(_) => ServiceExitCode::Win32(1),
    };

    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    result
}

/// Load + validate config. Split out from `run_node_blocking` so the
/// service main can call this BEFORE flipping status to Running —
/// otherwise a misconfigured install briefly shows Running before
/// transitioning to Stopped, which looks to SCM like a successful
/// start followed by a crash. See run_service for the ordering rationale.
fn load_and_validate_config() -> Result<crate::Config, Box<dyn std::error::Error>> {
    use crate::Config;

    let config = Config::load(None).map_err(|e| {
        format!(
            "Failed to load config from {}: {}. \
             Run `opensentry-cloudnode setup` from an admin console first.",
            crate::paths::config_db_path().display(),
            e
        )
    })?;

    if config.cloud.api_key.is_empty() || config.node.node_id.is_none() {
        return Err(
            "CloudNode is not configured. Open an admin console and run \
             `opensentry-cloudnode setup` to enrol this node, then \
             `Start-Service OpenSentryCloudNode`."
                .into(),
        );
    }

    Ok(config)
}

/// Build the tokio runtime and run [`crate::Node::run_headless`] inside it.
/// Blocks until the node returns (which happens when `stop_flag` flips
/// or a fatal error occurs).
fn run_node_blocking(
    config: crate::Config,
    stop_flag: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::Node;

    tracing::info!("Node ID: {:?}", config.node.node_id);
    tracing::info!("API URL: {}", config.cloud.api_url);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let node = Node::new(config).await?;
        node.run_headless(stop_flag).await
    })?;

    Ok(())
}

/// Configure tracing to write to a daily-rotating file under
/// `%ProgramData%\OpenSentry\logs\`. Returns the appender's worker
/// guard, which keeps the background writer thread alive until the
/// service exits — drop the guard and pending log lines may be lost.
fn init_file_logging() -> Result<tracing_appender::non_blocking::WorkerGuard, Box<dyn std::error::Error>> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let log_dir = crate::paths::data_dir().join("logs");
    std::fs::create_dir_all(&log_dir)?;

    // Daily rotation. File name: `cloudnode-service.YYYY-MM-DD`.
    // We keep all of them — log retention isn't this binary's job; an
    // operator can clean up via PowerShell or schedule a task. A single
    // 1080p camera generates ~5 MB/day of INFO-level logs, so a year
    // at 1 camera is ~2 GB; manageable.
    let file_appender =
        tracing_appender::rolling::daily(&log_dir, "cloudnode-service");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Layer order: file appender + DashboardLayer.  The dashboard layer
    // is a no-op until Node::run_headless installs a Dashboard via
    // crate::logging::set_dashboard, then it sinks events to the SQLite
    // log table for the in-product log viewer. The file layer captures
    // everything from process start onward.
    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false), // no escapes in log files
        )
        .with(crate::logging::DashboardLayer)
        .init();

    Ok(guard)
}
