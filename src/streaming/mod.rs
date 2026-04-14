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
//! HLS Streaming Module
//!
//! Generates HLS (HTTP Live Streaming) segments from camera frames
//! and uploads them to cloud storage.

pub mod codec_detector;
pub mod hls_generator;
pub mod hls_uploader;
pub mod motion_detector;
pub mod segment_uploader;
pub mod supervisor;

pub use codec_detector::CodecInfo;
pub use hls_generator::HlsGenerator;
pub use hls_generator::HlsGeneratorConfig;
pub use hls_generator::HlsSegment;
pub use hls_uploader::HlsUploader;
pub use hls_uploader::HlsUploaderConfig;
pub use segment_uploader::SegmentUploader;
pub use segment_uploader::UploaderConfig;

/// Find FFmpeg executable — local bundled copy first, then system PATH.
///
/// Shared by hls_generator, motion_detector, and websocket snapshot handler.
pub fn find_ffmpeg() -> String {
    #[cfg(target_os = "windows")]
    {
        if let Ok(cwd) = std::env::current_dir() {
            let local = cwd.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if local.exists() {
                return local.to_string_lossy().to_string();
            }
        }
    }
    "ffmpeg".to_string()
}