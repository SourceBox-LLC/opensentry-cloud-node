# AGENTS.md

OpenSentry CloudNode — turns USB webcams into cloud-connected security cameras. Rust binary that transcodes camera video into HLS and pushes each segment directly into the Command Center's in-memory cache. **No Tigris, no S3, no presigned URLs.**

**Companion docs:**
- [`README.md`](README.md) — user-facing install and operation guide.
- [`docs/runbooks/`](docs/runbooks/) — runbooks for common failure modes (start with `video-not-showing.md`).
- [`docs/adr/`](docs/adr/) — architecture decision records (e.g. `0001-pi-software-encoding.md` for why Pi is libx264-only).

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

Config is stored in a **SQLite database** (`data/node.db`). The API key is encrypted at rest using AES-256-GCM with a machine-derived key — SHA-256 of the OS machine identifier (`/etc/machine-id` on Linux, `MachineGuid` registry key on Windows, `IOPlatformUUID` on macOS) + an application salt. The DB is not portable between machines. DBs written by older CloudNode versions (hostname-derived key) are transparently migrated to the machine-ID-derived key on first decrypt.

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
│   ├── http.rs         # Endpoints: /health, /hls/* — binds to 127.0.0.1 by default
│   └── mod.rs
├── setup/              # Interactive TUI setup wizard (crossterm + inquire)
│   ├── mod.rs          # Setup flow
│   ├── platform.rs     # Platform detection (Linux / Windows / macOS / Pi / WSL)
│   ├── tui.rs          # Terminal UI
│   ├── ui.rs           # Rendering helpers
│   ├── animations.rs   # Progress animations
│   ├── validator.rs    # Credential validation via POST /api/nodes/validate
│   ├── recovery.rs     # Error recovery and user guidance
│   └── wsl_preflight.rs # WSL2 Scope A preflight: detect WSL, pick a usable distro,
│                        #   install FFmpeg in-distro, print usbipd bind/attach
│                        #   commands for each detected USB camera. Actions that
│                        #   need admin elevation are printed, not executed.
├── streaming/          # HLS pipeline
│   ├── hls_generator.rs    # FFmpeg subprocess per camera (HLS muxer)
│   ├── supervisor.rs       # Per-camera FFmpeg supervisor: exponential-backoff restart,
│   │                       #   stall-flag watchdog, propagates CameraStatus to the dashboard
│   ├── hls_uploader.rs     # Watches HLS dir, drives playlist updates + motion event channel
│   ├── segment_uploader.rs # Posts each .ts to POST /push-segment with retry/backoff
│   ├── motion_detector.rs  # Parallel FFmpeg scene-change scorer
│   ├── codec_detector.rs   # FFprobe-based codec detection
│   └── mod.rs              # Re-exports + shared find_ffmpeg() helper
└── storage/            # SQLite-backed local storage
    ├── database.rs     # NodeDatabase: snapshots, recordings, config (all BLOB/KV)
    └── mod.rs

examples/
└── wsl_preflight_probe.rs  # Manual probe that runs the WSL2 preflight against
                            #   the real host and prints distros / ffmpeg / usbipd state.
                            #   Run with: cargo run --example wsl_preflight_probe
```

## Architecture

### Lifecycle

`main.rs` → `Node::new()` → `Node::run()`

**Node::run()** workflow:
1. Create live TUI dashboard (raw mode, crossterm events)
2. Detect cameras (`camera::detect_cameras()`)
3. Register with Command Center (`api_client.register()`)
4. Detect hardware encoder once (NVENC/QSV/AMF on x86; **libx264 forced on Raspberry Pi**), persist to DB
5. Coerce any retired encoders stored in DB (`RETIRED_ENCODERS` — see "Encoder coercion" below) back to auto-detect
6. Spawn one `FFmpegSupervisor` per camera (wraps `HlsGenerator` — see "Supervisor" below)
7. Spawn HLS uploader tasks (segment push + playlist update + codec detection)
8. Spawn motion detector per camera (second FFmpeg probe for scene-change scoring)
9. Launch local HTTP server (port 8080) + WebSocket client
10. Start retention task (enforces `max_size_gb` via DB)
11. Run dashboard render loop (blocks until `/quit` or Ctrl+C)

### FFmpeg supervisor (`streaming/supervisor.rs`)

Each camera's HLS pipeline runs under an `FFmpegSupervisor` rather than being spawned once and forgotten. The supervisor:

- Polls the FFmpeg child every 2s (`POLL_INTERVAL`).
- On exit, restarts FFmpeg with exponential backoff (1s → 2s → 4s → … capped at 30s, matching the WebSocket reconnect ceiling).
- Gives up and marks the camera `Failed` if it restarts more than 5 times inside a 60s window.
- Resets backoff after 60s of healthy streaming (`HEALTHY_RESET_THRESHOLD`).
- Watches a shared `stall_flag: Arc<AtomicBool>` that the uploader raises after ~20s of no new segments — a wedged-but-alive FFmpeg (V4L2 deadlock, thermal throttle below real-time, USB bandwidth starvation) gets killed and routed through the normal restart path.
- Pushes `CameraStatus::Streaming / Restarting / Failed` into the dashboard so WebSocket and HTTP heartbeats report real pipeline state instead of the old hardcoded `"streaming"`.
- Supports a `PipelineSource::TestPattern(w, h, fps)` fallback used in dev / CI when a real webcam isn't available.

Before this supervisor existed, an FFmpeg crash (disk-full, closed V4L2 fd, segment-writer failure) silently left the camera offline from the browser's point of view while the node still reported `streaming` in every heartbeat — backend MCP tools ended up telling users to "update CloudNode" when the real failure was upstream.

**Disk-exhausted annotation.** On Linux the supervisor calls `libc::statvfs` on the HLS output dir before every start and after every crash. If the filesystem is under 256 MiB free, the error string surfaced to `CameraStatus::Restarting` / `CameraStatus::Failed` is prefixed with `(disk exhausted: N MiB free)`. That string flows into heartbeats and the `get_node` MCP tool, so an operator never has to SSH in to diagnose ENOSPC — they see it directly in the dashboard. Only implemented on Linux because the Pi is where the failure mode lives; on other platforms the helper returns `None` and the error string passes through untouched.

**Orphan segment sweeper** (`streaming/hls_uploader.rs` → `sweep_orphan_segments`). Belt-and-suspenders for the `-hls_flags delete_segments` guarantee. Every ~60s the uploader lists `data/hls/{cam}/segment_*.ts`, sorts by embedded sequence number, keeps the newest `local_buffer_size + 60` (~30+ MB upper bound per camera), and removes the rest. Runs on `tokio::task::spawn_blocking` so large directories don't stall the poll loop. Unit tests in `hls_uploader.rs::tests` (`sweep_keeps_newest_segments_by_sequence`, `sweep_noop_when_below_keep_count`, `sweep_ignores_non_segment_files`, `sweep_handles_nonexistent_dir`) lock the behaviour in.

### Encoder coercion (`src/node/runner.rs:174-188`)

The Pi's `h264_v4l2m2m` hardware encoder writes a non-conforming SPS on every Pi hardware revision we've tested, so it's been retired across the codebase (see `HlsGenerator::detect_hw_encoder` for the full reasoning). Because the runner normally only re-detects the encoder when the DB value is empty, a Pi that completed setup on v0.1.12 would otherwise keep using `h264_v4l2m2m` forever.

The coercion works by walking `RETIRED_ENCODERS: &[&str] = &["h264_v4l2m2m"]` against the stored value; if any match, the DB value is cleared to force re-detection on the next start. New retirements only need a one-liner added to that slice.

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

**Encoder-specific args** (`HlsGenerator::build_encoding_args`):

| Encoder | Accepts `-level auto`? | Preset | Notes |
|---------|------------------------|--------|-------|
| `h264_nvenc` (NVIDIA) | Yes | `p5` | CBR + zerolatency, `-level auto` lets NVENC pick |
| `h264_qsv` (Intel) | Yes | `veryfast` | CBR |
| `h264_amf` (AMD) | Yes | `speed` | CBR |
| `libx264` (CPU fallback) | **No** — level omitted | `ultrafast` | libx264 auto-computes level and embeds it in the SPS |

**HLS muxer flags** (`HLS_FLAGS_VALUE` in `hls_generator.rs`): passed to every FFmpeg invocation as `-hls_flags append_list+delete_segments`. `append_list` is required for the uploader's playlist-polling loop (it would otherwise see an empty playlist every time FFmpeg truncates on write); `delete_segments` tells FFmpeg to reap `.ts` files when they fall off the end of the sliding `hls_list_size` window. Before v0.1.16 only `append_list` was set, which meant the HLS output dir grew without bound whenever the uploader's inline cleanup missed a file (upload failure, stall, auth error) — on a Pi 4 with a flaky uplink that filled a 32 GB SD card in ~2 days. The regression test `hls_flags_include_delete_segments_and_append_list` locks both flags in.

`-level auto` is a driver-specific string accepted only by the hardware encoders; passing it to libx264 errors with `Error parsing option 'level' with value 'auto'` and the encoder refuses to open. Omitting `-level` entirely lets libx264 compute the right level from resolution / framerate / bitrate and write it into the SPS — which is what hls.js / MSE needs to decode.

`-preset ultrafast` (not `veryfast`) is deliberate for the Pi 4 case: at 1080p30 ultrafast runs ~1.5 cores per stream, so two simultaneous cameras fit in the Pi 4's 4-core budget with headroom for the upload / dashboard / WebSocket tasks. `veryfast` is ~2-3 cores per stream and would starve the second camera on a Pi. The regression tests `libx264_args_omit_level_flag` and `libx264_args_use_ultrafast_preset` in `hls_generator.rs` lock both decisions in.

### Local storage

SQLite database (`data/node.db`):
- Snapshots and recordings stored as BLOBs (not exposed in open folders)
- Config stored as key-value pairs in a `config` table
- API key encrypted with AES-256-GCM using a key derived from the OS machine ID (`/etc/machine-id` / `MachineGuid` / `IOPlatformUUID`)
- Retention enforced by `enforce_retention()` — oldest data deleted first when `storage.max_size_gb` is exceeded

### Local HTTP server (warp, port 8080)

Exposes the same `.ts` and `.m3u8` files the uploader pushes to the cloud, so you can stream locally without going through the backend (`VITE_LOCAL_HLS=true` on the frontend):

| Method | Path | Notes |
|--------|------|-------|
| GET | `/health` | Returns `OK` (also consumed by the Docker HEALTHCHECK) |
| GET | `/hls/{camera_id}/stream.m3u8` | Local HLS playlist |
| GET | `/hls/{camera_id}/segment_{n}.ts` | Local segment — filename must match `segment_<digits>.ts` exactly |

**Security:** the server has no authentication and binds to `127.0.0.1` by default. Only set `server.bind = "0.0.0.0"` if you explicitly want LAN-local HLS playback; changing it exposes live video to anyone on the network.

Recordings and snapshots used to be served here from the filesystem. They now live inside the encrypted SQLite DB and are fetched over the cloud API — the old `/recordings/*` and `/snapshots/*` routes were removed.

### Outbound API surface

All outbound calls use `ApiClient` in `src/api/client.rs`:

| Method | Path | Header | Body | When |
|--------|------|--------|------|------|
| POST | `/api/nodes/register` | `X-Node-API-Key` | `RegisterRequest` JSON | Startup |
| POST | `/api/nodes/heartbeat` | `X-Node-API-Key` | `HeartbeatRequest` JSON | Every `heartbeat_interval` s (fallback path; WS heartbeat is primary) |
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
- Slash command bar (`/help`, `/settings`, `/wipe`, `/export-logs`, `/reauth`, `/clear`, `/status`, `/quit`)
- Settings page with config display and action commands
- Raw mode input via crossterm events; `\x1B[nG` cursor positioning for right border alignment

### Destructive-command confirm-on-repeat

`/wipe` and `/reauth` don't execute the first time they're entered. They arm a `pending_confirm: Option<(command, Instant)>` on the dashboard; the **same command typed again within 30 seconds** (or the explicit `confirm` argument, e.g. `/wipe confirm`) actually runs it. Any unrelated command entered in between (including `/clear` or `/status`) drops the pending confirmation so an operator can't accidentally confirm a stale `/wipe` hours later.

The logic lives in `Dashboard::check_or_arm_confirm(cmd, explicit_arg, bare)` and is exercised by the test block near the bottom of `dashboard.rs` (look for `check_or_arm_confirm` assertions). Both commands also call `ApiClient::notify_wipe_started` / `notify_reauth` before the local action runs, so the backend logs the intent before the node potentially takes itself offline.

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
- 121+ unit tests across streaming / setup / node modules
- Integration tests in `tests/integration.rs`
- Uses `tokio-test` for async testing
- Key regression tests in `streaming/hls_generator.rs`:
  - `libx264_args_omit_level_flag` — guards against re-introducing `-level auto` on libx264
  - `libx264_args_use_ultrafast_preset` — locks the Pi 4 CPU budget
  - `hw_encoder_branches_still_use_level_auto` — makes sure the libx264 fix didn't break NVENC/QSV/AMF
  - `libx264_args_contain_required_pieces` — pix_fmt, codec, profile, audio
  - `hls_flags_include_delete_segments_and_append_list` — guards the v0.1.16 disk-fill fix (both flags must stay)
- Orphan-sweeper tests in `streaming/hls_uploader.rs`:
  - `sweep_keeps_newest_segments_by_sequence` — retention correctness
  - `sweep_noop_when_below_keep_count` — below-threshold no-op
  - `sweep_ignores_non_segment_files` — `stream.m3u8` / other files untouched
  - `sweep_handles_nonexistent_dir` — surfaces `io::Error` instead of panicking

**Manual probes (examples/):**

```bash
cargo run --example wsl_preflight_probe    # Print WSL + usbipd state without running setup
```

**Manual check:**
```bash
cargo run -- --once     # Run one detection cycle and exit (if supported by current main.rs)
```

## Docker

**Build:** `docker build -t opensentry-cloudnode:latest .`

Published image: `ghcr.io/sourcebox-llc/opensentry-cloudnode`. Tags track the Cargo version (`:0.1.16`), plus floating `:latest` and `:0.1`. The image is built + pushed by `.github/workflows/release.yml` on tag push. Multi-arch images (`linux/amd64`, `linux/arm64`) are built via Docker Buildx with QEMU emulation; prebuilt ARM64 binaries are also published to GitHub Releases (built on native `ubuntu-24.04-arm` runners).

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

**Raspberry Pi (Linux ARM64):** production-ready
- Prebuilt binaries and Docker images (`linux/arm64`) are published on every release
- Encoder: **libx264 CPU** only. `h264_v4l2m2m` is in `RETIRED_ENCODERS` because every Pi hardware revision we tested writes a non-conforming SPS that browsers reject.
- Preset: libx264 `ultrafast` keeps a 1080p30 stream under ~1.5 cores on a Pi 4, leaving room for a second camera. See "HLS generation" for the full rationale.
- USB: plug cameras into the Pi directly, not through an unpowered hub. Hub EMI faults show up as `usb-port: disabled by hub (EMI?)` + `xhci_hcd: Setup ERROR` in `dmesg` and wedge the whole USB controller until reboot.
- Under-voltage: `vcgencmd get_throttled` — anything non-zero means the PSU is sagging and FFmpeg restarts will follow.

**Windows:** production-ready (DirectShow)
- FFmpeg auto-downloaded during setup to `./ffmpeg/bin/`
- Camera names: `MEE USB Camera`, `Integrated Webcam`, etc.

**Windows + WSL2:** alternative deployment path
- Setup wizard detects the WSL2 option, runs `wsl_preflight.rs`, installs FFmpeg inside the chosen distro, and prints the `usbipd bind` / `usbipd attach --wsl` commands for each detected USB camera.
- Steps that need admin elevation (installing WSL itself, installing `usbipd-win` via `winget`, running `usbipd bind`) are printed for the operator to run in an elevated PowerShell — we don't execute them (Scope A). Scope B would handle elevation programmatically.
- `docker-desktop` distros are filtered out because they have no package manager and no v4l2 support.

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
| `tracing` / `tracing-subscriber` | Logging |
| `crossterm` | Terminal raw mode + input events (dashboard TUI) |
| `inquire` / `indicatif` | Interactive prompts + progress bars (setup wizard) |
| `colored` | ANSI color formatting |
| `rusqlite` (bundled) | SQLite database |
| `aes-gcm` / `sha2` / `rand` | AES-256-GCM encryption for API key at rest (key derived from OS machine ID) |
| `bytes` | Zero-copy buffers for segment upload |
| `base64` | Snapshot image transfer over WebSocket |
| `percent-encoding` | URL-safe encoding for WebSocket query params with arbitrary key bytes |
| `once_cell` | Lazy one-shot static initialisation (logging registry, encoder cache) |
| `chrono` | Timestamps |
| `uuid` | Unique identifiers |
| `sysinfo` | System information (hostname, platform detection) |
| `anyhow` / `thiserror` | Error handling |
| `dotenvy` | Legacy `.env` loading |
| `zip` | Installer archive extraction (Windows FFmpeg download) |
| `libc` (Linux only) | Raw V4L2 `ioctl` for camera capability probing — see `src/camera/platform/linux.rs` |

## Code Conventions

- No `unwrap()` outside of tests — use `?` or an `Error` variant
- All errors use the custom `Error` enum (`src/error.rs`)
- Async functions return `Result<T>`
- Platform-specific code lives in `camera/platform/`
- Re-exports in `lib.rs` for convenience
- CLI subcommands handled in `main.rs`
