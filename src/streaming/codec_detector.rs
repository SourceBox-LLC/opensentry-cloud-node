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
//! Codec Detector - Detects video/audio codecs from MPEG-TS segments using FFprobe
//!
//! Converts FFmpeg codec metadata (profile/level) into HLS-compatible codec strings (RFC 6381)

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

/// Detected codec information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecInfo {
    /// HLS-compatible video codec string (e.g., "avc1.42001E")
    pub video_codec: String,
    /// HLS-compatible audio codec string (e.g., "mp4a.40.2")
    pub audio_codec: String,
}

/// FFprobe stream information
#[derive(Debug, Deserialize)]
struct FFprobeStream {
    codec_name: Option<String>,
    profile: Option<String>,
    level: Option<i32>,
}

/// FFprobe output structure
#[derive(Debug, Deserialize)]
struct FFprobeOutput {
    streams: Vec<FFprobeStream>,
}

/// Convert FFmpeg codec names + profile/level to HLS codec strings (RFC 6381)
///
/// Examples:
/// - H.264 Baseline Level 3.0 → avc1.42e01e (lowercase hex)
/// - H.264 Main Level 4.1 → avc1.4da029
/// - AAC-LC → mp4a.40.2
/// Normalize H.264 level to nearest valid value
///
/// Valid H.264 levels: 10, 11, 12, 13, 20, 21, 22, 30, 31, 32, 40, 41, 42, 50, 51, 52
/// FFmpeg sometimes reports non-standard values that need normalization
fn normalize_h264_level(level: i32) -> String {
    let valid_levels = [
        10, 11, 12, 13, 20, 21, 22, 30, 31, 32, 40, 41, 42, 50, 51, 52,
    ];

    let normalized = valid_levels
        .iter()
        .min_by_key(|&&valid| (valid - level).abs())
        .copied()
        .unwrap_or(30);

    format!("{:02x}", normalized)
}

fn to_hls_codec_string(codec: &str, profile: Option<&str>, level: Option<i32>) -> String {
    match codec {
        "h264" => {
            let (profile_hex, constraint_byte) = match profile {
                Some("Baseline") | Some("CB") | Some("Constrained Baseline") => ("42", "e0"),
                Some("Main") => ("4d", "a0"),
                Some("High") => ("64", "a0"),
                Some(p) => {
                    tracing::warn!("Unknown H.264 profile '{}', using Baseline", p);
                    ("42", "e0")
                }
                None => ("42", "e0"),
            };

            let level_hex = level
                .map(|l| normalize_h264_level(l))
                .unwrap_or_else(|| "1e".to_string());

            format!("avc1.{}{}{}", profile_hex, constraint_byte, level_hex)
        }
        "hevc" | "h265" => {
            let profile_num = match profile {
                Some("Main") => "1",
                Some("Main10") => "2",
                Some(p) => {
                    tracing::warn!("Unknown HEVC profile '{}', using Main", p);
                    "1"
                }
                None => "1",
            };

            let level_num = level.map(|l| l / 30).unwrap_or(90) / 30;

            format!("hvc1.{}.L{}.B0", profile_num, level_num)
        }
        "aac" => "mp4a.40.2".to_string(),
        "opus" => "opus".to_string(),
        "mp3" | "mpga" => "mp4a.40.34".to_string(),
        codec_name => {
            tracing::warn!("Unknown codec '{}', using as-is", codec_name);
            codec_name.to_lowercase()
        }
    }
}

/// Detect video and audio codecs from an MPEG-TS segment file using FFprobe
///
/// # Arguments
/// * `segment_path` - Path to the segment file (e.g., "segment_00001.ts")
///
/// # Returns
/// * `CodecInfo` with HLS-compatible codec strings
///
/// # Errors
/// * If FFprobe is not found
/// * If segment file cannot be read
/// * If segment has no video stream
pub fn detect_codec(segment_path: &Path) -> Result<CodecInfo> {
    tracing::info!("[Codec] Detecting codec from segment: {:?}", segment_path);

    // Find FFprobe executable
    let ffprobe_path = find_ffprobe();

    // Run FFprobe to get video stream info
    let segment_path_str = segment_path.to_str().ok_or_else(|| {
        Error::Storage(format!(
            "Segment path contains invalid UTF-8: {:?}",
            segment_path
        ))
    })?;

    let video_output = Command::new(&ffprobe_path)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0", // First video stream
            "-show_entries",
            "stream=codec_name,profile,level",
            "-of",
            "json",
            segment_path_str,
        ])
        .output()
        .map_err(|e| {
            tracing::error!("[Codec] Failed to run FFprobe: {}", e);
            Error::Storage(format!("FFprobe execution failed: {}", e))
        })?;

    if !video_output.status.success() {
        let stderr = String::from_utf8_lossy(&video_output.stderr);
        tracing::error!("[Codec] FFprobe failed: {}", stderr);
        return Err(Error::Storage(format!("FFprobe failed: {}", stderr)));
    }

    // Parse video stream info
    let video_info: FFprobeOutput =
        serde_json::from_slice(&video_output.stdout).map_err(Error::Serialize)?;

    let video_stream = video_info
        .streams
        .first()
        .ok_or(Error::Storage("No video stream found in segment".into()))?;

    let video_codec_name = video_stream
        .codec_name
        .as_ref()
        .ok_or(Error::Storage("Video stream has no codec_name".into()))?;

    tracing::info!(
        "[Codec] Video stream: codec={}, profile={:?}, level={:?}",
        video_codec_name,
        video_stream.profile,
        video_stream.level
    );

    // Convert to HLS codec string
    let video_codec = to_hls_codec_string(
        video_codec_name,
        video_stream.profile.as_deref(),
        video_stream.level,
    );

    // Run FFprobe to get audio stream info
    let audio_output = Command::new(&ffprobe_path)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0", // First audio stream
            "-show_entries",
            "stream=codec_name",
            "-of",
            "json",
            segment_path_str,
        ])
        .output()
        .map_err(|e| Error::Storage(format!("FFprobe audio detection failed: {}", e)))?;

    let audio_codec = if audio_output.status.success() {
        let audio_info: FFprobeOutput = serde_json::from_slice(&audio_output.stdout)
            .unwrap_or(FFprobeOutput { streams: vec![] });

        let audio_codec_name = audio_info
            .streams
            .first()
            .and_then(|s| s.codec_name.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("aac");

        tracing::info!("[Codec] Audio stream: codec={}", audio_codec_name);

        to_hls_codec_string(audio_codec_name, None, None)
    } else {
        // No audio stream - use default AAC-LC
        tracing::warn!("[Codec] No audio stream found, using default AAC-LC");
        "mp4a.40.2".to_string()
    };

    Ok(CodecInfo {
        video_codec,
        audio_codec,
    })
}

/// Find FFprobe — delegates to the shared `streaming::find_ffprobe` so
/// bundled paths and platform-aware fallbacks stay in one place.
fn find_ffprobe() -> String {
    super::find_ffprobe()
}

/// Find FFmpeg — delegates to the shared `streaming::find_ffmpeg`.
fn find_ffmpeg() -> String {
    super::find_ffmpeg()
}

/// Detect codec by capturing a single frame from camera during setup
pub fn detect_codec_from_camera(camera_device: &str) -> Result<CodecInfo> {
    use std::process::Command;

    tracing::info!("[Codec] Detecting codec from camera: {}", camera_device);

    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join("opensentry_codec_test.ts");

    let ffmpeg_path = find_ffmpeg();

    // FFmpeg 7+ on Windows requires "video=" prefix for DirectShow devices
    // FFmpeg 6 and earlier accept device name directly
    #[cfg(target_os = "windows")]
    {
        let input_device = format!("video={}", camera_device);
        let mut args = vec!["-f", "dshow", "-i", &input_device];
        args.extend(vec![
            "-frames:v",
            "30",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-f",
            "mpegts",
            temp_file.to_str().ok_or_else(|| {
                Error::Storage("Temp file path contains invalid UTF-8".to_string())
            })?,
        ]);

        let status = Command::new(&ffmpeg_path)
            .args(&args)
            .output()
            .map_err(|e| Error::Storage(format!("FFmpeg execution failed: {}", e)))?;

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            return Err(Error::Storage(format!("FFmpeg failed: {}", stderr)));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut args = vec!["-f", "v4l2", "-i", camera_device];
        args.extend(vec![
            "-frames:v",
            "30",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-f",
            "mpegts",
            temp_file.to_str().ok_or_else(|| {
                Error::Storage("Temp file path contains invalid UTF-8".to_string())
            })?,
        ]);

        let status = Command::new(&ffmpeg_path)
            .args(&args)
            .output()
            .map_err(|e| Error::Storage(format!("FFmpeg execution failed: {}", e)))?;

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            return Err(Error::Storage(format!("FFmpeg failed: {}", stderr)));
        }
    }

    let codec_info = detect_codec(&temp_file)?;

    if temp_file.exists() {
        let _ = std::fs::remove_file(&temp_file);
    }

    Ok(codec_info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_h264_level_valid() {
        assert_eq!(normalize_h264_level(30), "1e"); // Level 3.0
        assert_eq!(normalize_h264_level(31), "1f"); // Level 3.1
        assert_eq!(normalize_h264_level(41), "29"); // Level 4.1
    }

    #[test]
    fn test_normalize_h264_level_invalid() {
        // Invalid values should round to nearest valid level, then format as hex
        assert_eq!(normalize_h264_level(15), "0d"); // 15 -> level 13 -> hex 0d
        assert_eq!(normalize_h264_level(33), "20"); // 33 -> level 32 -> hex 20
    }

    #[test]
    fn test_h264_baseline_codec_string() {
        assert_eq!(
            to_hls_codec_string("h264", Some("Baseline"), Some(30)),
            "avc1.42e01e"
        );
    }

    #[test]
    fn test_h264_main_codec_string() {
        assert_eq!(
            to_hls_codec_string("h264", Some("Main"), Some(41)),
            "avc1.4da029"
        );
    }

    #[test]
    fn test_h264_high_codec_string() {
        assert_eq!(
            to_hls_codec_string("h264", Some("High"), Some(51)),
            "avc1.64a033"
        );
    }

    #[test]
    fn test_aac_codec_string() {
        assert_eq!(to_hls_codec_string("aac", None, None), "mp4a.40.2");
    }

    #[test]
    fn test_unknown_codec_string() {
        assert_eq!(to_hls_codec_string("vp9", None, None), "vp9");
    }
}
