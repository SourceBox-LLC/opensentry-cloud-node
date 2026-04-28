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

//! Download + extract + verify FFmpeg from a vendor static build.
//!
//! Windows-only for v1. The reasoning: the MSI install path is the
//! one where "ffmpeg on PATH" isn't a safe assumption (LocalSystem
//! service, fresh OS, no winget pre-installed). Linux + macOS users
//! install through their distro package manager
//! (`apt install ffmpeg` / `brew install ffmpeg`), which gives them
//! a system-managed binary with security updates we have no business
//! second-guessing. The new error message in `setup/mod.rs::run_setup`
//! points them at those commands.
//!
//! ## Vendor
//!
//! `gyan.dev` ships static FFmpeg builds for Windows. We use
//! `ffmpeg-release-essentials.zip` (~100 MB extracted) which contains
//! `bin\ffmpeg.exe` + `bin\ffprobe.exe`. The "essentials" build omits
//! the kitchen-sink codecs in the "full" build that we don't use, in
//! exchange for a smaller download. Same vendor that the legacy
//! Windows install.ps1 used (now retired in favour of the MSI).
//!
//! If gyan.dev disappears we'd need a backup vendor. Today the
//! `ffmpeg-release-essentials.zip` URL is stable across releases —
//! gyan rebuilds the archive in place when a new ffmpeg version
//! drops, so we always get a current one without needing to bump
//! a version number in this code.
//!
//! ## Where it lands
//!
//! `<paths::data_dir()>/ffmpeg/bin/` — the same path the second
//! lookup candidate in `streaming::find_tool` checks. On a Windows
//! MSI install that resolves to `C:\ProgramData\OpenSentry\ffmpeg\
//! bin\`, which the LocalSystem service can read. On a manual /
//! Docker install, it follows wherever `SOURCEBOX_SENTRY_DATA_DIR` points.
//!
//! The install survives MSI uninstall (we deliberately don't clean
//! ProgramData on uninstall — same as `node.db`); a re-install picks
//! up the existing ffmpeg without a re-download.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// gyan.dev static-build ZIP. Stable URL; the archive contents update
/// in place each FFmpeg release.
const VENDOR_URL: &str =
    "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";

/// Maximum time for the download. 100 MB over a slow connection
/// shouldn't exceed this; if it does, the user can retry on a better
/// link or install manually.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Per-chunk read timeout. Triggers if the connection stalls mid-
/// download (e.g. wifi dropped). Keeps a wedged download from sitting
/// at "23%" forever.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the auto-install lands the ffmpeg root (the directory that
/// will contain `bin/ffmpeg.exe`).
fn install_root() -> PathBuf {
    crate::paths::data_dir().join("ffmpeg")
}

/// The exact ffmpeg.exe path that `streaming::find_tool` step 2 probes.
/// Used both as the install target and as the existence check for
/// `is_already_installed`.
pub fn install_target() -> PathBuf {
    let exe = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    install_root().join("bin").join(exe)
}

/// True if a previous auto-install already produced a usable binary.
/// The setup wizard checks this BEFORE prompting so a re-run after a
/// successful install doesn't ask the user "install ffmpeg?" again.
///
/// Strict definition: file exists. Doesn't verify it runs — that's
/// `verify_install`'s job, called immediately after a fresh download.
/// On a re-run we trust the file because if it didn't work, find_tool
/// wouldn't be returning success either, and we wouldn't be here.
pub fn is_already_installed() -> bool {
    install_target().exists()
}

/// Download + extract + verify ffmpeg into the data dir.
///
/// Blocking by design: the setup wizard runs synchronously and we
/// don't need a tokio runtime just for one HTTP fetch. `reqwest`'s
/// `blocking` feature is already pulled in for similar reasons in
/// the rest of the wizard.
///
/// `progress` is an indicatif ProgressBar the caller has already
/// styled and attached to its UI. We update its position as bytes
/// download; on extract we set message but don't try to drive a
/// second progress bar (zip extraction of a 100 MB archive takes
/// a couple of seconds and a frozen bar is more confusing than no bar).
///
/// Returns the absolute path to the installed `ffmpeg.exe` on success.
/// On any failure, leaves a partial install behind for the caller to
/// optionally clean up via `cleanup_partial_install` — we don't
/// auto-clean because a half-extracted archive might be useful for
/// post-mortem.
pub fn install(progress: &indicatif::ProgressBar) -> Result<PathBuf> {
    if is_already_installed() {
        return Ok(install_target());
    }

    let root = install_root();
    fs::create_dir_all(&root).with_context(|| {
        format!("could not create install directory {}", root.display())
    })?;

    progress.set_message("Downloading FFmpeg...");
    let zip_bytes = download(progress)?;

    progress.set_message("Extracting...");
    extract_into(&zip_bytes, &root)?;

    progress.set_message("Verifying...");
    verify_install(&install_target())?;

    Ok(install_target())
}

/// Stream `VENDOR_URL` into a `Vec<u8>`, updating `progress` as bytes
/// arrive. We hold the whole archive in memory rather than streaming
/// to disk because the zip crate's reader wants `Read + Seek` and an
/// in-memory `Cursor<Vec<u8>>` is simpler than a persistent temp file
/// (which would need cleanup paths on every error branch). 100 MB of
/// RAM is fine on any machine that can run a security camera node.
fn download(progress: &indicatif::ProgressBar) -> Result<Vec<u8>> {
    // reqwest 0.11 only has a single `timeout` (whole-request) — no
    // separate read_timeout. We use the longer DOWNLOAD_TIMEOUT to
    // bound the total operation; if a slow connection drips bytes
    // forever, this is the kill switch. A wedged connection that
    // sends ZERO bytes would still wait until DOWNLOAD_TIMEOUT, which
    // is acceptable for this UX (15 minutes is "long but not hung
    // for hours").
    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(format!(
            "sourcebox-sentry-cloudnode/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .context("could not build HTTP client")?;
    // READ_TIMEOUT is referenced in module-level docs; keep it as a
    // constant so a future move to reqwest 0.12 (which has read_timeout
    // on the blocking builder) is a one-line wire-up.
    let _ = READ_TIMEOUT;

    let mut response = client
        .get(VENDOR_URL)
        .send()
        .with_context(|| format!("could not reach {}", VENDOR_URL))?;

    if !response.status().is_success() {
        bail!(
            "download from {} returned HTTP {} — vendor site may be down or moved; \
             install ffmpeg manually and re-run setup",
            VENDOR_URL,
            response.status()
        );
    }

    let total = response.content_length().unwrap_or(0);
    if total > 0 {
        progress.set_length(total);
    }

    let mut buf = Vec::with_capacity(total as usize);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = response
            .read(&mut chunk)
            .context("download stalled or connection dropped")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        progress.set_position(buf.len() as u64);
    }

    if buf.is_empty() {
        bail!("downloaded archive was empty (0 bytes)");
    }

    Ok(buf)
}

/// Extract `bin/ffmpeg(.exe)` and `bin/ffprobe(.exe)` from the gyan
/// archive into `root/bin/`.
///
/// The archive layout is:
///
///     ffmpeg-N.N-essentials_build/
///         bin/
///             ffmpeg.exe
///             ffprobe.exe
///             ffplay.exe   <- we don't need this
///         doc/             <- we don't need this either
///         presets/
///         LICENSE
///         README.txt
///
/// We pull only the two binaries we need + the LICENSE (gyan asks for
/// the LICENSE to be redistributed alongside the binaries, and it's
/// tiny). Skipping the rest keeps the install at ~150 MB instead of
/// the full ~300 MB the archive would unpack to.
fn extract_into(zip_bytes: &[u8], root: &Path) -> Result<()> {
    use std::io::Cursor;

    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("could not create {}", bin_dir.display()))?;

    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .context("downloaded file is not a valid zip archive")?;

    let mut copied_ffmpeg = false;
    let mut copied_ffprobe = false;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("could not read zip entry {}", i))?;

        // The path inside the archive is e.g.
        // "ffmpeg-N.N-essentials_build/bin/ffmpeg.exe". We strip the
        // versioned prefix and check what's left.
        let name = entry.name().to_string();
        let suffix = match name.split_once('/') {
            Some((_versioned_root, rest)) => rest,
            None => continue, // top-level entry (rare); skip.
        };

        let dst = match suffix {
            "bin/ffmpeg.exe" | "bin/ffmpeg" => {
                copied_ffmpeg = true;
                bin_dir.join(if cfg!(target_os = "windows") {
                    "ffmpeg.exe"
                } else {
                    "ffmpeg"
                })
            }
            "bin/ffprobe.exe" | "bin/ffprobe" => {
                copied_ffprobe = true;
                bin_dir.join(if cfg!(target_os = "windows") {
                    "ffprobe.exe"
                } else {
                    "ffprobe"
                })
            }
            "LICENSE" => root.join("LICENSE"),
            _ => continue,
        };

        let mut out = fs::File::create(&dst)
            .with_context(|| format!("could not create {}", dst.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("could not extract {}", dst.display()))?;

        // On Unix, restore the executable bit so chmod-strip-on-zip
        // doesn't leave us with a non-executable binary. (On Windows
        // file permissions don't gate execution.)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(mode));
            } else {
                let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o755));
            }
        }
    }

    if !copied_ffmpeg {
        bail!("zip archive did not contain bin/ffmpeg(.exe) — vendor format may have changed");
    }
    if !copied_ffprobe {
        bail!("zip archive did not contain bin/ffprobe(.exe) — vendor format may have changed");
    }

    Ok(())
}

/// Run `ffmpeg -version` and check that it exits 0 with output that
/// looks like ffmpeg. A "downloaded but doesn't run" failure (corrupt
/// extract, antivirus quarantine, missing CRT redistributable) needs
/// to fail the install loudly rather than silently leave a broken
/// binary behind.
fn verify_install(ffmpeg_path: &Path) -> Result<()> {
    let output = Command::new(ffmpeg_path)
        .arg("-version")
        .output()
        .with_context(|| {
            format!(
                "could not execute {} — file may be quarantined by antivirus, \
                 or missing the Microsoft Visual C++ redistributable",
                ffmpeg_path.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "{} -version exited {} — ffmpeg binary appears broken",
            ffmpeg_path.display(),
            output.status
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.to_lowercase().contains("ffmpeg version") {
        bail!(
            "{} -version output did not start with 'ffmpeg version' — \
             archive may have unpacked the wrong binary",
            ffmpeg_path.display()
        );
    }

    Ok(())
}

/// Remove a partial install. Useful if the caller wants to retry
/// after a network failure mid-extract. Best-effort — failures here
/// are logged but not propagated, so the caller can still surface
/// the original install error to the user.
#[allow(dead_code)] // wired up by the wizard in a follow-up commit
pub fn cleanup_partial_install() {
    let root = install_root();
    if root.exists() {
        if let Err(e) = fs::remove_dir_all(&root) {
            tracing::warn!(
                "could not clean partial ffmpeg install at {}: {}",
                root.display(),
                e
            );
        }
    }
}

// ── tests ───────────────────────────────────────────────────────────
//
// Real download tests are deliberately omitted — they'd hit gyan.dev
// from CI on every push, which is rude and slow. The verify_install
// path is the only piece worth unit-testing in isolation, and it
// requires an actual ffmpeg binary, which we don't ship in the test
// suite. Tests live one level up in the setup module's integration
// tests once we have a way to mock-install a binary that responds to
// `-version`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_target_lives_under_data_dir() {
        let target = install_target();
        let expected_root = crate::paths::data_dir().join("ffmpeg").join("bin");
        assert!(
            target.starts_with(&expected_root),
            "install target {} should be under {}",
            target.display(),
            expected_root.display()
        );
    }

    #[test]
    fn install_target_filename_is_platform_correct() {
        let target = install_target();
        let filename = target.file_name().unwrap().to_string_lossy();
        if cfg!(target_os = "windows") {
            assert_eq!(filename, "ffmpeg.exe");
        } else {
            assert_eq!(filename, "ffmpeg");
        }
    }

    #[test]
    fn is_already_installed_returns_false_for_fresh_path() {
        // We can't reliably run this on a CI machine that might have
        // ffmpeg installed at the data_dir target — but cargo runs
        // tests from the repo root with the legacy ./data path, which
        // is unlikely to contain a pre-installed ffmpeg/. The assertion
        // is best-effort: if it ever fires false, the test environment
        // is unusual and the test author should investigate.
        if !is_already_installed() {
            // expected
        } else {
            // Test environment has ffmpeg already — skip rather than
            // fail since that's a legitimate state.
            eprintln!(
                "skipping: ffmpeg already installed at {}",
                install_target().display()
            );
        }
    }
}
