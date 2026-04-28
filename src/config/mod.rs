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
//! Configuration loading and parsing

mod settings;

pub use settings::*;

use crate::error::{Error, Result};
use crate::storage::NodeDatabase;

/// CLI argument overrides (passed from main)
#[derive(Debug, Default)]
pub struct CliOverrides {
    pub node_id: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
}

impl Config {
    /// Load configuration: try DB first, then fall back to YAML/env.
    pub fn load(path: Option<&str>) -> Result<Self> {
        // 1. Try loading from the SQLite database (new path).
        //
        // Path resolution lives in `crate::paths::config_db_path()` so a
        // Windows-Service install (cwd = System32) finds its DB under
        // %ProgramData%\OpenSentry\ instead of trying to write inside
        // System32. See paths::data_dir() for the full resolution order.
        let db_path = crate::paths::config_db_path();
        if db_path.exists() {
            if let Ok(db) = NodeDatabase::new(&db_path) {
                if db.has_config() {
                    tracing::info!("Loading config from database");
                    return Self::load_from_db(&db);
                }
            }
        }

        // 2. Fall back to YAML file + env vars (legacy path)
        let mut config = if let Some(p) = path {
            Self::from_file(p)?
        } else {
            Self::from_default_locations()?
        };

        config = config.with_env_overrides();

        // 3. If we loaded from legacy sources and have credentials,
        //    migrate them to the database for next time. Make sure the
        //    parent dir exists — on a fresh MSI install %ProgramData%\
        //    OpenSentry\ may not exist yet because the WiX template
        //    only creates the install dir, not the data dir.
        if !config.cloud.api_key.is_empty() && config.node.node_id.is_some() {
            if let Some(parent) = db_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(db) = NodeDatabase::new(&db_path) {
                if let Err(e) = config.save_to_db(&db) {
                    tracing::warn!("Config migration to DB failed: {}", e);
                } else {
                    tracing::info!("Config migrated to database (API key encrypted)");
                }
            }
        }

        Ok(config)
    }

    /// Load all config values from the SQLite database.
    pub fn load_from_db(db: &NodeDatabase) -> Result<Self> {
        let mut config = Config::default();

        if let Some(v) = db.get_config("node_id")? {
            config.node.node_id = Some(v);
        }
        if let Some(v) = db.get_config("node_name")? {
            config.node.name = v;
        }
        // API key is encrypted at rest
        if let Some(v) = db.get_config_encrypted("api_key")? {
            config.cloud.api_key = v;
        }
        if let Some(v) = db.get_config("api_url")? {
            config.cloud.api_url = v;
        }
        if let Some(v) = db.get_config("heartbeat_interval")? {
            config.cloud.heartbeat_interval = v.parse().unwrap_or(30);
        }
        if let Some(v) = db.get_config("storage_path")? {
            config.storage.path = v;
        }
        if let Some(v) = db.get_config("max_size_gb")? {
            config.storage.max_size_gb = v.parse().unwrap_or(64);
        }
        if let Some(v) = db.get_config("fps")? {
            config.streaming.fps = v.parse().unwrap_or(30);
        }
        if let Some(v) = db.get_config("encoder")? {
            config.streaming.encoder = v;
        }
        if let Some(v) = db.get_config("segment_duration")? {
            config.streaming.hls.segment_duration = v.parse().unwrap_or(1);
        }
        if let Some(v) = db.get_config("hls_enabled")? {
            config.streaming.hls.enabled = v == "true";
        }
        if let Some(v) = db.get_config("bitrate")? {
            config.streaming.hls.bitrate = v;
        }
        if let Some(v) = db.get_config("server_port")? {
            config.server.port = v.parse().unwrap_or(8080);
        }
        if let Some(v) = db.get_config("log_level")? {
            config.logging.level = v;
        }
        if let Some(v) = db.get_config("motion_enabled")? {
            config.motion.enabled = v == "true";
        }
        if let Some(v) = db.get_config("motion_sensitivity")? {
            config.motion.threshold = v.parse().unwrap_or(0.02);
        }
        if let Some(v) = db.get_config("motion_cooldown")? {
            config.motion.cooldown_secs = v.parse().unwrap_or(30);
        }

        // Still allow env vars to override DB values (useful for debugging)
        config = config.with_env_overrides();

        Ok(config)
    }

    /// Save all config values to the SQLite database.
    /// The API key is encrypted at rest.
    pub fn save_to_db(&self, db: &NodeDatabase) -> Result<()> {
        if let Some(ref id) = self.node.node_id {
            db.set_config("node_id", id)?;
        }
        db.set_config("node_name", &self.node.name)?;
        db.set_config("api_url", &self.cloud.api_url)?;
        db.set_config("heartbeat_interval", &self.cloud.heartbeat_interval.to_string())?;
        db.set_config("storage_path", &self.storage.path)?;
        db.set_config("max_size_gb", &self.storage.max_size_gb.to_string())?;
        db.set_config("fps", &self.streaming.fps.to_string())?;
        if !self.streaming.encoder.is_empty() {
            db.set_config("encoder", &self.streaming.encoder)?;
        }
        db.set_config("segment_duration", &self.streaming.hls.segment_duration.to_string())?;
        db.set_config("hls_enabled", if self.streaming.hls.enabled { "true" } else { "false" })?;
        db.set_config("bitrate", &self.streaming.hls.bitrate)?;
        db.set_config("server_port", &self.server.port.to_string())?;
        db.set_config("log_level", &self.logging.level)?;
        db.set_config("motion_enabled", if self.motion.enabled { "true" } else { "false" })?;
        db.set_config("motion_sensitivity", &self.motion.threshold.to_string())?;
        db.set_config("motion_cooldown", &self.motion.cooldown_secs.to_string())?;

        // Encrypt the API key
        if !self.cloud.api_key.is_empty() {
            db.set_config_encrypted("api_key", &self.cloud.api_key)?;
        }

        Ok(())
    }

    fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("Failed to read config file {}: {}", path, e)))?;

        Self::parse_yaml(&content)
    }

    fn from_default_locations() -> Result<Self> {
        let default_paths = vec![
            "./config.yaml",
            "./config.yml",
            "/etc/sourcebox-sentry/config.yaml",
            "/etc/sourcebox-sentry/config.yml",
        ];

        for path in default_paths {
            if std::path::Path::new(path).exists() {
                tracing::info!("Loading config from {}", path);
                return Self::from_file(path);
            }
        }

        tracing::info!("No config file found, using defaults");
        Ok(Self::default())
    }

    fn parse_yaml(content: &str) -> Result<Self> {
        let docs = yaml_rust::YamlLoader::load_from_str(content)?;
        let doc = docs
            .first()
            .ok_or_else(|| Error::Config("Empty config file".into()))?;

        let mut config = Config::default();

        // Parse node config
        let node = &doc["node"];
        if !node.is_badvalue() {
            if let Some(name) = node["name"].as_str() {
                config.node.name = name.to_string();
            }
        }

        // Parse cloud config
        let cloud = &doc["cloud"];
        if !cloud.is_badvalue() {
            if let Some(api_url) = cloud["api_url"].as_str() {
                config.cloud.api_url = api_url.to_string();
            }
            if let Some(heartbeat_interval) = cloud["heartbeat_interval"].as_i64() {
                config.cloud.heartbeat_interval = heartbeat_interval as u64;
            }
        }

        // Parse cameras config
        let cameras = &doc["cameras"];
        if !cameras.is_badvalue() {
            if let Some(auto_detect) = cameras["auto_detect"].as_bool() {
                config.cameras.auto_detect = auto_detect;
            }
            if let Some(devices) = cameras["devices"].as_vec() {
                config.cameras.devices = devices
                    .iter()
                    .filter_map(|d| d.as_str().map(|s| s.to_string()))
                    .collect();
            }
        }

        // Parse streaming config
        let streaming = &doc["streaming"];
        if !streaming.is_badvalue() {
            if let Some(fps) = streaming["fps"].as_i64() {
                config.streaming.fps = fps as u32;
            }
            if let Some(jpeg_quality) = streaming["jpeg_quality"].as_i64() {
                config.streaming.jpeg_quality = jpeg_quality as u8;
            }
            // Parse HLS config
            let hls = &streaming["hls"];
            if !hls.is_badvalue() {
                if let Some(enabled) = hls["enabled"].as_bool() {
                    config.streaming.hls.enabled = enabled;
                }
                if let Some(segment_duration) = hls["segment_duration"].as_i64() {
                    config.streaming.hls.segment_duration = segment_duration as u32;
                }
                if let Some(playlist_size) = hls["playlist_size"].as_i64() {
                    config.streaming.hls.playlist_size = playlist_size as u32;
                }
                if let Some(bitrate) = hls["bitrate"].as_str() {
                    config.streaming.hls.bitrate = bitrate.to_string();
                }
            }
        }

        // Parse storage config
        let storage = &doc["storage"];
        if !storage.is_badvalue() {
            if let Some(path) = storage["path"].as_str() {
                config.storage.path = path.to_string();
            }
            if let Some(max_size_gb) = storage["max_size_gb"].as_i64() {
                config.storage.max_size_gb = max_size_gb as u64;
            }
        }

        // Parse server config
        let server = &doc["server"];
        if !server.is_badvalue() {
            if let Some(port) = server["port"].as_i64() {
                config.server.port = port as u16;
            }
            if let Some(bind) = server["bind"].as_str() {
                config.server.bind = bind.to_string();
            }
        }

        // Parse logging config
        let logging = &doc["logging"];
        if !logging.is_badvalue() {
            if let Some(level) = logging["level"].as_str() {
                config.logging.level = level.to_string();
            }
        }

        Ok(config)
    }

    fn with_env_overrides(mut self) -> Self {
        // Node ID from env
        if let Ok(node_id) = std::env::var("SOURCEBOX_SENTRY_NODE_ID") {
            self.node.node_id = Some(node_id);
        }

        // API key from env
        if let Ok(key) = std::env::var("SOURCEBOX_SENTRY_API_KEY") {
            self.cloud.api_key = key;
        }

        // API URL from env
        if let Ok(url) = std::env::var("SOURCEBOX_SENTRY_API_URL") {
            self.cloud.api_url = url;
        }

        // Encoder from env (legacy: was only in .env, not in Config)
        if let Ok(enc) = std::env::var("SOURCEBOX_SENTRY_ENCODER") {
            self.streaming.encoder = enc;
        }

        // Log level from env
        if let Ok(level) = std::env::var("RUST_LOG") {
            self.logging.level = level;
        }

        self
    }

    /// Apply CLI argument overrides
    pub fn with_overrides(mut self, overrides: CliOverrides) -> Self {
        if let Some(node_id) = overrides.node_id {
            self.node.node_id = Some(node_id);
        }
        if let Some(api_key) = overrides.api_key {
            self.cloud.api_key = api_key;
        }
        if let Some(api_url) = overrides.api_url {
            self.cloud.api_url = api_url;
        }
        self
    }
}
