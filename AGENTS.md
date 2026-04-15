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
4. **CLI flags** — highest priority: `--node-id`, `--api-key`, `--api-url`, `--config`, `--log-level`, `--once`

**Subcommands** (`src/main.rs`):
- *(none / default)* — starts the node, or launches setup if no credentials are stored
- `run` — explicit start command; accepts the same `--node-id` / `--api-key` / `--api-url` / `--once` flags
- `setup` — interactive TUI setup wizard (`--non-interactive` flag is declared but not yet implemented)
- `uninstall` — removes `.env`, `data/`, and `ffmpeg/` (use `--force` to skip confirmation)

## Project Structure

```
src/
├── main.rs           # CLI entry point (clap)
├── lib.rs            # Re-exports
├── dashboard.rs      # Live TUI dashboard with slash commands
├── api/              # Cloud API client (reqwest) + WebSocket
│   ├── client.rs     # API communication
│   ├── websocket.rs  # WebSocket client (commands from cloud)
│   └── types.rs      # Request/response types
├── camera/           # Camera detection & capture
│   ├── detector.rs   # Auto-detect USB cameras
│   ├── capture.rs    # Frame capture
│   ├── platform/     # Platform-specific implementations
│   │   ├── linux.rs  # Video4Linux2
│   │   ├── windows.rs # DirectShow
│   │   └── macos.rs  # AVFoundation
│   └── types.rs      # Camera types
├── config/           # Configuration
│   ├── mod.rs        # Config loader (DB → YAML → env → CLI)
│   └── settings.rs   # Settings structs (incl. HlsConfig)
├── error.rs          # Custom Error enum (thiserror)
├── node/             # Main orchestrator
│   └── runner.rs     # Node lifecycle
├── server/           # HTTP server (warp)
│   └── http.rs       # Endpoints: /health, /hls/*, /recordings, /snapshots
├── setup/            # Interactive TUI setup wizard
│   ├── mod.rs        # Setup flow
│   ├── platform.rs   # Platform detection
│   ├── recovery.rs   # Error recovery and user guidance
│   ├── tui.rs        # Terminal UI (crossterm + inquire)
│   ├── animations.rs # Terminal animations / progress effects
│   ├── ui.rs         # Shared UI helpers and widgets
│   └── validator.rs  # Input validation for setup prompts
├── streaming/        # HLS generation
│   ├── hls_generator.rs    # FFmpeg orchestration
│   ├── hls_uploader.rs     # Upload segments to cloud
│   ├── segment_uploader.rs # Individual segment upload
│   └── codec_detector.rs   # Video codec detection
└── storage/          # SQLite-backed local storage
    └── database.rs   # NodeDatabase: snapshots, recordings, config (all BLOB/KV)
```

## Architecture

**Lifecycle**: `main.rs` → `Node::new()` → `Node::run()`

**Node::run()** workflow:
1. Create live TUI dashboard (raw mode, crossterm events)
2. Detect cameras (`camera::detect_cameras()`)
3. Register with cloud API (`api_client.register()`)
4. Detect hardware encoder once (NVENC/QSV/AMF/V4L2M2M → libx264 fallback), persist to DB
5. Create HLS generator per camera (FFmpeg subprocess)
6. Start HLS uploader tasks (segment upload + codec detection)
7. Launch HTTP server (port 8080) + WebSocket client
8. Start retention task (enforces `max_size_gb` via DB)
9. Run dashboard render loop (blocks until `/quit` or Ctrl+C)

**Camera Capture**: Platform-specific
- Linux: `/dev/video*` devices via v4l2
- Windows: DirectShow via FFmpeg `-f dshow`
- macOS: AVFoundation via FFmpeg `-f avfoundation`

**HLS Generation**: FFmpeg subprocess transcoding camera → HLS segments
- Output: `./data/hls/{camera_id}/stream.m3u8`
- Segments: `segment_0.ts`, `segment_1.ts`, etc.
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
- `GET /recordings/...` - Serves files from `<storage>/recordings/`
- `GET /recordings/list` - JSON list of recording filenames (`.mp4`, `.mkv`)
- `GET /snapshots/...` - Serves files from `<storage>/snapshots/`
- `GET /snapshots/list` - JSON list of snapshot filenames (`.jpg`, `.jpeg`)

**Dashboard TUI** (`dashboard.rs`):
- Full-screen live dashboard with camera status, upload stats, log viewer
- Slash command bar (`/help`, `/settings`, `/wipe`, `/export-logs`, `/reauth`, `/quit`)
- Settings page with config display and action commands
- Raw mode input via crossterm events, `\x1B[nG` cursor positioning for right border

## Key Patterns

**Error Handling**: `thiserror` with custom `Error` enum in `src/error.rs`
```rust
pub type Result<T> = std::result::Result<T, Error>;
```

**Async Runtime**: `tokio` throughout
- All I/O operations are async
- FFmpeg managed via `tokio::process::Command`

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
- Integration tests in `tests/integration.rs`
- Uses `tokio-test` for async testing

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

**Docker Compose**: `docker-compose up -d`
- Requires `.env` file with credentials
- Mounts `./data` for persistence

## Platform Notes

**Linux**: Production-ready (v4l2)
- Add user to video group: `sudo usermod -a -G video $USER`
- Camera devices: `/dev/video0`, `/dev/video1`, etc.

**Windows**: Production-ready (DirectShow)
- FFmpeg auto-downloaded during setup
- Camera names: `MEE USB Camera`, `Integrated Webcam`, etc.

**macOS**: Experimental (AVFoundation)
- Requires FFmpeg: `brew install ffmpeg`
- May need camera permission in System Preferences

## Key Dependencies

- `tokio` - Async runtime
- `warp` - HTTP server
- `reqwest` - HTTP client
- `serde`/`serde_json` - JSON serialization
- `clap` - CLI parser
- `tracing`/`tracing-subscriber`/`tracing-appender` - Logging
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

## Code Conventions

- No `unwrap()` outside of tests (use `?` or `Context`)
- All errors use custom `Error` enum (`src/error.rs`)
- Async functions return `Result<T>`
- Platform-specific code in `camera/platform/`
- Re-exports in `lib.rs` for convenience
- CLI commands handled in `main.rs` with subcommands