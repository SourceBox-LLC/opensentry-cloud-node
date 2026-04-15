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
//! USB Camera Detection (Platform-agnostic)
//!
//! Provides platform-agnostic camera detection by delegating to platform-specific
//! implementations (Linux v4l2, Windows DirectShow, macOS AVFoundation).

use super::types::CameraCapabilities;
use crate::error::Result;

pub use crate::camera::platform::{
    create_detector, is_valid_device_path, validate_device_available, CameraDetector,
};

/// Information about a detected camera
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectedCamera {
    /// Device path or identifier (platform-specific)
    /// - Linux: /dev/video0
    /// - Windows: Camera Name (DirectShow device name)
    /// - macOS: 0 (AVFoundation device index)
    pub device_path: String,

    /// Camera name from system
    pub name: String,

    /// Camera capabilities
    pub capabilities: CameraCapabilities,

    /// Supported resolutions (width, height)
    pub supported_resolutions: Vec<(u32, u32)>,

    /// Preferred resolution for streaming
    pub preferred_resolution: (u32, u32),
}

/// Detect all connected cameras (platform-agnostic)
///
/// Automatically detects the current platform and uses the appropriate
/// camera detection method:
/// - Linux: Scans /dev/video* devices using v4l2
/// - Windows: Enumerates DirectShow video devices via FFmpeg
/// - macOS: Enumerates AVFoundation devices via FFmpeg
///
/// # Errors
///
/// Returns an error if:
/// - Platform detection fails
/// - Camera enumeration fails
/// - Required dependencies (FFmpeg on Windows/macOS) are not available
///
/// # Example
///
/// ```rust,no_run
/// use opensentry_cloudnode::camera::detect_cameras;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let cameras = detect_cameras()?;
///     for camera in cameras {
///         println!("Found camera: {} at {}", camera.name, camera.device_path);
///     }
///     Ok(())
/// }
/// ```
pub fn detect_cameras() -> Result<Vec<DetectedCamera>> {
    let detector = create_detector();

    tracing::info!("Detecting cameras on {}...", detector.platform_name());

    let cameras = detector.detect_cameras()?;

    tracing::info!("Found {} camera(s)", cameras.len());

    Ok(cameras)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_cameras_runs() {
        // This will work on Linux/Windows/macOS
        // Just ensure it doesn't panic
        let result = detect_cameras();
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_valid_device_path() {
        #[cfg(target_os = "linux")]
        {
            assert!(is_valid_device_path("/dev/video0"));
            assert!(is_valid_device_path("/dev/video9"));
            assert!(!is_valid_device_path("/dev/sda"));
        }

        #[cfg(target_os = "windows")]
        {
            // Windows uses camera names, so any string is valid
            assert!(is_valid_device_path("USB Camera"));
            assert!(is_valid_device_path("Integrated Camera"));
        }

        #[cfg(target_os = "macos")]
        {
            // macOS uses numeric indices
            assert!(is_valid_device_path("0"));
            assert!(is_valid_device_path("1"));
            assert!(!is_valid_device_path("invalid"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_validate_device_available_missing_path_reports_not_found() {
        // A path we're confident will never exist — the message must
        // steer the operator toward USB / WSL passthrough, not some
        // generic "IO error".  The specific check here is that the
        // "not found" branch produced the message, not the permissions
        // branch.
        let result = validate_device_available("/dev/definitely-not-a-real-video-device-12345");
        let err = result.expect_err("missing device must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "expected 'not found' guidance, got: {msg}"
        );
        assert!(
            msg.contains("USB") || msg.contains("WSL"),
            "expected USB/WSL guidance, got: {msg}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_validate_device_available_rejects_non_char_device() {
        // A regular file at a path shouldn't be accepted as a v4l2
        // device — FFmpeg would happily try to open it as a media
        // file and fail in confusing ways.
        let tmp = std::env::temp_dir().join("opensentry_test_not_a_video_device");
        std::fs::write(&tmp, b"this is not a video device").unwrap();
        let result = validate_device_available(tmp.to_str().unwrap());
        std::fs::remove_file(&tmp).ok();

        let err = result.expect_err("regular file must be rejected");
        assert!(
            err.to_string().contains("not a character device"),
            "expected char-device rejection, got: {err}"
        );
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn test_validate_device_available_is_noop_on_non_linux() {
        // DShow names and AVFoundation indices have no filesystem
        // presence to probe — the validator must pass through without
        // touching the disk.
        assert!(validate_device_available("Integrated Camera").is_ok());
        assert!(validate_device_available("0").is_ok());
        assert!(validate_device_available("").is_ok());
    }
}
