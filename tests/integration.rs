//! Integration tests for SourceBox Sentry CloudNode

use sourcebox_sentry_cloudnode::{Config, Result};

#[test]
fn test_config_load_default() -> Result<()> {
    let config = Config::load(None)?;
    assert!(!config.node.name.is_empty());
    assert!(!config.cloud.api_url.is_empty());
    Ok(())
}

#[test]
fn test_camera_detect() {
    // detect_cameras() shells out to FFmpeg on Windows + macOS for
    // device enumeration. v0.1.35 onward CloudNode uses the system
    // FFmpeg (no bundled fallback), so on a test environment without
    // FFmpeg on PATH the call returns Err — that's fine, we're just
    // verifying it doesn't panic.
    match sourcebox_sentry_cloudnode::camera::detect_cameras() {
        Ok(cameras) => println!("Detected {} cameras", cameras.len()),
        Err(e) => println!("Camera detect skipped (no FFmpeg on PATH?): {}", e),
    }
}