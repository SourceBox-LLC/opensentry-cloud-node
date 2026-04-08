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
//! Platform-specific camera detection
//!
//! This module provides platform-agnostic camera detection through platform-specific
//! implementations for Linux (v4l2), Windows (DirectShow), and macOS (AVFoundation).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use crate::camera::DetectedCamera;
use crate::error::Result;

/// Platform-agnostic camera detector trait
pub trait CameraDetector: Send + Sync {
    /// Detect all available cameras
    fn detect_cameras(&self) -> Result<Vec<DetectedCamera>>;

    /// Get platform name for display
    fn platform_name(&self) -> &'static str;
}

/// Create platform-specific detector
pub fn create_detector() -> Box<dyn CameraDetector> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxDetector::new())
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsDetector::new())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOSDetector::new())
    }
}

/// Check if device path is valid for current platform
pub fn is_valid_device_path(_path: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        _path.starts_with("/dev/video")
    }

    #[cfg(target_os = "windows")]
    {
        // DirectShow uses device names, not paths
        true
    }

    #[cfg(target_os = "macos")]
    {
        // AVFoundation uses numeric indices as strings
        _path.parse::<u32>().is_ok()
    }
}
