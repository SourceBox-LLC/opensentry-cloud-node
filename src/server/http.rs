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
//! HTTP Server Implementation
//!
//! Serves recordings, snapshots, and HLS streams.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use warp::Filter;

use crate::error::Result;
use crate::config::ServerConfig;

pub struct HttpServer {
    config: ServerConfig,
    storage_path: PathBuf,
    hls_cameras: HashMap<String, PathBuf>,
}

impl HttpServer {
    pub fn new(config: ServerConfig, storage_path: PathBuf) -> Self {
        Self {
            config,
            storage_path,
            hls_cameras: HashMap::new(),
        }
    }

    /// Create HTTP server with HLS camera map
    pub fn new_with_hls(
        config: ServerConfig,
        storage_path: PathBuf,
        hls_cameras: HashMap<String, PathBuf>,
    ) -> Self {
        Self {
            config,
            storage_path,
            hls_cameras,
        }
    }

    /// Start the HTTP server
    pub async fn run(self) -> Result<()> {
        let bind_addr = format!("{}:{}", self.config.bind, self.config.port);
        tracing::info!("Starting HTTP server on {}", bind_addr);

        // Health check endpoint
        let health = warp::path("health")
            .and(warp::get())
            .map(|| "OK\n");

        // Recordings directory
        let recordings_path = self.storage_path.clone();
        let recordings_filter = warp::path("recordings");
        let recordings_fs = warp::fs::dir(recordings_path.join("recordings"));

        // Snapshots directory
        let snapshots_path = self.storage_path.clone();
        let snapshots_filter = warp::path("snapshots");
        let snapshots_fs = warp::fs::dir(snapshots_path.join("snapshots"));

        // List recordings
        let path_recordings = self.storage_path.clone();
        let list_recordings = warp::path("recordings")
            .and(warp::path("list"))
            .and(warp::get())
            .map(move || {
                let recordings = list_files_in_dir(&path_recordings.join("recordings"), &["mp4", "mkv"]);
                warp::reply::json(&recordings)
            });

        // List snapshots
        let path_snapshots = self.storage_path.clone();
        let list_snapshots = warp::path("snapshots")
            .and(warp::path("list"))
            .and(warp::get())
            .map(move || {
                let snapshots = list_files_in_dir(&path_snapshots.join("snapshots"), &["jpg", "jpeg"]);
                warp::reply::json(&snapshots)
            });

        // HLS stream endpoints
        let hls_cameras = Arc::new(self.hls_cameras.clone());
        
        // GET /hls/{camera_id}/stream.m3u8
        let hls_cameras_clone = hls_cameras.clone();
        let hls_playlist = warp::path!("hls" / String / "stream.m3u8")
            .and(warp::get())
            .map(move |camera_id: String| {
                let cameras = hls_cameras_clone.clone();
                match cameras.get(&camera_id) {
                    Some(hls_dir) => {
                        let playlist_path = hls_dir.join("stream.m3u8");
                        match std::fs::read(&playlist_path) {
                            Ok(content) => {
                                build_response(200, Some(("Content-Type", "application/vnd.apple.mpegurl")), Some(("Cache-Control", "no-cache")), content)
                            }
                            Err(e) => {
                                tracing::error!("Failed to read playlist for {}: {}", camera_id, e);
                                build_response(404, None, None, Vec::new())
                            }
                        }
                    }
                    None => {
                        build_response(404, None, None, Vec::new())
                    }
                }
            });

        // GET /hls/{camera_id}/segment_{n}.ts
        let hls_cameras_clone2 = hls_cameras;
        let hls_segment = warp::path!("hls" / String / String)
            .and(warp::get())
            .map(move |camera_id: String, filename: String| {
                let cameras = hls_cameras_clone2.clone();
                // Only serve .ts files
                if !filename.ends_with(".ts") || !filename.starts_with("segment_") {
                    return build_response(400, None, None, Vec::new());
                }
                
                match cameras.get(&camera_id) {
                    Some(hls_dir) => {
                        let segment_path = hls_dir.join(&filename);
                        match std::fs::read(&segment_path) {
                            Ok(content) => {
                                build_response(200, Some(("Content-Type", "video/mp2t")), Some(("Cache-Control", "public, max-age=3600")), content)
                            }
                            Err(e) => {
                                tracing::debug!("Segment not found {}: {}", filename, e);
                                build_response(404, None, None, Vec::new())
                            }
                        }
                    }
                    None => {
                        build_response(404, None, None, Vec::new())
                    }
                }
            });

        // Combine routes
        let routes = health
            .or(recordings_filter.and(recordings_fs))
            .or(snapshots_filter.and(snapshots_fs))
            .or(list_recordings)
            .or(list_snapshots)
            .or(hls_playlist)
            .or(hls_segment);

        // Start server
        warp::serve(routes)
            .run(([0, 0, 0, 0], self.config.port))
            .await;

        Ok(())
    }
}

/// List files in directory with given extensions
fn list_files_in_dir(dir: &std::path::Path, extensions: &[&str]) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let path = e.path();
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    
                    if extensions.contains(&ext) {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect()
        }
        Err(_) => Vec::new()
    }
}

/// Build HTTP response without panicking
/// 
/// This helper function constructs an HTTP response safely.
/// Since we control all inputs (status codes and headers), this should never fail.
/// In the unlikely event of a failure, it returns a simple 500 error.
fn build_response(
    status: u16,
    header1: Option<(&str, &str)>,
    header2: Option<(&str, &str)>,
    body: Vec<u8>,
) -> warp::http::Response<Vec<u8>> {
    let mut builder = warp::http::Response::builder()
        .status(status);
    
    if let Some((name, value)) = header1 {
        builder = builder.header(name, value);
    }
    
    if let Some((name, value)) = header2 {
        builder = builder.header(name, value);
    }
    
    builder
        .body(body)
        .unwrap_or_else(|e| {
            tracing::error!("Failed to build HTTP response: {}", e);
            warp::http::Response::builder()
                .status(500)
                .body(Vec::new())
                .expect("Fallback response builder should never fail")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_files() {
        let files = list_files_in_dir(std::path::Path::new("."), &["rs"]);
        // Should list rust files in current directory
        println!("Files: {:?}", files);
    }
}