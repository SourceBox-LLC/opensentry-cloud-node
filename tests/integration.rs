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
fn test_camera_detect() -> Result<()> {
    let cameras = sourcebox_sentry_cloudnode::camera::detect_cameras()?;
    // Should not panic, but may be empty on non-Linux systems
    println!("Detected {} cameras", cameras.len());
    Ok(())
}