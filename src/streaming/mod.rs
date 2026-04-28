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
//! HLS Streaming Module
//!
//! Generates HLS (HTTP Live Streaming) segments from camera frames
//! and uploads them to cloud storage.

pub mod codec_detector;
pub mod hls_generator;
pub mod hls_uploader;
pub mod motion_detector;
pub mod segment_uploader;
pub mod supervisor;

pub use codec_detector::CodecInfo;
pub use hls_generator::HlsGenerator;
pub use hls_generator::HlsGeneratorConfig;
pub use hls_generator::HlsSegment;
pub use hls_uploader::HlsUploader;
pub use hls_uploader::HlsUploaderConfig;
pub use segment_uploader::SegmentUploader;
pub use segment_uploader::UploaderConfig;

/// Find FFmpeg executable — local bundled copy first, then common install
/// paths, then system PATH.
///
/// The PATH fallback is the traditional behaviour, but under a restricted
/// environment (e.g. a systemd unit where PATH=/usr/bin) bare `ffmpeg`
/// resolution can fail even when ffmpeg is installed at `/usr/local/bin`
/// or `/opt/homebrew/bin`.  Probing those locations explicitly removes a
/// class of "works in my shell but not as a service" surprises.
///
/// Shared by hls_generator, motion_detector, and websocket snapshot handler.
pub fn find_ffmpeg() -> String {
    find_tool("ffmpeg")
}

/// Find FFprobe executable — same rules as `find_ffmpeg`.
pub fn find_ffprobe() -> String {
    find_tool("ffprobe")
}

/// Platform-aware executable lookup for FFmpeg-family tools.
///
/// `name` is the bare name ("ffmpeg" or "ffprobe"); the Windows branch
/// appends `.exe` automatically.
///
/// # Lookup precedence
///
/// 1. **Cwd-bundled copy** (`<cwd>/ffmpeg/bin/<name>`) — preserves the
///    legacy `cargo run` developer flow where ffmpeg lives next to the
///    repo root.
/// 2. **Data-dir bundled copy** (`<data_dir>/ffmpeg/bin/<name>`) — where
///    the setup wizard's auto-install lands ffmpeg, and where the MSI
///    service (cwd = `C:\Windows\System32`) finds it. Without this
///    step the service couldn't see the auto-installed binary at all,
///    because cwd is wrong and there's no symlink machinery on Windows
///    we want to lean on.
/// 3. **Well-known absolute paths** (Linux/macOS) — handles the
///    "works in my shell but not as a service" trap where systemd runs
///    with PATH=/usr/bin:/bin and a brew-installed `ffmpeg` at
///    `/opt/homebrew/bin/ffmpeg` becomes invisible.
/// 4. **System PATH** (last resort) — bare `name`, lets the OS resolve.
fn find_tool(name: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let exe_name = format!("{}.exe", name);

        // 1. Cwd-bundled copy.
        if let Ok(cwd) = std::env::current_dir() {
            let local = cwd.join("ffmpeg").join("bin").join(&exe_name);
            if local.exists() {
                return local.to_string_lossy().to_string();
            }
        }

        // 2. Data-dir bundled copy. Critical for the MSI Service path —
        //    that process runs with cwd = System32, so step 1 misses
        //    even when the auto-install dropped ffmpeg into
        //    %ProgramData%\SourceBoxSentry\ffmpeg\bin\.
        let data_local = crate::paths::data_dir().join("ffmpeg").join("bin").join(&exe_name);
        if data_local.exists() {
            return data_local.to_string_lossy().to_string();
        }

        // 3. PATH fallback.
        return name.to_string();
    }

    #[cfg(not(target_os = "windows"))]
    {
        // 1. Cwd-bundled copy.
        if let Ok(cwd) = std::env::current_dir() {
            let local = cwd.join("ffmpeg").join("bin").join(name);
            if local.exists() {
                return local.to_string_lossy().to_string();
            }
        }

        // 2. Data-dir bundled copy. Mirrors the Windows behaviour so
        //    the auto-install path works identically across platforms.
        let data_local = crate::paths::data_dir().join("ffmpeg").join("bin").join(name);
        if data_local.exists() {
            return data_local.to_string_lossy().to_string();
        }

        // 3. Well-known absolute paths.  Apt/dnf/pacman land in /usr/bin;
        //    source or manual installs in /usr/local/bin; Homebrew on Apple
        //    Silicon uses /opt/homebrew/bin; Intel macOS uses /usr/local/bin.
        //    Checking these explicitly is the difference between "works when
        //    I run it from my shell" and "works when systemd runs it with
        //    PATH=/usr/bin:/bin".
        for candidate in [
            "/usr/local/bin",
            "/usr/bin",
            "/opt/homebrew/bin",
            "/snap/bin",
        ] {
            let p = std::path::Path::new(candidate).join(name);
            if p.exists() {
                return p.to_string_lossy().to_string();
            }
        }

        // 4. Last resort: bare name — let the OS resolve via PATH.
        name.to_string()
    }
}