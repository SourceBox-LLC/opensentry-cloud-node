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

//! Filesystem-aware storage stats.
//!
//! `StorageStats` is the snapshot CloudNode reports to Command Center on
//! every heartbeat: how much of the configured cap (`max_size_gb`) we've
//! used, plus how much free / total space the underlying filesystem has.
//! Command Center renders a per-node usage bar from these numbers and
//! warns the operator if the host disk itself is filling up.
//!
//! There's also a hard-floor recording-pause that fires when the host
//! filesystem drops below `SAFETY_FLOOR_BYTES` — independent of the
//! operator's `max_size_gb`.  The cap protects CloudNode against
//! growing past its allocation; the safety floor protects the *host*
//! against CloudNode filling its disk.  Without the floor, an operator
//! who sets `max_size_gb` larger than the disk can hold will eventually
//! hit `ENOSPC` and SQLite (or worse, every other process on the box)
//! breaks.
//!
//! The pause is a process-wide AtomicBool so the recording writer can
//! check it on the hot path without taking a lock.  Updated by
//! `StorageStats::collect` which runs from the heartbeat task tick
//! (every 30s by default).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use sysinfo::Disks;

/// Free-space threshold below which CloudNode pauses new recording writes.
/// 1 GiB is enough headroom that SQLite WAL flushes, FFmpeg HLS rotation,
/// and other normal operations can still finish without `ENOSPC`. Tune
/// upwards on systems with bigger working sets; never below ~256 MiB.
pub const SAFETY_FLOOR_BYTES: u64 = 1024 * 1024 * 1024;

/// Set to `true` whenever the most recent `StorageStats::collect` saw
/// disk-free below the safety floor.  The recording writer reads this
/// before each insert; if `true`, it skips the write and logs.
///
/// Process-wide static so any writer can read without plumbing a
/// reference through every call site.  Atomic so reads are lock-free
/// on the hot path.
pub static RECORDING_PAUSED_FOR_DISK: AtomicBool = AtomicBool::new(false);

/// Snapshot of node storage state, collected each heartbeat tick.
///
/// Wire-shaped: serializes directly into the `storage_stats` field of
/// `HeartbeatRequest` so Command Center can persist + display it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct StorageStats {
    /// Bytes the node has stored in its SQLite DB (snapshots +
    /// recording_segments tables — same source as `enforce_retention`).
    /// This is the numerator of the operator-visible usage bar.
    pub used_bytes: u64,

    /// Operator-configured cap from `config.storage.max_size_gb`,
    /// converted to bytes.  Denominator of the usage bar.
    pub max_bytes: u64,

    /// Free bytes on the filesystem that holds the data dir.  Used by
    /// Command Center to render a "host disk almost full" warning
    /// alongside the cap-based bar.  0 if we couldn't identify the disk
    /// (Command Center should hide the warning in that case rather than
    /// claim "0 bytes free").
    pub disk_free_bytes: u64,

    /// Total bytes on the same filesystem.  Lets the dashboard show
    /// "X GB free of Y GB" without a second round-trip.
    pub disk_total_bytes: u64,
}

impl StorageStats {
    /// Build a fresh snapshot.  Pulls used-bytes from the caller (cheap,
    /// already cached in the retention path), reads disk stats via
    /// `sysinfo`, and updates the global recording-pause flag as a side
    /// effect so the writers see the new state immediately.
    pub fn collect(used_bytes: u64, max_bytes: u64, data_dir: &Path) -> Self {
        let (disk_free_bytes, disk_total_bytes) = read_disk_info(data_dir);
        let stats = Self {
            used_bytes,
            max_bytes,
            disk_free_bytes,
            disk_total_bytes,
        };

        // Pause recording when the host disk is critically low — but
        // only if we actually know the free space.  A 0 here means
        // "couldn't identify the disk" (no mount-point matched the
        // data_dir) and pausing on that would leave a lot of nodes
        // stuck for no good reason.
        if disk_free_bytes > 0 {
            RECORDING_PAUSED_FOR_DISK.store(
                disk_free_bytes < SAFETY_FLOOR_BYTES,
                Ordering::Relaxed,
            );
        }

        stats
    }

    /// Cap-based usage percentage (0.0–100.0+).  Goes above 100 briefly
    /// between when the writer adds a segment and when retention fires;
    /// callers should clamp for display.
    pub fn percent_full(&self) -> f64 {
        if self.max_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes as f64 / self.max_bytes as f64) * 100.0
    }
}

/// Cheap O(disk_count) lookup of the filesystem free + total bytes for
/// the disk that contains `data_dir`.
///
/// Strategy: enumerate every mounted disk and pick the one whose mount
/// point is the *longest* prefix of `data_dir`.  Handles nested mounts
/// like `/var` mounted under `/`, where both match the prefix but
/// `/var` is more specific.  Returns (0, 0) if no disk matches —
/// expected on minimal Docker images that show a virtual rootfs but
/// don't expose a backing block device through `/proc`.
fn read_disk_info(data_dir: &Path) -> (u64, u64) {
    let abs = data_dir.canonicalize().unwrap_or_else(|_| data_dir.to_path_buf());

    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<&sysinfo::Disk> = None;
    let mut best_len: usize = 0;

    for disk in &disks {
        let mp = disk.mount_point();
        if abs.starts_with(mp) {
            // `len()` of the OsStr is fine as a "more-specific" tiebreaker
            // since any longer mount point that's also a prefix is by
            // definition deeper in the tree.
            let len = mp.as_os_str().len();
            if len >= best_len {
                best_len = len;
                best = Some(disk);
            }
        }
    }

    match best {
        Some(d) => (d.available_space(), d.total_space()),
        None => (0, 0),
    }
}

/// True if recording writers should skip new writes due to host-disk
/// pressure.  Cheap atomic read; safe to call on the hot path.
pub fn should_pause_recording() -> bool {
    RECORDING_PAUSED_FOR_DISK.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_full_handles_zero_cap() {
        let s = StorageStats {
            used_bytes: 100,
            max_bytes: 0,
            disk_free_bytes: 0,
            disk_total_bytes: 0,
        };
        // Zero-cap (legacy install / misconfigured node) must not panic
        // or divide-by-zero — return 0% so the bar shows empty rather
        // than NaN/Inf.
        assert_eq!(s.percent_full(), 0.0);
    }

    #[test]
    fn percent_full_at_cap_is_100() {
        let s = StorageStats {
            used_bytes: 64 * 1024 * 1024 * 1024,
            max_bytes: 64 * 1024 * 1024 * 1024,
            disk_free_bytes: 100 * 1024 * 1024 * 1024,
            disk_total_bytes: 500 * 1024 * 1024 * 1024,
        };
        assert!((s.percent_full() - 100.0).abs() < 0.001);
    }

    #[test]
    fn percent_full_above_cap_overflows_briefly() {
        // The retention loop runs every 5 min; between ticks the writer
        // can push usage just past the cap.  percent_full reports the
        // raw value; UI clamps for display.
        let s = StorageStats {
            used_bytes: 65 * 1024 * 1024 * 1024,
            max_bytes: 64 * 1024 * 1024 * 1024,
            disk_free_bytes: 100 * 1024 * 1024 * 1024,
            disk_total_bytes: 500 * 1024 * 1024 * 1024,
        };
        assert!(s.percent_full() > 100.0);
    }

    #[test]
    fn collect_with_unknown_disk_does_not_set_pause() {
        // A non-existent path should fall through to (0, 0) for disk
        // info, and that should NOT trip the pause flag — pausing on
        // "couldn't identify the disk" would brick a lot of edge-case
        // installs (Docker, weird FUSE mounts) for no reason.
        RECORDING_PAUSED_FOR_DISK.store(false, Ordering::Relaxed);
        let _stats = StorageStats::collect(
            0,
            64 * 1024 * 1024 * 1024,
            std::path::Path::new("/this/path/does/not/exist/anywhere"),
        );
        assert!(!RECORDING_PAUSED_FOR_DISK.load(Ordering::Relaxed));
    }
}
