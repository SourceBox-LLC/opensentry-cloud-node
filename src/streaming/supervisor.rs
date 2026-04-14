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

//! FFmpeg supervisor.
//!
//! Wraps a per-camera `HlsGenerator` with a loop that:
//!   * polls FFmpeg's exit status every `POLL_INTERVAL`
//!   * when the child exits, respawns it with exponential backoff
//!     (1s, 2s, 4s, …, capped at 30s — matches the WS reconnect backoff)
//!   * bails (→ `Failed`) after too many restarts in a 60s window, so a
//!     permanently broken camera / FFmpeg config / disk-full situation
//!     doesn't spin forever
//!   * pushes `CameraStatus::Streaming / Restarting / Failed` into the
//!     Dashboard so the WS + HTTP heartbeats report real pipeline state
//!     instead of the old hardcoded `"streaming"`.
//!
//! Prior to this supervisor, an FFmpeg crash (e.g. a disk-full errno -28,
//! a closed V4L2 fd, a segment-writer failure) would silently leave the
//! camera offline from the browser's point of view while the node still
//! reported `status: streaming` in every heartbeat. The backend MCP
//! tools would then tell users "update CloudNode to latest version"
//! when the real failure was upstream in FFmpeg.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::dashboard::{CameraStatus, Dashboard};
use crate::streaming::{HlsGenerator, HlsGeneratorConfig};

/// How often we poll FFmpeg's child process for exit.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Starting backoff after the first crash.
const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Upper bound on backoff between restart attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Restart-count sliding window.
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// If the pipeline crashes more than this many times inside
/// `RESTART_WINDOW`, stop trying and mark it `Failed`.
const MAX_RESTARTS_IN_WINDOW: u32 = 5;

/// How long the pipeline has to be healthy before we consider the
/// failure sequence "over" and reset the backoff.
const HEALTHY_RESET_THRESHOLD: Duration = Duration::from_secs(60);

/// A way to start FFmpeg — either a real capture device or a
/// synthetic test pattern (used as a fallback on machines without
/// a working webcam so dev / CI can still exercise the pipeline).
#[derive(Clone)]
pub enum PipelineSource {
    Device(String),
    /// (width, height, fps)
    TestPattern(u32, u32, u32),
}

/// Static config the supervisor needs for the lifetime of a camera.
pub struct SupervisorConfig {
    pub hls_config: HlsGeneratorConfig,
    /// Try this source first on every (re)start.
    pub primary: PipelineSource,
    /// If `primary` fails on the VERY FIRST attempt, fall through to
    /// this. Useful for "try real camera, fall back to test pattern".
    /// Ignored on subsequent restarts — once we've committed to a
    /// source that works, we stay with it.
    pub fallback: Option<PipelineSource>,
    pub camera_name: String,
    pub camera_id: String,
}

/// Supervise a single camera's HLS pipeline.
///
/// Owns a fresh `HlsGenerator` per run. Never returns under normal
/// operation — the caller either `abort()`s the task on shutdown or
/// flips `stop_flag` so we break out of the poll loop cleanly.
pub async fn supervise_hls(
    cfg: SupervisorConfig,
    dash: Dashboard,
    stop_flag: Arc<AtomicBool>,
) {
    let SupervisorConfig {
        hls_config,
        primary,
        fallback,
        camera_name,
        camera_id,
    } = cfg;

    // Which source we're currently trying. Starts as `primary`; if that
    // fails the very first attempt, we switch to `fallback` (once).
    let mut active_source = primary;
    let mut fallback_consumed = false;

    let mut backoff = BASE_BACKOFF;
    // Timestamps of recent crashes; anything older than RESTART_WINDOW
    // gets popped off the front so bursts after long healthy periods
    // don't accumulate.
    let mut crash_history: VecDeque<Instant> = VecDeque::new();

    'outer: loop {
        if stop_flag.load(Ordering::Relaxed) {
            return;
        }

        // ── Build a fresh generator and start FFmpeg ───────────────────
        let mut generator = match HlsGenerator::new(hls_config.clone()) {
            Ok(g) => g,
            Err(e) => {
                let err = format!("HlsGenerator::new failed: {}", e);
                dash.log_error(format!("[{}] supervisor: {}", camera_name, err));
                dash.update_camera_status_by_id(
                    &camera_id,
                    CameraStatus::Failed { last_error: err },
                );
                return;
            }
        };

        let start_result = start_with(&mut generator, &active_source);

        if let Err(e) = start_result {
            let err = format!("FFmpeg start failed: {}", e);
            dash.log_warn(format!("[{}] supervisor: {}", camera_name, err));

            // One-shot fallback on the very first attempt: if primary
            // failed and a fallback is configured, switch and try once
            // before counting this as a "real" crash.
            if !fallback_consumed && crash_history.is_empty() {
                if let Some(fb) = fallback.clone() {
                    dash.log_info(format!(
                        "[{}] supervisor: trying fallback source",
                        camera_name
                    ));
                    active_source = fb;
                    fallback_consumed = true;
                    // Immediately retry, no backoff — this is the
                    // probe path, not a restart.
                    continue 'outer;
                }
            }

            let should_restart = record_crash_and_maybe_bail(
                &mut crash_history,
                &camera_id,
                &camera_name,
                &err,
                &dash,
            );
            if !should_restart {
                return;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
            continue 'outer;
        }

        // FFmpeg is up. Flip the dashboard to Streaming.
        fallback_consumed = true; // don't downgrade to fallback after a success
        dash.update_camera_status_by_id(&camera_id, CameraStatus::Streaming);
        let alive_since = Instant::now();

        // ── Poll until FFmpeg exits or we're told to stop ──────────────
        loop {
            if stop_flag.load(Ordering::Relaxed) {
                // Best-effort kill before we drop the generator —
                // HlsGenerator has no Drop impl, so without this the
                // child would leak until the OS reaps it.
                let _ = generator.stop();
                return;
            }

            tokio::time::sleep(POLL_INTERVAL).await;

            let Some(status) = generator.check_process() else {
                // Still running — keep polling.
                continue;
            };

            let alive_for = alive_since.elapsed();
            let err = format!(
                "FFmpeg exited with {} after running {}s",
                status,
                alive_for.as_secs()
            );
            dash.log_warn(format!("[{}] {}", camera_name, err));

            // If the pipeline stayed healthy long enough, treat this
            // crash as a new failure sequence — reset backoff & window.
            if alive_for >= HEALTHY_RESET_THRESHOLD {
                crash_history.clear();
                backoff = BASE_BACKOFF;
            }

            let should_restart = record_crash_and_maybe_bail(
                &mut crash_history,
                &camera_id,
                &camera_name,
                &err,
                &dash,
            );
            if !should_restart {
                return;
            }

            // Flip to Restarting BEFORE the sleep so the very next
            // heartbeat surfaces the real state (instead of waiting
            // until we've actually respawned).
            let attempt = crash_history.len() as u32;
            dash.update_camera_status_by_id(
                &camera_id,
                CameraStatus::Restarting {
                    attempt,
                    last_error: err.clone(),
                },
            );
            dash.log_info(format!(
                "[{}] supervisor: restarting in {}s (attempt {})",
                camera_name,
                backoff.as_secs(),
                attempt
            ));
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
            continue 'outer;
        }
    }
}

fn start_with(
    generator: &mut HlsGenerator,
    source: &PipelineSource,
) -> crate::error::Result<()> {
    match source {
        PipelineSource::Device(path) => generator.start_from_device(path),
        PipelineSource::TestPattern(w, h, fps) => generator.start_from_frames(*w, *h, *fps),
    }
}

/// Append a crash to the sliding window. Returns `false` (→ bail, mark
/// Failed) if the window already has too many crashes, else `true`.
fn record_crash_and_maybe_bail(
    crash_history: &mut VecDeque<Instant>,
    camera_id: &str,
    camera_name: &str,
    last_error: &str,
    dash: &Dashboard,
) -> bool {
    let now = Instant::now();
    crash_history.push_back(now);
    // Drop crashes older than the window.
    while let Some(front) = crash_history.front() {
        if now.duration_since(*front) > RESTART_WINDOW {
            crash_history.pop_front();
        } else {
            break;
        }
    }

    if crash_history.len() as u32 > MAX_RESTARTS_IN_WINDOW {
        dash.log_error(format!(
            "[{}] supervisor: {} restarts in {}s — giving up",
            camera_name,
            crash_history.len(),
            RESTART_WINDOW.as_secs()
        ));
        dash.update_camera_status_by_id(
            camera_id,
            CameraStatus::Failed {
                last_error: last_error.to_string(),
            },
        );
        return false;
    }
    true
}
