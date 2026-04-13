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
//! WebSocket client for persistent bidirectional communication with the backend.
//!
//! Connects to `ws(s)://<backend>/ws/node?api_key=<key>&node_id=<id>` and
//! maintains the connection with auto-reconnect. Sends heartbeats over the
//! socket and listens for commands from the backend.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use serde::{Deserialize, Serialize};
use base64::prelude::*;
use crate::dashboard::Dashboard;
use crate::storage::NodeDatabase;
use crate::streaming::hls_uploader::MotionEvent;

/// JSON message sent/received over the WebSocket.
#[derive(Debug, Serialize, Deserialize)]
pub struct WsMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Runs the WebSocket client loop with auto-reconnect.
///
/// This function never returns under normal operation — it reconnects
/// with exponential backoff whenever the connection drops.
pub async fn run_ws_client(
    api_url: String,
    api_key: String,
    node_id: String,
    camera_ids: Vec<String>,
    heartbeat_interval: u64,
    dash: Dashboard,
    hls_base_dir: PathBuf,
    db: NodeDatabase,
    recording_state: Arc<RwLock<HashSet<String>>>,
    mut motion_rx: tokio::sync::mpsc::Receiver<MotionEvent>,
) {
    let ws_url = build_ws_url(&api_url, &api_key, &node_id);
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    loop {
        dash.log_info("WebSocket connecting…");

        match connect_async(&ws_url).await {
            Ok((ws_stream, _response)) => {
                dash.log_info("WebSocket connected");
                backoff = Duration::from_secs(1); // reset on success

                let (mut write, mut read) = ws_stream.split();

                // Send heartbeats on a timer, read commands from backend
                let mut heartbeat_ticker =
                    tokio::time::interval(Duration::from_secs(heartbeat_interval));

                loop {
                    tokio::select! {
                        // -- Heartbeat tick --
                        _ = heartbeat_ticker.tick() => {
                            let msg = build_heartbeat(&camera_ids);
                            let text = match serde_json::to_string(&msg) {
                                Ok(t) => t,
                                Err(e) => {
                                    tracing::warn!("Failed to serialize heartbeat: {}", e);
                                    continue;
                                }
                            };
                            if write.send(Message::Text(text)).await.is_err() {
                                dash.log_warn("WebSocket send failed — reconnecting");
                                break; // exit inner loop → reconnect
                            }
                        }

                        // -- Motion event from uploader (cooldown already applied) --
                        Some(event) = motion_rx.recv() => {
                            dash.log_info(format!(
                                "Motion detected on {} (score {}%)",
                                event.camera_id, event.score
                            ));

                            let msg = WsMessage {
                                msg_type: "event".to_string(),
                                id: None,
                                command: Some("motion_detected".to_string()),
                                payload: serde_json::json!({
                                    "camera_id": event.camera_id,
                                    "score": event.score,
                                    "timestamp": event.timestamp,
                                    "segment_seq": event.segment_seq,
                                }),
                            };
                            if let Ok(text) = serde_json::to_string(&msg) {
                                if write.send(Message::Text(text)).await.is_err() {
                                    dash.log_warn("WebSocket send failed — reconnecting");
                                    break;
                                }
                            }
                        }

                        // -- Incoming message from backend --
                        incoming = read.next() => {
                            match incoming {
                                Some(Ok(Message::Text(text))) => {
                                    if let Some(response) = handle_message(
                                        &text, &dash,
                                        &hls_base_dir, &db, &recording_state,
                                    ).await {
                                        let resp_text = serde_json::to_string(&response)
                                            .unwrap_or_default();
                                        if write.send(Message::Text(resp_text)).await.is_err() {
                                            dash.log_warn("WebSocket send failed — reconnecting");
                                            break;
                                        }
                                    }
                                }
                                Some(Ok(Message::Ping(data))) => {
                                    let _ = write.send(Message::Pong(data)).await;
                                }
                                Some(Ok(Message::Close(_))) => {
                                    dash.log_warn("WebSocket closed by server — reconnecting");
                                    break;
                                }
                                Some(Err(e)) => {
                                    dash.log_warn(format!("WebSocket error: {} — reconnecting", e));
                                    break;
                                }
                                None => {
                                    dash.log_warn("WebSocket stream ended — reconnecting");
                                    break;
                                }
                                _ => {} // Binary, Pong, etc.
                            }
                        }
                    }
                }
            }
            Err(e) => {
                dash.log_warn(format!("WebSocket connect failed: {}", e));
            }
        }

        // Exponential backoff before reconnect
        dash.log_info(format!("Reconnecting in {}s…", backoff.as_secs()));
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Build the WebSocket URL from the HTTP API URL.
fn build_ws_url(api_url: &str, api_key: &str, node_id: &str) -> String {
    let base = api_url
        .replace("https://", "wss://")
        .replace("http://", "ws://")
        .trim_end_matches('/')
        .to_string();

    format!(
        "{}/ws/node?api_key={}&node_id={}",
        base,
        urlencoded(api_key),
        urlencoded(node_id),
    )
}

/// Minimal URL-encoding for query param values.
fn urlencoded(s: &str) -> String {
    s.replace('%', "%25")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace(' ', "%20")
        .replace('+', "%2B")
}

/// Build a heartbeat message.
fn build_heartbeat(camera_ids: &[String]) -> WsMessage {
    let cameras: Vec<serde_json::Value> = camera_ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "camera_id": id,
                "status": "streaming"
            })
        })
        .collect();

    let local_ip = get_local_ip();

    WsMessage {
        msg_type: "heartbeat".to_string(),
        id: None,
        command: None,
        payload: serde_json::json!({
            "cameras": cameras,
            "local_ip": local_ip,
        }),
    }
}

/// Handle an incoming message from the backend.
/// Returns an optional response to send back over the WebSocket.
async fn handle_message(
    text: &str,
    dash: &Dashboard,
    hls_base_dir: &Path,
    db: &NodeDatabase,
    recording_state: &Arc<RwLock<HashSet<String>>>,
) -> Option<WsMessage> {
    let msg: WsMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Invalid WS message: {}", e);
            return None;
        }
    };

    match msg.msg_type.as_str() {
        "ack" => {
            // Heartbeat acknowledged — silent, no log noise
            None
        }
        "command" => {
            let cmd = msg.command.as_deref().unwrap_or("unknown");
            let msg_id = msg.id.clone();
            dash.log_info(format!("Command received: {}", cmd));

            let result = dispatch_command(
                cmd, &msg.payload, hls_base_dir, db, recording_state,
            ).await;

            let payload = match &result {
                Ok(data) => serde_json::json!({
                    "status": "success",
                    "data": data,
                }),
                Err(err) => {
                    dash.log_warn(format!("Command '{}' failed: {}", cmd, err));
                    serde_json::json!({
                        "status": "error",
                        "error": err,
                    })
                }
            };

            Some(WsMessage {
                msg_type: "command_result".to_string(),
                id: msg_id,
                command: Some(cmd.to_string()),
                payload,
            })
        }
        "error" => {
            let detail = msg.payload.get("detail")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            dash.log_warn(format!("Server error: {}", detail));
            None
        }
        other => {
            tracing::debug!("Unhandled WS message type: {}", other);
            None
        }
    }
}

/// Route a command to the appropriate handler.
async fn dispatch_command(
    cmd: &str,
    payload: &serde_json::Value,
    hls_base_dir: &Path,
    db: &NodeDatabase,
    recording_state: &Arc<RwLock<HashSet<String>>>,
) -> std::result::Result<serde_json::Value, String> {
    match cmd {
        "take_snapshot" => {
            let camera_id = payload.get("camera_id")
                .and_then(|v| v.as_str())
                .ok_or("missing camera_id")?;
            cmd_take_snapshot(camera_id, hls_base_dir, db).await
        }
        "start_recording" => {
            let camera_id = payload.get("camera_id")
                .and_then(|v| v.as_str())
                .ok_or("missing camera_id")?;
            recording_state.write().map_err(|e| e.to_string())?
                .insert(camera_id.to_string());
            tracing::info!("Recording started for camera {}", camera_id);
            Ok(serde_json::json!({"recording": true, "camera_id": camera_id}))
        }
        "stop_recording" => {
            let camera_id = payload.get("camera_id")
                .and_then(|v| v.as_str())
                .ok_or("missing camera_id")?;
            recording_state.write().map_err(|e| e.to_string())?
                .remove(camera_id);
            tracing::info!("Recording stopped for camera {}", camera_id);
            Ok(serde_json::json!({"recording": false, "camera_id": camera_id}))
        }
        "list_snapshots" => {
            let camera_id = payload.get("camera_id").and_then(|v| v.as_str());
            let snaps = db.list_snapshots(camera_id).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(&snaps).unwrap_or_default())
        }
        "list_recordings" => {
            let camera_id = payload.get("camera_id").and_then(|v| v.as_str());
            let recs = db.list_recordings(camera_id).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(&recs).unwrap_or_default())
        }
        "wipe_data" => {
            db.wipe_all().map_err(|e| e.to_string())?;
            tracing::warn!("All local data wiped by backend command");
            Ok(serde_json::json!({"wiped": true}))
        }
        other => Err(format!("unknown command: {}", other)),
    }
}

// ── Command handlers ─────────────────────────────────────────────────────────

/// Extract a JPEG frame from the latest HLS segment and save it to the DB.
async fn cmd_take_snapshot(
    camera_id: &str,
    hls_base_dir: &Path,
    db: &NodeDatabase,
) -> std::result::Result<serde_json::Value, String> {
    let camera_hls_dir = hls_base_dir.join(camera_id);
    let latest_segment = find_latest_segment(&camera_hls_dir)
        .ok_or_else(|| format!("No segments found for camera {}", camera_id))?;

    // Use FFmpeg to extract a single frame as JPEG
    let temp_path = std::env::temp_dir()
        .join(format!("opensentry_snap_{}.jpg", camera_id));

    let ffmpeg = crate::streaming::find_ffmpeg();

    let output = tokio::process::Command::new(&ffmpeg)
        .args([
            "-y",
            "-i", &latest_segment.to_string_lossy(),
            "-frames:v", "1",
            "-q:v", "2",
        ])
        .arg(&temp_path)
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("FFmpeg failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last_line = stderr.lines().last().unwrap_or("unknown error");
        return Err(format!("FFmpeg error: {}", last_line));
    }

    // Read the JPEG data and save to database
    let data = tokio::fs::read(&temp_path).await
        .map_err(|e| format!("Failed to read snapshot: {}", e))?;
    let _ = tokio::fs::remove_file(&temp_path).await;

    let now = chrono::Utc::now();
    let filename = format!(
        "{}_{}.jpg",
        camera_id.replace(['/', '\\'], "_"),
        now.format("%Y%m%d_%H%M%S"),
    );
    let timestamp = now.timestamp_millis();
    let size = data.len() as u64;

    // Base64-encode the JPEG for transfer over WebSocket
    let image_b64 = BASE64_STANDARD.encode(&data);

    db.save_snapshot(camera_id, &filename, timestamp, &data)
        .map_err(|e| format!("DB save error: {}", e))?;

    tracing::info!("Snapshot captured: {} ({} bytes)", filename, size);

    Ok(serde_json::json!({
        "filename": filename,
        "size_bytes": size,
        "timestamp": timestamp,
        "image_b64": image_b64,
    }))
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Find the newest .ts segment file in a directory by sequence number.
fn find_latest_segment(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".ts") { return None; }
            // Format: segment_00001.ts
            let parts: Vec<&str> = name.split('_').collect();
            if parts.len() != 2 { return None; }
            let seq = parts[1].trim_end_matches(".ts").parse::<u64>().ok()?;
            Some((seq, e.path()))
        })
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, path)| path)
}

fn get_local_ip() -> Option<String> {
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}
