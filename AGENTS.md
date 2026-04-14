# AGENTS.md

OpenSentry CloudNode — turns USB webcams into cloud-connected security cameras. Rust binary that transcodes camera video into HLS and pushes each segment directly into the Command Center's in-memory cache. **No Tigris, no S3, no presigned URLs.**

## Build & Test Commands

```bash
cargo build              # Debug build
cargo build --release    # Optimized (production)
cargo test               # Unit + integration tests
cargo clippy             # Lint
cargo fmt -- --check     # Format check
cargo run                # Development mode (falls through to setup wizard if unconfigured)
cargo run -- setup       # Force-run the setup wizard
```

## Configuration

Config is stored in a **SQLite database** (`data/node.db`). The API key is encrypted at rest using AES-256-GCM with a machine-derived key (SHA-256 of hostname + application salt). The DB is not portable between machines.

Loading priority (in `Config::load()`):

1. **SQLite database** (`data/node.db`) — primary, created by setup wizard
2. **YAML file** (`config.yaml`) — legacy fallback, auto-migrated to DB on first load
3. **Environment variables** — override any DB/YAML values:
   - `OPENSENTRY_NODE_ID`, `OPENSENTRY_API_KEY`, `OPENSENTRY_API_URL`
   - `OPENSENTRY_ENCODER` — video encoder override (e.g. `h264_nvenc`)
   - `RUST_LOG` — log level
4. **CLI flags** — highest priority: `--node-id`, `--api-key`, `--api-url`

### Config sections (`Config` in `src/config/settings.rs`)

- `node` — friendly name
- `cloud` — `api_url`, `api_key` (never serialised), `heartbeat_interval`
- `cameras` — `auto_detect`, optional manual `devices` list
- `streaming` — `fps`, `jpeg_quality`, `encoder`, nested `hls` (`enabled`, `segment_duration`, `playlist_size`, `bitrate`)
- `recording` — `enabled`, `format` (`mp4` or `mkv`)
- `storage` — `path`, `max_size_gb`
- `server` — local HTTP `port` + `bind`
- `logging` — `level`
- `motion` — `enabled`, `threshold` (scene-change score 0.0–1.0), `cooldown_secs`

## Project Structure

```
src/
├── main.rs             # CLI entry point (clap)
├── lib.rs              # Library re-exports
├── dashboard.rs        # Live TUI dashboard + slash commands
├── error.rs            # Custom Error enum + Result alias
├── logging.rs          # tracing subscriber setup
├── api/                # Cloud API client + WebSocket
│   ├── client.rs       # ApiClient — register, heartbeat, codec, push_segment, playlist, motion
│   ├── websocket.rs    # WS loop with auto-reconnect; relays motion events + handles commands
│   ├── types.rs        # Request/response types
│   └── mod.rs
├── camera/             # Detection & capture
│   ├── detector.rs     # Auto-detect USB cameras
│   ├── capture.rs      # Frame capture helpers
│   ├── platform/       # Linux (v4l2) / Windows (DirectShow) / macOS (AVFoundation)
│   ├── types.rs
│   └── mod.rs
├── config/             # Configuration
│   ├── mod.rs          # Config loader (DB → YAML → env → CLI)
│   └── settings.rs     # Config structs (see sections above)
├── node/               # Orchestrator
│   ├── runner.rs       # Node lifecycle (register, spawn pipelines, dashboard loop)
│   └── mod.rs
├── server/             # Local HTTP server (warp)
│   ├── http.rs         # Endpoints: /health, /hls/*, /recordings/*, /snapshots/*
│   └── mod.rs
├── setup/              # Interactive TUI setup wizard (crossterm + inquire)
│   ├── mod.rs          # Setup flow
│   ├── platform.rs     # Platform detection
│   ├── tui.rs          # Terminal UI
│   ├── ui.rs           # Rendering helpers
│   ├── animations.rs   # Progress animations
│   ├── validator.rs    # Credential validation via POST /api/nodes/validate
│   └── recovery.rs     # Error recovery and user guidance
├── streaming/          # HLS pipeline
│   ├── hls_generator.rs    # FFmpeg subprocess per camera (HLS muxer)
│   ├── supervisor.rs       # Polls FFmpeg every 2s, respawns with exponential backoff
│   │                       # (1s→30s), reports Streaming/Restarting/Failed into Dashboard
│   ├── hls_uploader.rs     # Watches HLS dir, drives playlist updates + motion event channel
│   ├── segment_uploader.rs # Posts each .ts to POST /push-segment with retry/backoff
│   ├── motion_detector.rs  # Parallel FFmpeg scene-change scorer
│   ├── codec_detector.rs   # FFprobe-based codec detection
│   └── mod.rs              # Re-exports + shared find_ffmpeg() helper
└── storage/            # SQLite-backed local storage
    ├── database.rs     # NodeDatabase: snapshots, recordings, config (all BLOB/KV)
    └── mod.rs
```

## Architecture

### Lifecycle

`main.rs` → `Node::new()` → `Node::run()`

**Node::run()** workflow:
1. Create live TUI dashboard (raw mode, crossterm events)
2. Detect cameras (`camera::detect_cameras()`)
3. Register with Command Center (`api_client.register()`)
4. Detect hardware encoder once (NVENC/QSV/AMF), persist to DB
5. Spawn an FFmpeg **supervisor** per camera (`streaming/supervisor.rs`) that owns the `HlsGenerator`, polls the child every 2s, respawns it with exponential backoff (1s → 2s → 4s → … capped at 30s) when it dies, and trips the camera into `Failed` state if the backoff window sees 5+ crashes in 60s. Each transition (`Starting` / `Streaming` / `Restarting { reason }` / `Failed { reason }`) is pushed into the `Dashboard` so the heartbeat / WS messages carry the real pipeline state (with `last_error`) rather than the old hardcoded `"streaming"`.
6. Spawn HLS uploader tasks (segment push + playlist update + codec detection)
7. Spawn motion detector per camera (second FFmpeg probe for scene-change scoring)
8. Launch local HTTP server (port 8080) + WebSocket client
9. Start retention task (enforces `max_size_gb` via DB)
10. Run dashboard render loop (blocks until `/quit` or Ctrl+C)

### Video push path

```
Camera ─► FFmpeg muxer ─► data/hls/{cam}/segment_NNNNN.ts
                               │
                  hls_uploader │ detects new file
                               ▼
                      segment_uploader.push_segment()
                               │   bytes, filename
                               ▼
            POST /api/cameras/{cam}/push-segment?filename=…
            Header: X-Node-API-Key: …
            Body:   raw MPEG-TS bytes, Content-Type: video/mp2t
                               │
                               ▼
                     Command Center in-memory cache
```

On every playlist refresh (`stream.m3u8`), CloudNode also POSTs the file text to `POST /api/cameras/{id}/playlist`. The backend rewrites segment URLs to relative proxy paths and caches that rewritten version.

### Motion events

`motion_detector.rs` runs a second FFmpeg per camera with the `select='gt(scene,THRESHOLD)'` filter. Above-threshold frames emit a `MotionEvent { camera_id, score, timestamp, segment_seq }` onto an `mpsc` channel.

The WebSocket loop (`api/websocket.rs`) drains that channel inside its `tokio::select!`. When an event arrives it sends:

```json
{"type": "event", "command": "motion_detected", "payload": {...}}
```

If the WebSocket is disconnected, the event falls back to `POST /api/cameras/{id}/motion` via `ApiClient::report_motion()`. Cooldown is applied by `hls_uploader` before the event ever reaches the channel, so flapping is not a concern at the backend.

### Camera capture (platform-specific)

- **Linux:** `/dev/video*` devices via v4l2
- **Windows:** DirectShow via FFmpeg `-f dshow`
- **macOS:** AVFoundation via FFmpeg `-f avfoundation`

### HLS generation

FFmpeg subprocess transcoding camera → HLS segments:
- Output: `./data/hls/{camera_id}/stream.m3u8`
- Segment duration: `streaming.hls.segment_duration` (default 1s)
- Playlist window: `streaming.hls.playlist_size` (default 15)
- HLS directories wiped on startup so segment numbering resets cleanly
- Encoder detected once at startup and shared across all cameras

### Local storage

SQLite database (`data/node.db`):
- Snapshots and recordings stored as BLOBs (not exposed in open folders)
- Config stored as key-value pairs in a `config` table
- API key encrypted with AES-256-GCM (machine-derived key from hostname)
- Retention enforced by `enforce_retention()` — oldest data deleted first when `storage.max_size_gb` is exceeded

### Local HTTP server (warp, port 8080)

Exposes the same `.ts` and `.m3u8` files the uploader pushes to the cloud, so you can stream locally without going through the backend (`VITE_LOCAL_HLS=true` on the frontend):

| Method | Path | Notes |
|--------|------|-------|
| GET | `/health` | Returns `OK` |
| GET | `/hls/{camera_id}/stream.m3u8` | Local HLS playlist |
| GET | `/hls/{camera_id}/segment_{n}.ts` | Local segment (validates prefix + extension) |
| GET | `/recordings/list` | JSON list of stored recording filenames |
| GET | `/recordings/{file}` | Served from `data/recordings/` |
| GET | `/snapshots/list` | JSON list of stored snapshot filenames |
| GET | `/snapshots/{file}` | Served from `data/snapshots/` |

### Outbound API surface

All outbound calls use `ApiClient` in `src/api/client.rs`:

| Method | Path | Header | Body | When |
|--------|------|--------|------|------|
| POST | `/api/nodes/register` | `X-Node-API-Key` | `RegisterRequest` JSON | Startup |
| POST | `/api/nodes/heartbeat` | `X-Node-API-Key` | `HeartbeatRequest` JSON (includes per-camera `CameraStatus { camera_id, status, last_error }` with real pipeline state) | Every `heartbeat_interval` s (fallback path; WS heartbeat is primary) |
| POST | `/api/cameras/{id}/codec` | `X-Node-API-Key` | `{video_codec, audio_codec}` JSON | After first segment or codec change |
| POST | `/api/cameras/{id}/push-segment?filename=…` | `X-Node-API-Key` | raw `.ts` bytes (`video/mp2t`) | Every segment |
| POST | `/api/cameras/{id}/playlist` | `X-Node-API-Key` | playlist text (`text/plain`) | Every playlist rewrite |
| POST | `/api/cameras/{id}/motion` | `X-Node-API-Key` | `{score, timestamp, segment_seq}` JSON | Motion event when WS is disconnected |
| WS | `/ws/node?api_key=…&node_id=…` | query params | JSON frames | Connected continuously; carries heartbeat, commands, motion events |

**WebSocket message types:**

- Node → Backend: `heartbeat`, `command_result`, `event` (with `command: "motion_detected"`)
- Backend → Node: `ack`, `command` (e.g. capture snapshot, start/stop recording), `error`

## Dashboard TUI (`dashboard.rs`)

- Full-screen live dashboard with camera status, upload stats, log viewer
- Slash command bar (`/help`, `/settings`, `/wipe confirm`, `/export-logs`, `/reauth confirm`, `/clear`, `/status`, `/quit`)
- Settings page with config display and action commands
- Raw mode input via crossterm events; `\x1B[nG` cursor positioning for right border alignment

## Key Patterns

**Error Handling:** `thiserror` with custom `Error` enum in `src/error.rs`
```rust
pub type Result<T> = std::result::Result<T, Error>;
```

**Async Runtime:** `tokio` throughout
- All I/O is async
- FFmpeg managed via `tokio::process::Command`
- Channels (`tokio::sync::mpsc`) for motion events and command dispatch

**Platform Abstraction:** Conditional compilation
```rust
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
```

**FFmpeg binary:** `find_ffmpeg()` in `src/streaming/mod.rs` prefers `./ffmpeg/bin/ffmpeg.exe` on Windows (setup-wizard-downloaded), falling back to `ffmpeg` on PATH.

**Retry policy:** `SegmentUploader` retries on 408/429/5xx and `reqwest` transport errors with exponential backoff (100ms, 200ms, 200ms).

## Development Workflow

1. **First Run:** `cargo run` → launches setup wizard
2. **Setup Wizard:** detects platform, cameras, downloads FFmpeg (Windows), prompts for credentials, validates against `POST /api/nodes/validate`
3. **Config stored in DB:** saves to `data/node.db` (API key encrypted with AES-256-GCM)
4. **Subsequent Runs:** `cargo run` → loads config from DB, starts dashboard TUI

## Testing

**Unit tests:** `cargo test`
- Integration tests in `tests/integration.rs`
- Uses `tokio-test` for async testing

**Manual check:**
```bash
cargo run -- --once     # Run one detection cycle and exit (if supported by current main.rs)
```

## Docker

**Build:** `docker build -t opensentry-cloudnode:latest .`

**Run:**
```bash
docker run -d \
  --device /dev/video0:/dev/video0 \
  -e OPENSENTRY_NODE_ID=xxx \
  -e OPENSENTRY_API_KEY=xxx \
  -e OPENSENTRY_API_URL=https://backend.example.com \
  -p 8080:8080 \
  -v ./data:/app/data \
  opensentry-cloudnode:latest
```

**Docker Compose:** `docker-compose up -d`
- Requires `.env` with credentials
- Mounts `./data` for persistence

## Platform Notes

**Linux:** production-ready (v4l2)
- Add user to video group: `sudo usermod -a -G video $USER`
- Camera devices: `/dev/video0`, `/dev/video1`, etc.

**Windows:** production-ready (DirectShow)
- FFmpeg auto-downloaded during setup to `./ffmpeg/bin/`
- Camera names: `MEE USB Camera`, `Integrated Webcam`, etc.

**macOS:** experimental (AVFoundation)
- Requires FFmpeg: `brew install ffmpeg`
- May need camera permission in System Settings

## Key Dependencies

| Crate | Role |
|-------|------|
| `tokio` | Async runtime |
| `reqwest` | HTTP client (push-segment, playlist, motion, heartbeat) |
| `warp` | Local HTTP server |
| `tokio-tungstenite` + `futures-util` | WebSocket client |
| `serde` / `serde_json` | JSON serialization |
| `clap` | CLI parser |
| `tracing` / `tracing-subscriber` / `tracing-appender` | Logging |
| `crossterm` | Terminal raw mode + input events (dashboard TUI) |
| `inquire` / `indicatif` | Interactive prompts + progress bars (setup wizard) |
| `colored` | ANSI color formatting |
| `rusqlite` (bundled) | SQLite database |
| `aes-gcm` / `sha2` / `rand` | AES-256-GCM encryption for API key at rest |
| `hostname` | Machine-derived encryption key |
| `bytes` | Zero-copy buffers for segment upload |
| `base64` | Snapshot image transfer over WebSocket |
| `chrono` | Timestamps |
| `uuid` | Unique identifiers |
| `sysinfo` | System information (hostname, platform detection) |
| `anyhow` / `thiserror` | Error handling |
| `dotenvy` | Legacy `.env` loading |
| `zip` | Installer archive extraction (Windows FFmpeg download) |

## Code Conventions

- No `unwrap()` outside of tests — use `?` or an `Error` variant
- All errors use the custom `Error` enum (`src/error.rs`)
- Async functions return `Result<T>`
- Platform-specific code lives in `camera/platform/`
- Re-exports in `lib.rs` for convenience
- CLI subcommands handled in `main.rs`
