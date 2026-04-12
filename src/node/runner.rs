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
//! Node Runner
//!
//! Main orchestration: camera detection, registration, streaming, and the
//! live dashboard that replaces raw log output while the node is running.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use colored::Colorize;
use crate::config::Config;
use crate::error::Result;
use crate::camera::{self, DetectedCamera};
use crate::api::ApiClient;
use crate::storage::NodeDatabase;
use crate::server::HttpServer;
use crate::streaming::{HlsGenerator, HlsGeneratorConfig, HlsUploader, HlsUploaderConfig};
use crate::dashboard::{CameraState, CameraStatus, Dashboard};

/// Main node orchestrator
pub struct Node {
    config: Config,
    api_client: ApiClient,
    hls_output_dir: PathBuf,
    db: NodeDatabase,
}

/// Running camera stream
struct RunningStream {
    generator: HlsGenerator,
    upload_handle: tokio::task::JoinHandle<()>,
}

impl Node {
    pub async fn new(config: Config) -> Result<Self> {
        let api_client = ApiClient::new(&config.cloud.api_url, &config.cloud.api_key)?;
        let storage_path = PathBuf::from(&config.storage.path);
        std::fs::create_dir_all(&storage_path)?;
        let db = NodeDatabase::new(&storage_path.join("node.db"))?;
        let hls_output_dir = storage_path.join("hls");
        std::fs::create_dir_all(&hls_output_dir)?;

        Ok(Self {
            config,
            api_client,
            db,
            hls_output_dir,
        })
    }

    /// Run the node with live dashboard
    pub async fn run(mut self) -> Result<()> {
        // ── Create dashboard ────────────────────────────────────────────────
        let node_id = self.config.node.node_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let dash = Dashboard::new(&node_id, &self.config.cloud.api_url);
        dash.set_settings(crate::dashboard::SettingsInfo {
            node_name: self.config.node.name.clone(),
            storage_path: self.config.storage.path.clone(),
            max_size_gb: self.config.storage.max_size_gb,
            segment_duration: self.config.streaming.hls.segment_duration,
            fps: self.config.streaming.fps,
            encoder: self.config.streaming.encoder.clone(),
            hls_enabled: self.config.streaming.hls.enabled,
            heartbeat_interval: self.config.cloud.heartbeat_interval,
            motion_enabled: self.config.motion.enabled,
            motion_sensitivity: self.config.motion.sensitivity,
            motion_cooldown: self.config.motion.cooldown_secs,
        });
        dash.set_db(self.db.clone(), self.hls_output_dir.clone());
        dash.load_logs_from_db();

        // Install dashboard into the tracing layer so all tracing::info!() etc.
        // calls throughout the codebase flow into the TUI + SQLite.
        crate::logging::set_dashboard(dash.clone());

        // ── 1. Detect cameras ────────────────────────────────────────────────
        dash.log_info("Detecting cameras…");
        let detected_cameras = self.detect_cameras(&dash).await?;

        // Register cameras in dashboard
        for cam in &detected_cameras {
            dash.add_camera(CameraState {
                name: cam.name.clone(),
                resolution: format!("{}×{}", cam.preferred_resolution.0, cam.preferred_resolution.1),
                video_codec: String::new(),
                audio_codec: String::new(),
                status: CameraStatus::Starting,
                segments_uploaded: 0,
                bytes_uploaded: 0,
            });
        }

        // ── 2. Register with cloud ────────────────────────────────────────────
        dash.log_info("Registering with cloud…");
        let registration = self.register_with_cloud(&detected_cameras, &dash).await?;
        let node_id = registration.node_id.clone();
        let camera_mapping: HashMap<String, String> = registration.cameras;
        dash.log_info(format!("Registered as node {}", node_id.cyan().bold()));

        // ── 3. Start HLS streams ──────────────────────────────────────────────
        let mut running_streams: Vec<RunningStream> = Vec::new();
        let mut cameras_with_hls: Vec<(String, PathBuf)> = Vec::new();
        let recording_state: Arc<RwLock<HashSet<String>>> =
            Arc::new(RwLock::new(HashSet::new()));

        // Motion event channel — uploaders send, WebSocket client receives
        let (motion_tx, motion_rx) = tokio::sync::mpsc::channel::<
            crate::streaming::hls_uploader::MotionEvent,
        >(64);

        // Clean all HLS directories on startup so segment numbering resets fresh
        if let Ok(entries) = std::fs::read_dir(&self.hls_output_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }

        // Detect encoder once (not per-camera) and persist it
        if self.config.streaming.encoder.is_empty() {
            let ffmpeg_path = crate::streaming::HlsGenerator::find_ffmpeg();
            if let Some(enc) = crate::streaming::HlsGenerator::detect_hw_encoder(&ffmpeg_path) {
                dash.log_info(format!("Hardware encoder detected: {}", enc.cyan()));
                self.config.streaming.encoder = enc.clone();
                // Save to DB so future runs skip auto-detect
                if let Err(e) = self.db.set_config("encoder", &enc) {
                    tracing::warn!("Failed to save encoder to DB: {}", e);
                }
            } else {
                dash.log_info("No hardware encoder found, using software (libx264)");
                self.config.streaming.encoder = "libx264".to_string();
                let _ = self.db.set_config("encoder", "libx264");
            }
        }

        // Update settings display with detected encoder
        dash.set_settings(crate::dashboard::SettingsInfo {
            node_name: self.config.node.name.clone(),
            storage_path: self.config.storage.path.clone(),
            max_size_gb: self.config.storage.max_size_gb,
            segment_duration: self.config.streaming.hls.segment_duration,
            fps: self.config.streaming.fps,
            encoder: self.config.streaming.encoder.clone(),
            hls_enabled: self.config.streaming.hls.enabled,
            heartbeat_interval: self.config.cloud.heartbeat_interval,
            motion_enabled: self.config.motion.enabled,
            motion_sensitivity: self.config.motion.sensitivity,
            motion_cooldown: self.config.motion.cooldown_secs,
        });

        for detected in detected_cameras {
            let camera_id = camera_mapping.get(&detected.device_path)
                .cloned()
                .unwrap_or_else(|| {
                    let sanitized = detected.device_path
                        .replace("/", "_")
                        .replace("\\", "_")
                        .replace(" ", "_")
                        .trim_start_matches('_')
                        .to_string();
                    format!("{}_{}", node_id, sanitized)
                });

            let camera_hls_dir = self.hls_output_dir.join(&camera_id);
            std::fs::create_dir_all(&camera_hls_dir)?;

            if self.config.streaming.hls.enabled {
                let mut hls_config = HlsGeneratorConfig::from(self.config.streaming.hls.clone());
                hls_config.output_dir = camera_hls_dir.clone();
                hls_config.fps = self.config.streaming.fps;
                hls_config.encoder = self.config.streaming.encoder.clone();

                let mut generator = HlsGenerator::new(hls_config)?;

                let started = match generator.start_from_device(&detected.device_path) {
                    Ok(_) => {
                        dash.log_info(format!("Started HLS for {}", detected.name.cyan()));
                        dash.update_camera_status(&detected.name, CameraStatus::Streaming);
                        true
                    }
                    Err(e) => {
                        dash.log_warn(format!("Camera {} failed ({}), using test pattern", detected.name, e));
                        match generator.start_from_frames(
                            self.config.streaming.hls.segment_duration * 100,
                            self.config.streaming.hls.segment_duration * 75,
                            self.config.streaming.fps,
                        ) {
                            Ok(_) => {
                                dash.update_camera_status(&detected.name, CameraStatus::Streaming);
                                true
                            }
                            Err(e2) => {
                                dash.log_error(format!("Failed to start test pattern: {}", e2));
                                dash.update_camera_status(&detected.name, CameraStatus::Error(e2.to_string()));
                                false
                            }
                        }
                    }
                };

                if started {
                    cameras_with_hls.push((camera_id.clone(), camera_hls_dir.clone()));

                    // Build uploader with dashboard reference
                    let uploader_config = HlsUploaderConfig::new(camera_id.clone(), camera_hls_dir);
                    let cam_name = detected.name.clone();
                    let uploader = HlsUploader::new(
                        uploader_config,
                        self.api_client.clone(),
                        recording_state.clone(),
                        self.db.clone(),
                        self.config.motion.clone(),
                        motion_tx.clone(),
                    );
                    let dash_clone = dash.clone();
                    let camera_id_clone = camera_id.clone();

                    let upload_handle = tokio::spawn(async move {
                        if let Err(e) = uploader.start_with_dashboard(dash_clone, cam_name, camera_id_clone).await {
                            tracing::error!("HLS uploader error: {}", e);
                        }
                    });

                    running_streams.push(RunningStream { generator, upload_handle });
                }
            }
        }

        // ── 4. Start HTTP server ──────────────────────────────────────────────
        let camera_map: HashMap<String, PathBuf> = cameras_with_hls.into_iter().collect();
        let http_server = self.create_http_server_with_hls(camera_map);
        tokio::spawn(async move {
            if let Err(e) = http_server.run().await {
                tracing::error!("HTTP server error: {}", e);
            }
        });

        dash.log_info(format!(
            "Node online — streaming {} camera(s)",
            running_streams.len()
        ));

        // ── 5. Start heartbeat + WebSocket ───────────────────────────────────
        // Collect camera IDs so the heartbeat can report them as "streaming"
        let streaming_camera_ids: Vec<String> = camera_mapping.values().cloned().collect();
        let heartbeat_handle = self.start_heartbeat_loop(dash.clone(), streaming_camera_ids.clone());

        // Start WebSocket command channel (runs alongside HTTP heartbeats)
        let ws_handle = {
            let api_url = self.config.cloud.api_url.clone();
            let api_key = self.config.cloud.api_key.clone();
            let ws_node_id = node_id.clone();
            let ws_camera_ids = streaming_camera_ids;
            let ws_interval = self.config.cloud.heartbeat_interval;
            let ws_dash = dash.clone();
            let ws_hls_dir = self.hls_output_dir.clone();
            let ws_db = self.db.clone();
            let ws_rec_state = recording_state.clone();
            let ws_motion_cooldown = self.config.motion.cooldown_secs;
            tokio::spawn(async move {
                crate::api::websocket::run_ws_client(
                    api_url,
                    api_key,
                    ws_node_id,
                    ws_camera_ids,
                    ws_interval,
                    ws_dash,
                    ws_hls_dir,
                    ws_db,
                    ws_rec_state,
                    motion_rx,
                    ws_motion_cooldown,
                ).await;
            })
        };

        // ── 6. Start retention cleanup (enforce max_size_gb) ──────────────────
        let retention_handle = {
            let ret_db = self.db.clone();
            let max_bytes = self.config.storage.max_size_gb * 1024 * 1024 * 1024;
            let ret_dash = dash.clone();
            tokio::spawn(async move {
                let interval = tokio::time::Duration::from_secs(5 * 60); // every 5 minutes
                loop {
                    tokio::time::sleep(interval).await;
                    match ret_db.enforce_retention(max_bytes) {
                        Ok((current, freed)) => {
                            if freed > 0 {
                                let freed_mb = freed / (1024 * 1024);
                                let current_gb = current / (1024 * 1024 * 1024);
                                ret_dash.log_info(format!(
                                    "Retention: freed {} MB, now {} GB", freed_mb, current_gb
                                ));
                            }
                        }
                        Err(e) => {
                            ret_dash.log_warn(format!("Retention check failed: {}", e));
                        }
                    }
                    // Prune old log entries (keep last 10,000)
                    let _ = ret_db.prune_logs(10_000);
                }
            })
        };

        // ── 7. Start dashboard render loop in a background thread ─────────────
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_flag.clone();
        let dash_clone = dash.clone();
        let render_thread = std::thread::spawn(move || {
            dash_clone.run_render_loop(Duration::from_millis(500), stop_clone);
        });

        // ── 8. Wait for shutdown (Ctrl+C from OS or /quit from dashboard) ────
        // In raw mode Ctrl+C is captured by the dashboard input loop and sets
        // the stop flag directly, so we poll it alongside the OS signal.
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                dash.log_warn("Shutdown signal received — stopping…");
                stop_flag.store(true, Ordering::Relaxed);
            }
            _ = async {
                while !stop_flag.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            } => {}
        }

        // Give dashboard one last render cycle to show shutdown message
        std::thread::sleep(Duration::from_millis(600));
        let _ = render_thread.join();

        // Stop streams
        for stream in running_streams {
            drop(stream.generator);
            stream.upload_handle.abort();
        }
        heartbeat_handle.abort();
        ws_handle.abort();
        retention_handle.abort();

        println!("\n  {}", "CloudNode stopped.".yellow());

        Ok(())
    }

    /// Run once (test mode)
    pub async fn run_once(mut self) -> Result<()> {
        let dash = Dashboard::new(
            self.config.node.node_id.as_deref().unwrap_or("test"),
            &self.config.cloud.api_url,
        );

        let detected_cameras = self.detect_cameras(&dash).await?;
        for cam in &detected_cameras {
            println!("  Camera: {} ({})", cam.name.cyan(), cam.device_path);
            println!("    Resolution: {}x{}", cam.preferred_resolution.0, cam.preferred_resolution.1);
        }

        match self.register_with_cloud(&detected_cameras, &dash).await {
            Ok(reg) => println!("  Registered: {}", reg.node_id.green()),
            Err(e)  => println!("  API not available: {}", e),
        }

        println!("\n  Testing heartbeat…");
        match self.test_heartbeat().await {
            Ok(r) => {
                println!("  Heartbeat: {}", r.timestamp.green());
                if r.key_rotated {
                    println!("  {} API key was rotated — update your config!", "⚠".yellow());
                }
            }
            Err(e) => println!("  Heartbeat failed: {}", e),
        }

        Ok(())
    }

    fn start_heartbeat_loop(&self, dash: Dashboard, camera_ids: Vec<String>) -> tokio::task::JoinHandle<()> {
        let api_url = self.config.cloud.api_url.clone();
        let api_key = self.config.cloud.api_key.clone();
        let node_id = self.config.node.node_id.clone();
        let interval = self.config.cloud.heartbeat_interval;

        tokio::spawn(async move {
            let mut client = match ApiClient::new(&api_url, &api_key) {
                Ok(c) => c,
                Err(e) => {
                    dash.log_error(format!("Heartbeat client error: {}", e));
                    return;
                }
            };
            if let Some(id) = &node_id {
                client.set_node_id(id.clone());
            }
            let local_ip = get_local_ip();

            // Report all cameras as "streaming" in each heartbeat
            let camera_statuses: Vec<(String, String)> = camera_ids
                .iter()
                .map(|id| (id.clone(), "streaming".to_string()))
                .collect();

            loop {
                match client.heartbeat_with_retry(local_ip.as_deref(), camera_statuses.clone(), 3).await {
                    Ok(r) => {
                        if r.key_rotated {
                            if let Some(new_key) = r.new_api_key {
                                dash.log_warn("API key rotated by server — updating");
                                client.update_api_key(new_key);
                            }
                        }
                        // Heartbeat success is silent — no log noise every N seconds
                    }
                    Err(e) => dash.log_error(format!("Heartbeat failed: {}", e)),
                }
                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
        })
    }

    async fn test_heartbeat(&self) -> Result<crate::api::HeartbeatResponse> {
        let mut client = ApiClient::new(&self.config.cloud.api_url, &self.config.cloud.api_key)?;
        if let Some(id) = &self.config.node.node_id {
            client.set_node_id(id.clone());
        }
        client.heartbeat_with_retry(get_local_ip().as_deref(), vec![], 3).await
    }

    async fn detect_cameras(&self, dash: &Dashboard) -> Result<Vec<DetectedCamera>> {
        let cameras = camera::detect_cameras()?;
        if cameras.is_empty() {
            dash.log_warn("No cameras detected");
        } else {
            dash.log_info(format!("Detected {} camera(s)", cameras.len()));
        }
        Ok(cameras)
    }

    async fn register_with_cloud(
        &mut self,
        cameras: &[DetectedCamera],
        dash: &Dashboard,
    ) -> Result<crate::api::RegisterResponse> {
        let camera_infos: Vec<crate::api::CameraInfo> = cameras.iter().map(|c| c.clone().into()).collect();

        let node_id = self.config.node.node_id.as_ref().ok_or_else(|| {
            let error = crate::setup::recovery::RegistrationError::ConfigError {
                message: "Node ID not configured. Run setup first.".to_string(),
            };
            let _ = crate::setup::recovery::show_registration_error(&error);
            crate::error::Error::Config("Node ID not configured".into())
        })?;

        let codec_info = if let Some(cam) = cameras.first() {
            match crate::streaming::codec_detector::detect_codec_from_camera(&cam.device_path) {
                Ok(info) => {
                    dash.log_info(format!(
                        "Codec detected: {} / {}",
                        info.video_codec.cyan(),
                        info.audio_codec.cyan()
                    ));
                    Some(info)
                }
                Err(e) => {
                    dash.log_warn(format!("Codec detection failed: {}", e));
                    None
                }
            }
        } else {
            None
        };

        match self.api_client.register(
            node_id,
            &self.config.node.name,
            camera_infos,
            codec_info.as_ref().map(|c| c.video_codec.as_str()),
            codec_info.as_ref().map(|c| c.audio_codec.as_str()),
        ).await {
            Ok(r) => Ok(r),
            Err(e) => {
                let msg = e.to_string();
                let reg_err = if msg.contains("404") || msg.contains("not found") {
                    crate::setup::recovery::RegistrationError::InvalidNodeId { node_id: node_id.clone() }
                } else if msg.contains("401") || msg.contains("403") {
                    crate::setup::recovery::RegistrationError::InvalidApiKey { node_id: node_id.clone() }
                } else if msg.contains("connect") || msg.contains("timeout") {
                    crate::setup::recovery::RegistrationError::NetworkError { message: msg.clone() }
                } else {
                    crate::setup::recovery::RegistrationError::ServerError { code: 0, message: msg.clone() }
                };
                let _ = crate::setup::recovery::show_registration_error(&reg_err);
                Err(e)
            }
        }
    }

    fn create_http_server_with_hls(&self, hls_cameras: HashMap<String, PathBuf>) -> HttpServer {
        HttpServer::new_with_hls(
            self.config.server.clone(),
            PathBuf::from(&self.config.storage.path),
            hls_cameras,
        )
    }
}

fn get_local_ip() -> Option<String> {
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}
