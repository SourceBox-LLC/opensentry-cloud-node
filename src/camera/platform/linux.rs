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
//! Linux camera detection using v4l2 API
//!
//! Scans for USB cameras on Linux systems by checking /dev/video* devices.

use std::fs;
use std::path::Path;

use super::CameraDetector;
use crate::camera::types::CameraCapabilities;
use crate::camera::DetectedCamera;
use crate::error::Result;

pub struct LinuxDetector;

impl LinuxDetector {
    pub fn new() -> Self {
        Self
    }
}

impl CameraDetector for LinuxDetector {
    fn detect_cameras(&self) -> Result<Vec<DetectedCamera>> {
        let mut cameras = Vec::new();

        tracing::info!("Scanning for USB cameras on Linux (v4l2)...");

        for i in 0..10u32 {
            let path = format!("/dev/video{}", i);

            if !Path::new(&path).exists() {
                continue;
            }

            match probe_camera(&path) {
                Ok(camera) => {
                    tracing::info!("Detected camera: {} at {}", camera.name, camera.device_path);
                    cameras.push(camera);
                }
                Err(e) => {
                    tracing::debug!("Skipping {}: {}", path, e);
                }
            }
        }

        tracing::info!("Found {} camera(s)", cameras.len());
        Ok(cameras)
    }

    fn platform_name(&self) -> &'static str {
        "Linux (v4l2)"
    }
}

/// Probe a single camera device for information
fn probe_camera(device_path: &str) -> Result<DetectedCamera> {
    // Get camera name from sysfs
    let name = get_device_name(device_path)?;

    // Check if this is a video capture device (not just a metadata device)
    if !is_capture_device(device_path)? {
        return Err(crate::error::Error::Camera(
            "Not a video capture device".into(),
        ));
    }

    // Try common resolutions to find supported ones
    let supported_resolutions = get_supported_resolutions(device_path);

    // Choose preferred resolution (prefer 1080p, then 720p, then 480p)
    let preferred_resolution = choose_preferred_resolution(&supported_resolutions);

    Ok(DetectedCamera {
        device_path: device_path.to_string(),
        name,
        capabilities: CameraCapabilities {
            streaming: true,
            hardware_encoding: check_hardware_encoding(device_path),
            formats: vec!["YUYV".to_string(), "MJPG".to_string()],
        },
        supported_resolutions,
        preferred_resolution,
    })
}

/// Get camera name from sysfs
fn get_device_name(device_path: &str) -> Result<String> {
    // Extract video number from device path
    let video_num = device_path
        .trim_start_matches("/dev/video")
        .parse::<u32>()
        .map_err(|_| crate::error::Error::Camera("Invalid device path".into()))?;

    // Read name from sysfs
    let sysfs_path = format!("/sys/class/video4linux/video{}/name", video_num);

    let name = match fs::read_to_string(&sysfs_path) {
        Ok(content) => content.trim().to_string(),
        Err(e) => {
            tracing::debug!("Cannot read {}: {}", sysfs_path, e);
            format!("USB Camera {}", video_num)
        }
    };

    Ok(name)
}

/// Check if device is a video capture device
fn is_capture_device(device_path: &str) -> Result<bool> {
    // Linux V4L2 capability: device_caps & V4L2_CAP_VIDEO_CAPTURE
    // For now, assume it is if we can read basic info
    let video_num = device_path
        .trim_start_matches("/dev/video")
        .parse::<u32>()
        .map_err(|_| crate::error::Error::Camera("Invalid device path".into()))?;

    let caps_path = format!("/sys/class/video4linux/video{}/dev_caps", video_num);

    match fs::read_to_string(&caps_path) {
        Ok(content) => {
            // Check if it's a capture device
            Ok(content.contains("capture"))
        }
        Err(_) => {
            // If we can't read caps, assume it's valid
            Ok(true)
        }
    }
}

/// Get supported resolutions for a camera
fn get_supported_resolutions(_device_path: &str) -> Vec<(u32, u32)> {
    // Common USB camera resolutions
    // In a full implementation, we'd query this from the device
    vec![
        (1920, 1080), // 1080p
        (1280, 720),  // 720p
        (640, 480),   // VGA
        (320, 240),   // QVGA
    ]
}

/// Choose the best resolution from supported list
fn choose_preferred_resolution(supported: &[(u32, u32)]) -> (u32, u32) {
    // Priority: 1080p > 720p > 480p
    let preferred_order = [(1920, 1080), (1280, 720), (640, 480)];

    for &res in &preferred_order {
        if supported.contains(&res) {
            return res;
        }
    }

    // Fall back to first supported, or 720p default
    supported.first().copied().unwrap_or((1280, 720))
}

/// Check if device supports hardware encoding
fn check_hardware_encoding(_device_path: &str) -> bool {
    // Check for hardware H.264 encoder (common on Raspberry Pi cameras)
    // For now, assume false for USB cameras
    // Raspberry Pi camera module would have this enabled
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_cameras_runs() {
        // This will only work on Linux with actual cameras
        // On other platforms, it should return an empty vector
        let detector = LinuxDetector::new();
        let result = detector.detect_cameras();
        // Just ensure it doesn't panic
        assert!(result.is_ok());
    }

    #[test]
    fn test_choose_preferred_resolution() {
        let supported = vec![(1920, 1080), (1280, 720), (640, 480)];
        let res = choose_preferred_resolution(&supported);
        assert_eq!(res, (1920, 1080));

        let supported = vec![(640, 480)];
        let res = choose_preferred_resolution(&supported);
        assert_eq!(res, (640, 480));

        let supported: Vec<(u32, u32)> = vec![];
        let res = choose_preferred_resolution(&supported);
        assert_eq!(res, (1280, 720)); // Default
    }
}
