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

//! Lightweight motion detection using FFmpeg scene-change scoring.
//!
//! Runs `ffmpeg -i <segment> -vf "select='gte(scene,T)',metadata=print"` on
//! each completed HLS segment. If any frame exceeds the threshold the segment
//! is flagged as containing motion and the peak score is returned.

use std::path::Path;
use std::time::Duration;

/// Maximum time FFmpeg is allowed to run per segment before we give up.
/// With the 320x180 downscale, even a 2-second segment should analyse in
/// well under 5 seconds.  If it takes longer, something is broken.
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(5);

/// Analyse a `.ts` segment for motion using FFmpeg scene-change detection.
///
/// Returns `Some(peak_score)` if any frame's scene score exceeds `threshold`,
/// or `None` if there is no significant motion (or FFmpeg fails/times out).
pub async fn detect_motion(segment_path: &Path, threshold: f64) -> Option<f64> {
    if !segment_path.exists() {
        return None;
    }

    let ffmpeg = super::find_ffmpeg();
    let threshold_str = format!("{:.4}", threshold);

    // Ask FFmpeg to select frames whose scene score exceeds the threshold and
    // print the metadata to stderr.  The `-f null -` output discards decoded
    // frames — we only care about the metadata lines.
    let result = tokio::time::timeout(
        FFMPEG_TIMEOUT,
        tokio::process::Command::new(&ffmpeg)
            .args([
                "-i",
                &segment_path.to_string_lossy(),
                "-vf",
                &format!("scale=320:180,select='gte(scene,{})',metadata=print", threshold_str),
                "-an",
                "-f",
                "null",
                "-",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await;

    let output = match result {
        Ok(Ok(o)) => o,
        Ok(Err(_)) => return None,   // FFmpeg failed to run
        Err(_) => {
            tracing::warn!("Motion detection timed out for {}", segment_path.display());
            return None;
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_peak_scene_score(&stderr)
}

/// Parse FFmpeg metadata output for the highest `lavfi.scene_score` value.
///
/// FFmpeg prints lines like:
///   `lavfi.scene_score=0.482361`
/// We extract all of them and return the maximum.
fn parse_peak_scene_score(stderr: &str) -> Option<f64> {
    let mut peak: Option<f64> = None;

    for line in stderr.lines() {
        if let Some(val_str) = line.strip_prefix("lavfi.scene_score=") {
            if let Ok(score) = val_str.trim().parse::<f64>() {
                peak = Some(peak.map_or(score, |p: f64| p.max(score)));
            }
        }
    }

    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_peak_scene_score_found() {
        let stderr = "\
frame:0    pts:0       pts_time:0
lavfi.scene_score=0.123456
frame:1    pts:3000    pts_time:0.033
lavfi.scene_score=0.654321
frame:2    pts:6000    pts_time:0.067
lavfi.scene_score=0.234567";
        assert_eq!(parse_peak_scene_score(stderr), Some(0.654321));
    }

    #[test]
    fn test_parse_peak_scene_score_none() {
        let stderr = "some random ffmpeg output\nno scores here";
        assert_eq!(parse_peak_scene_score(stderr), None);
    }

    #[test]
    fn test_parse_peak_scene_score_empty() {
        assert_eq!(parse_peak_scene_score(""), None);
    }
}
