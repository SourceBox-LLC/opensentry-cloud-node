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

//! Centralised data-directory resolution.
//!
//! CloudNode persists three things to disk: the encrypted SQLite config
//! database (`node.db`), recordings (`recordings/`), and the machine-id
//! fallback file used as the AES key seed on minimal Linux images. All
//! three need to agree on *where* "the data dir" is.
//!
//! Historically the answer was just `./data/` relative to the process's
//! working directory. That worked when CloudNode was launched manually
//! from a terminal, but the moment we ship a Windows MSI that registers
//! a Windows Service, the cwd becomes `C:\Windows\System32` (Service
//! Control Manager's default) and the relative path resolves to a
//! directory the LocalSystem account has no business writing into.
//!
//! Resolution order (first match wins):
//!
//! 1. `$SOURCEBOX_SENTRY_DATA_DIR` if set — explicit override, used by Docker
//!    and by the MSI-installed service registration.
//! 2. The legacy `./data/` directory if it already exists, so anyone who
//!    upgraded a manual `cargo build` install in-place keeps working.
//! 3. Platform default:
//!    - Windows: `%ProgramData%\SourceBoxSentry` (standard system-wide app
//!      data location; writable by services running as LocalSystem and
//!      by interactive admins).
//!    - Other: `./data` (matches legacy behaviour for Linux/macOS where
//!      the install scripts already drop the binary in a directory the
//!      user owns).
//!
//! The function is pure — it does NOT create the directory. Callers
//! that need it to exist should call `std::fs::create_dir_all` after.

use std::path::PathBuf;

/// Where CloudNode stores its config DB, recordings, and any other
/// persistent state. See module docs for the resolution order.
pub fn data_dir() -> PathBuf {
    // 1. Explicit override.
    if let Ok(dir) = std::env::var("SOURCEBOX_SENTRY_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }

    // 2. Legacy in-place ./data/ takes priority over the platform
    //    default so existing cargo-build installs don't migrate
    //    themselves on the next launch (which would silently abandon
    //    their config DB).
    //
    //    CRITICAL: this check requires `./data/node.db` to exist, NOT
    //    just `./data/`. Original logic checked only `./data` existence
    //    which produced a footgun for MSI users:
    //
    //      - User puts the test .bat or any random binary on their
    //        Desktop where they happen to have a folder named "data"
    //        (from some other project, or auto-created by an earlier
    //        confused invocation, or whatever).
    //      - cwd at launch = Desktop. `./data` resolves to
    //        `C:\Users\<x>\Desktop\data` and exists.
    //      - data_dir returns `./data`. Config::load reads
    //        `./data/node.db` — doesn't exist — needs_setup=true,
    //        wizard re-runs.
    //      - find_tool looks for ffmpeg at `./data/ffmpeg/bin/ffmpeg.exe`
    //        — doesn't exist — falls through to bare "ffmpeg" — PATH
    //        search fails — `Io error: program not found`.
    //      - Meanwhile the REAL data is at C:\ProgramData\SourceBoxSentry
    //        with node.db + ffmpeg/ all present and the binary just
    //        ignores it.
    //
    //    Requiring `node.db` inside `./data` anchors the legacy check to
    //    actual CloudNode state, not a coincidentally-named directory.
    //    Cargo-build dev workflows are unaffected: their `./data` always
    //    has node.db once setup has run there. Empty or unrelated
    //    `./data` folders fall through to the platform default.
    let legacy = PathBuf::from("./data");
    if legacy.join("node.db").exists() {
        return legacy;
    }

    // 3. Platform default.
    #[cfg(target_os = "windows")]
    {
        // Try the env var first — it's the "right" way and lets a
        // future enterprise install relocate ProgramData (rare).
        if let Ok(programdata) = std::env::var("ProgramData") {
            if !programdata.is_empty() {
                return PathBuf::from(programdata).join("SourceBoxSentry");
            }
        }
        // Hardcoded fallback. We don't reach this for interactive
        // user contexts (Windows always sets ProgramData for them),
        // but **Windows Services running as LocalSystem don't always
        // inherit the ProgramData env var** — observed empirically on
        // Windows 11 26200 where the SCM-managed environment block
        // omits it. Falling through to "./data" was disastrous: cwd
        // for a service is C:\Windows\System32, so data_dir resolved
        // to C:\Windows\System32\data which is unwritable + unfindable
        // by Config::load. Hardcode the canonical Windows location
        // (stable since Vista, documented at
        // KNOWNFOLDERID FOLDERID_ProgramData = "{0x62AB5D82,...}")
        // so the service finds the same files the setup wizard wrote.
        return PathBuf::from(r"C:\ProgramData").join("SourceBoxSentry");
    }

    // Final fallback: keep the legacy relative path so a fresh non-MSI
    // install on Linux/macOS behaves the same as before this refactor.
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("./data")
    }
}

/// Where the encrypted config SQLite lives.
///
/// Convenience wrapper — every caller wants `data_dir().join("node.db")`,
/// and centralising it means a future move to `config.db` or a versioned
/// filename is one edit, not a grep-and-replace across the codebase.
pub fn config_db_path() -> PathBuf {
    data_dir().join("node.db")
}
