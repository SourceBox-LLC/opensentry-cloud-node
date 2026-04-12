# AGENTS.md

OpenSentry CloudNode - Turns USB webcams into cloud-connected security cameras.

## Build & Test Commands

```bash
cargo build              # Debug build
cargo build --release    # Optimized build (production)
cargo test               # Run tests
cargo clippy             # Lint
cargo fmt -- --check     # Format check
cargo run                # Development mode
```

## Configuration

Config is stored in a **SQLite database** (`data/node.db`). The API key is encrypted at rest using AES-256-GCM with a machine-derived key (hostname + app salt).

Loading priority (in `Config::load()`):

1. **SQLite database** (`data/node.db`) — primary, created by setup wizard
2. **YAML file** (`config.yaml`) — legacy fallback, auto-migrated to DB on first load
3. **Environment variables** — override any DB/YAML values:
   - `OPENSENTRY_NODE_ID`, `OPENSENTRY_API_KEY`, `OPENSENTRY_API_URL`
   - `OPENSENTRY_ENCODER` — video encoder override (e.g. `h264_nvenc`)
   - `RUST_LOG` — log level
4. **CLI flags** — highest priority: `--node-id`, `--api-key`, `--api-url`

## Project Structure

```
src/
├── main.rs           # CLI entry point (clap) — subcommands: run, setup, uninstall
├── lib.rs            # Library root with re-exports
├── dashboard.rs      # Live TUI dashboard with slash commands (/help, /settings, /set, /status, /clear, /quit)
├── error.rs          # Custom error types (thiserror Error enum, Result alias)
├── logging.rs        # Tracing layer that forwards log events into the TUI dashboard
├── api/              # Cloud API client (reqwest) + WebSocket
│   ├── client.rs     # API communication (register, push-segment, heartbeat)
│   ├── websocket.rs  # WebSocket client (commands from cloud, auto-reconnect)
│   └── types.rs      # Request/response types
├── camera/           # Camera detection & capture
│   ├── detector.rs   # Auto-detect USB cameras
│   ├── capture.rs    # Frame capture (alternative capture approach)
│   ├── platform/     # Platform-specific implementations
│   │   ├── linux.rs  # Video4Linux2
│   │   ├── windows.rs # DirectShow
│   │   └── macos.rs  # AVFoundation
│   └── types.rs      # Camera types
├── config/           # Configuration
│   ├── mod.rs        # Config loader (DB → YAML → env → CLI)
│   └── settings.rs   # Settings structs (StreamingConfig, HlsConfig, MotionConfig, etc.)
├── node/             # Main orchestrator
│   └── runner.rs     # Node lifecycle (Node::new() → Node::run())
├── server/           # HTTP server (warp)
│   └── http.rs       # Endpoints: /health, /hls/*, /recordings/*, /snapshots/*
├── setup/            # Interactive TUI setup wizard
│   ├── mod.rs        # Setup flow orchestration
│   ├── tui.rs        # Terminal UI (crossterm + inquire)
│   ├── platform.rs   # Platform detection
│   ├── validator.rs  # Connection validation (API URL, Node ID, API Key)
│   ├── recovery.rs   # Error recovery and user guidance
│   ├── animations.rs # Visual effects (confetti, rainbow gradients)
│   └── ui.rs         # Terminal UI panel system (bordered panels, pill progress bars)
├── streaming/        # HLS generation
│   ├── hls_generator.rs    # FFmpeg orchestration (std::process::Command)
│   ├── hls_uploader.rs     # Upload segments to cloud
│   ├── segment_uploader.rs # Individual segment upload
│   ├── codec_detector.rs   # Video codec detection (FFprobe + camera probing)
│   └── motion_detector.rs  # Motion detection via FFmpeg scene-change analysis
└── storage/          # SQLite-backed local storage
    └── database.rs   # NodeDatabase: snapshots, recordings, config (all BLOB/KV)
```

## Architecture

**Lifecycle**: `main.rs` → `Node::new()` → `Node::run()`

**Node::run()** workflow:
1. Create live TUI dashboard, configure settings, install into tracing layer
2. Detect cameras (`camera::detect_cameras()`)
3. Register with cloud API (`api_client.register()`)
4. Clean HLS directories, detect hardware encoder once (NVENC/QSV/AMF), persist to DB; then per camera: create HLS generator + start uploader task (with motion detection channel)
5. Start HTTP server (port 8080) as a tokio task
6. Start heartbeat loop (separate tokio task) + WebSocket client (separate tokio task)
7. Start retention cleanup task (enforces `max_size_gb` via DB)
8. Start dashboard render loop in a **background std::thread**
9. Wait for shutdown signal (Ctrl+C or stop flag via `tokio::select!`) — this is the actual blocking point

**Camera Capture**: Platform-specific
- Linux: `/dev/video*` devices via v4l2
- Windows: DirectShow via FFmpeg `-f dshow`
- macOS: AVFoundation via FFmpeg `-f avfoundation`

**HLS Generation**: FFmpeg subprocess transcoding camera → HLS segments
- Output: `./data/hls/{camera_id}/stream.m3u8`
- Segments: `segment_00000.ts`, `segment_00001.ts`, etc. (5-digit zero-padded)
- HLS directories wiped on startup so segment numbering resets fresh
- Encoder detected once at startup and shared across all cameras

**Local Storage**: SQLite database (`data/node.db`)
- Snapshots and recordings stored as BLOBs (not exposed in open folders)
- Config stored as key-value pairs in `config` table
- API key encrypted with AES-256-GCM (machine-derived key from hostname)
- Retention enforced by `enforce_retention()` — oldest data deleted first

**HTTP Server** (warp, port 8080):
- `GET /health` - Health check
- `GET /hls/{camera_id}/stream.m3u8` - HLS playlist
- `GET /hls/{camera_id}/segment_{n}.ts` - Video segments
- `GET /recordings/*` - Static file serving of recordings
- `GET /recordings/list` - JSON list of recording files
- `GET /snapshots/*` - Static file serving of snapshots
- `GET /snapshots/list` - JSON list of snapshot files

**Dashboard TUI** (`dashboard.rs`):
- Full-screen live dashboard with camera status, upload stats, log viewer
- **Main view commands:** `/help` (or `/` or `/?`), `/settings`, `/status`, `/clear` (or `/cls`), `/quit` (or `/exit` or `/q`)
- **Settings view commands:** `/help`, `/set <key> <value>` (fps, encoder, segment_duration, bitrate, motion on/off, sensitivity, cooldown), `/export-logs`, `/wipe confirm`, `/reauth confirm`, `/back`, `/quit`
- Raw mode input via crossterm events, `\x1B[nG` cursor positioning for right border

**Motion Detection** (`streaming/motion_detector.rs`):
- Analyzes HLS segments for scene changes using FFmpeg
- Configured via `MotionConfig`: `enabled` (default true), `sensitivity` (default 0.3), `cooldown_secs` (default 30)
- Motion events sent via channel to WebSocket client → forwarded to Command Center
- Configurable at runtime via `/set motion on/off`, `/set sensitivity <value>`, `/set cooldown <secs>`

## Key Patterns

**Error Handling**: `thiserror` with custom `Error` enum in `src/error.rs`
```rust
pub type Result<T> = std::result::Result<T, Error>;
```

**Async Runtime**: `tokio` throughout
- All I/O operations are async
- HLS FFmpeg managed via `std::process::Command` (synchronous subprocess)
- Motion detection FFmpeg managed via `tokio::process::Command` (async)

**Platform Abstraction**: Conditional compilation
```rust
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
```

**FFmpeg**: External process
- HLS generator spawns FFmpeg subprocess
- Codec detection via FFprobe
- Windows: FFmpeg auto-downloaded to `./ffmpeg/bin/`

## Development Workflow

1. **First Run**: `cargo run` → Launches setup wizard
2. **Setup Wizard**: Detects platform, cameras, downloads FFmpeg (Windows), prompts for credentials
3. **Config stored in DB**: Saves to `data/node.db` (API key encrypted with AES-256-GCM)
4. **Subsequent Runs**: `cargo run` → Loads config from DB, starts dashboard TUI

## Testing

**Unit Tests**: `cargo test`
- Integration tests in `tests/integration.rs` (config loading, camera detection)
- Inline unit test modules in: `hls_generator.rs`, `codec_detector.rs`, `motion_detector.rs`, `http.rs`, `capture.rs`, platform modules
- `tokio-test` available as dev-dependency (used by inline async tests)

**Manual Testing**:
```bash
cargo run -- --once  # Run one detection cycle and exit
```

## Docker

**Build**: `docker build -t opensentry-cloudnode:latest .`

**Run**:
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

**Docker Compose**: `docker compose up -d` (or `docker-compose up -d`)
- Set env vars via `.env` file or shell environment
- Mounts `./data` for persistence

## Platform Notes

**Linux**: Production-ready (v4l2)
- Add user to video group: `sudo usermod -a -G video $USER`
- Camera devices: `/dev/video0`, `/dev/video1`, etc.

**Windows**: Production-ready (DirectShow)
- FFmpeg auto-downloaded during setup
- Camera names: `MEE USB Camera`, `Integrated Webcam`, etc.

**macOS**: Untested (AVFoundation)
- Requires FFmpeg: `brew install ffmpeg`
- May need camera permission in System Preferences

## Key Dependencies

- `tokio` - Async runtime
- `warp` - HTTP server
- `reqwest` - HTTP client
- `serde`/`serde_json` - JSON serialization
- `clap` - CLI parser
- `tracing`/`tracing-subscriber` - Logging (custom `DashboardLayer` in `logging.rs`, no tracing-appender)
- `crossterm` - Terminal raw mode and input events (dashboard TUI)
- `inquire` - Interactive prompts (setup wizard)
- `indicatif` - Progress bars (setup wizard)
- `colored` - ANSI color formatting
- `rusqlite` - SQLite database (config, snapshots, recordings)
- `aes-gcm`/`sha2`/`rand` - AES-256-GCM encryption for API key at rest
- `hostname` - Machine-derived encryption key
- `tokio-tungstenite`/`futures-util` - WebSocket client
- `anyhow`/`thiserror` - Error handling
- `chrono` - Timestamps
- `uuid` - Unique identifiers
- `sysinfo` - System information
- `dotenvy` - Legacy .env file loading
- `yaml-rust` - YAML config file parsing
- `once_cell` - Global dashboard slot in `logging.rs`
- `base64` - Snapshot image transfer over WebSocket
- `bytes` - Byte buffer handling
- `zip` - Archive extraction (FFmpeg auto-download on Windows)

## Code Conventions

- No `unwrap()` outside of tests (use `?` or `Context`)
- All errors use custom `Error` enum (`src/error.rs`)
- Async functions return `Result<T>`
- Platform-specific code in `camera/platform/`
- Re-exports in `lib.rs` for convenience
- CLI subcommands in `main.rs`: `run` (start node), `setup` (interactive/non-interactive setup), `uninstall` (remove stored data)