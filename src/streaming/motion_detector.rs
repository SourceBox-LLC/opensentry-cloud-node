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

    // Always extract ALL scene scores (threshold 0 in FFmpeg), then compare
    // against the configured threshold in Rust.  This lets us log actual peak
    // scores for debugging even when they fall below the trigger threshold.
    let result = tokio::time::timeout(
        FFMPEG_TIMEOUT,
        tokio::process::Command::new(&ffmpeg)
            .args([
                "-i",
                &segment_path.to_string_lossy(),
                "-vf",
                "scale=320:180,fps=5,select='gte(scene,0)',metadata=print",
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
    let peak = parse_peak_scene_score(&stderr);

    match peak {
        Some(score) if score >= threshold => {
            tracing::info!(
                "Motion detected: score={:.6} threshold={:.4} segment={}",
                score, threshold, segment_path.display()
            );
            Some(score)
        }
        Some(score) => {
            tracing::debug!(
                "No motion: peak={:.6} < threshold={:.4} segment={}",
                score, threshold, segment_path.display()
            );
            None
        }
        None => None,
    }
}

/// Parse FFmpeg metadata output for the highest `lavfi.scene_score` value.
///
/// FFmpeg `metadata=print` outputs lines like:
///   `[Parsed_metadata_3 @ 0x...] lavfi.scene_score=0.482361`
/// We search for the `lavfi.scene_score=` substring anywhere in each line.
fn parse_peak_scene_score(stderr: &str) -> Option<f64> {
    const NEEDLE: &str = "lavfi.scene_score=";
    let mut peak: Option<f64> = None;

    for line in stderr.lines() {
        if let Some(pos) = line.find(NEEDLE) {
            let val_str = &line[pos + NEEDLE.len()..];
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
        // Real FFmpeg output has a [Parsed_metadata_N @ 0x...] prefix
        let stderr = "\
[Parsed_metadata_3 @ 0x1234] frame:0    pts:0       pts_time:0
[Parsed_metadata_3 @ 0x1234] lavfi.scene_score=0.123456
[Parsed_metadata_3 @ 0x1234] frame:1    pts:3000    pts_time:0.033
[Parsed_metadata_3 @ 0x1234] lavfi.scene_score=0.654321
[Parsed_metadata_3 @ 0x1234] frame:2    pts:6000    pts_time:0.067
[Parsed_metadata_3 @ 0x1234] lavfi.scene_score=0.234567";
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
