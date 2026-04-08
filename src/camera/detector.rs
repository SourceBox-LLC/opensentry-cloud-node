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

pub use crate::camera::platform::{create_detector, is_valid_device_path, CameraDetector};

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
}
