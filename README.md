<p align="center">
  <h1 align="center">OpenSentry CloudNode</h1>
  <p align="center">
    Turn any USB webcam into a cloud-connected security camera.
    <br />
    <a href="#quick-start">Quick Start</a>
    &middot;
    <a href="#configuration">Configuration</a>
    &middot;
    <a href="#docker">Docker</a>
    &middot;
    <a href="#troubleshooting">Troubleshooting</a>
  </p>
</p>

<p align="center">
  <a href="https://www.gnu.org/licenses/gpl-3.0"><img src="https://img.shields.io/badge/License-GPLv3-blue.svg" alt="License: GPL v3"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Built_with-Rust-dea584.svg" alt="Built with Rust"></a>
</p>

---

CloudNode runs on your local network, detects USB cameras, and streams live video to the [OpenSentry Command Center](https://opensentry-command.fly.dev) via HLS. All configuration is stored locally in an encrypted SQLite database — no cloud dependency for setup.

**What it does:**

- Detects USB cameras and transcodes each to HLS using FFmpeg (with hardware acceleration when available)
- Pushes 1-second `.ts` segments directly to Command Center's in-memory cache — no S3, no presigned URLs
- Detects motion from FFmpeg scene-change analysis and reports events over WebSocket (with an HTTP fallback for reliability)
- Stores recordings and snapshots locally in an encrypted SQLite database with automatic retention
- Runs a live terminal dashboard with slash commands and log viewer

**Supported platforms:**

| Platform | Status | Camera API |
|----------|--------|------------|
| Linux x86_64 / ARM64 | Production ready | Video4Linux2 |
| Windows 10 / 11 | Production ready | DirectShow |
| macOS | Experimental | AVFoundation |

---

## Quick Start

### Prerequisites

- A USB webcam
- An [OpenSentry Command Center](https://opensentry-command.fly.dev) account with a Node ID and API Key (generated from the Settings page)
- **Docker** (recommended) or **Rust 1.70+** with **FFmpeg**

### Install

The fastest way to install CloudNode:

**Linux / macOS:**
```bash
curl -fsSL https://opensentry-command.fly.dev/install.sh | bash
```

**Windows (PowerShell):**
```powershell
irm https://opensentry-command.fly.dev/install.ps1 | iex
```

The installer downloads the latest release, checks for FFmpeg, and guides you through setup.

<details>
<summary><strong>Manual install (build from source)</strong></summary>

```bash
git clone https://github.com/SourceBox-LLC/opensentry-cloud-node.git
cd opensentry-cloud-node
cargo build --release

# Run the interactive setup wizard
./target/release/opensentry-cloudnode setup
```
</details>

The setup wizard handles everything automatically:

1. Detects your platform and connected cameras
2. Downloads FFmpeg if needed (Windows)
3. Prompts for your Node ID, API Key, and Command Center URL
4. Detects the best available hardware encoder (NVENC, QSV, AMF)
5. Encrypts and stores credentials locally in `data/node.db`

After setup, start the node:

```bash
./target/release/opensentry-cloudnode
```

---

## Dashboard

CloudNode runs a full-screen terminal dashboard showing camera status, upload progress, and live logs.

Type `/` and press **Enter** to open the command menu.

**Main view:**

| Command | Description |
|---------|-------------|
| `/settings` | Open the settings page |
| `/status` | Show node status summary |
| `/clear` | Clear the log panel |
| `/quit` | Stop the node and exit |

**Settings page:**

| Command | Description |
|---------|-------------|
| `/export-logs` | Save logs to a timestamped file |
| `/wipe confirm` | Erase all stored data and reset |
| `/reauth confirm` | Clear credentials and re-run setup |
| `/back` | Return to the dashboard |

Press **Esc** to return from settings. Destructive commands require the `confirm` argument.

---

## Configuration

### How config is loaded

CloudNode resolves configuration in this order (highest priority last):

1. **SQLite database** (`data/node.db`) — created by the setup wizard, primary source of truth
2. **YAML file** (`config.yaml`) — legacy fallback, auto-migrated to the DB on first load
3. **Environment variables** — override any stored values
4. **CLI flags** — highest priority

### Environment variables

Use environment variables to override database values without modifying the DB:

| Variable | Description |
|----------|-------------|
| `OPENSENTRY_NODE_ID` | Node ID |
| `OPENSENTRY_API_KEY` | API Key |
| `OPENSENTRY_API_URL` | Command Center URL |
| `OPENSENTRY_ENCODER` | Video encoder override (e.g. `h264_nvenc`, `libx264`) |
| `RUST_LOG` | Log level: `trace`, `debug`, `info`, `warn`, `error` |

### CLI flags

```bash
opensentry-cloudnode --node-id <ID> --api-key <KEY> --api-url <URL>
```

### Motion detection

Motion detection is on by default. CloudNode pipes each camera through a second FFmpeg process with `select='gt(scene,THRESHOLD)'` to score how much the frame changed; scores above the threshold emit a `motion_detected` event (over WebSocket, or `POST /api/cameras/{id}/motion` if the socket is down). Per-camera cooldown prevents flapping.

Defaults (configurable via `config.yaml` `motion:` section):

| Setting | Default | Meaning |
|---------|---------|---------|
| `enabled` | `true` | Toggle motion detection |
| `threshold` | `0.02` | Scene-change score (0.0 = identical, 1.0 = totally different) |
| `cooldown_secs` | `30` | Minimum seconds between events per camera |

### Security

The API key is **encrypted at rest** using AES-256-GCM with a machine-derived key (SHA-256 of hostname + application salt). The database is not portable between machines — moving `node.db` to a different host will make the stored key unreadable.

---

## Docker

### Quick run

```bash
docker build -t opensentry-cloudnode .

docker run -d \
  --name opensentry-cloudnode \
  --device /dev/video0:/dev/video0 \
  -e OPENSENTRY_NODE_ID=your_node_id \
  -e OPENSENTRY_API_KEY=your_api_key \
  -e OPENSENTRY_API_URL=https://your-backend.example.com \
  -p 8080:8080 \
  -v ./data:/app/data \
  opensentry-cloudnode
```

### Docker Compose

```bash
cp .env.example .env
# Edit .env with your credentials
docker-compose up -d
```

### Multiple cameras

Pass each device to the container:

```bash
docker run -d \
  --device /dev/video0:/dev/video0 \
  --device /dev/video2:/dev/video2 \
  -e OPENSENTRY_NODE_ID=your_node_id \
  -e OPENSENTRY_API_KEY=your_api_key \
  -e OPENSENTRY_API_URL=https://your-backend.example.com \
  -p 8080:8080 \
  opensentry-cloudnode
```

---

## Architecture

```
                  USB Cameras
                      │
              ┌───────┴─────────────────┐
              │       CloudNode         │
              │                         │
              │  Camera detection ──────┼──► FFmpeg (HLS transcoding)
              │                         │           │
              │  Motion detector ───────┼────┐      ▼
              │  (scene-change FFmpeg)  │    │   .ts + .m3u8
              │                         │    │      │
              │  Dashboard (TUI)        │    │      ▼
              │                         │    │   ┌─────────────────┐
              │  HTTP server :8080 ◄────┼────┼───│  SegmentUploader │──push─► Command Center
              │  (local HLS + recs)     │    │   └─────────────────┘   (POST /push-segment)
              │                         │    │
              │  WebSocket client ◄─────┼────┘   motion event
              │  (+ HTTP fallback)      │    ───────► POST /api/.../motion (if WS is down)
              └─────────────────────────┘
```

**Video pipeline:** Camera → FFmpeg subprocess → rolling HLS segments (`.ts`) → `SegmentUploader` pushes each segment to Command Center via `POST /api/cameras/{id}/push-segment` → backend caches in memory → browser fetches via same-origin proxy. **No S3, no presigned URLs.**

**Playlist:** Every time FFmpeg rewrites `stream.m3u8`, CloudNode also POSTs the playlist text to `POST /api/cameras/{id}/playlist` so the backend's rewritten (relative-URL) copy stays fresh.

**Motion:** A second FFmpeg probe per camera emits scene-change scores. Above-threshold frames raise a `MotionEvent`, which is sent over the WebSocket (`/ws/node`) as an `event { command: "motion_detected" }`. If the socket is disconnected, the uploader falls back to `POST /api/cameras/{id}/motion`.

**Local storage:** SQLite database (`data/node.db`) stores configuration, snapshots, and recordings as BLOBs (not exposed in open folders). Retention is enforced automatically — oldest data is deleted first when `max_size_gb` is exceeded.

**Hardware encoding:** At startup, CloudNode probes for a hardware encoder (NVENC, QSV, AMF) and caches the result in the database. Falls back to `libx264` if none is found.

---

## API Endpoints

The node runs an HTTP server on port 8080 for local access:

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Health check |
| GET | `/hls/{camera_id}/stream.m3u8` | HLS playlist (local) |
| GET | `/hls/{camera_id}/segment_{n}.ts` | Video segment (local) |
| GET | `/recordings/list` | JSON list of stored recording filenames |
| GET | `/recordings/{file}` | Download a stored recording |
| GET | `/snapshots/list` | JSON list of stored snapshot filenames |
| GET | `/snapshots/{file}` | Download a stored snapshot |

These are intended for local consumption (e.g. `VITE_LOCAL_HLS=true` on the Command Center frontend). In normal cloud operation the browser fetches video through the backend proxy, not directly from the node.

**Outbound calls to Command Center** (via `reqwest`):

All authenticated outbound calls use the same header: **`X-Node-API-Key: <api_key>`**. The WebSocket is the only exception — it takes the key as a query parameter.

| Endpoint | Purpose |
|----------|---------|
| `POST /api/nodes/validate` | Validate a `node_id` + API key pair before saving config (setup wizard) |
| `POST /api/nodes/register` | Register node + cameras on startup |
| `POST /api/nodes/heartbeat` | Liveness (every 30s by default) |
| `POST /api/cameras/{id}/codec` | Report detected video/audio codec |
| `POST /api/cameras/{id}/push-segment?filename=…` | Push a `.ts` segment into the backend's in-memory cache |
| `POST /api/cameras/{id}/playlist` | Update the rewritten HLS playlist |
| `POST /api/cameras/{id}/motion` | Motion event fallback (used when WebSocket is down) |
| `WS /ws/node?api_key=…&node_id=…` | Bidirectional channel: heartbeat, commands, motion events (key passed as query param) |

---

## Development

### Build

```bash
cargo build              # Debug
cargo build --release    # Optimized
cargo test               # Run tests
cargo clippy             # Lint
cargo fmt -- --check     # Format check
```

### Project structure

```
src/
├── main.rs               # CLI entry point (clap)
├── lib.rs                # Library re-exports
├── dashboard.rs          # Live TUI dashboard + slash commands
├── error.rs              # Custom Error enum + Result type
├── logging.rs            # tracing subscriber setup
├── api/                  # Cloud API client + WebSocket
│   ├── client.rs         # ApiClient — register, heartbeat, codec, push-segment, playlist, motion
│   ├── websocket.rs      # WebSocket loop with auto-reconnect; sends motion events
│   ├── types.rs          # Request/response types
│   └── mod.rs
├── camera/               # Detection and capture (platform-specific)
│   ├── detector.rs       # Auto-detect USB cameras
│   ├── capture.rs        # Frame capture
│   ├── platform/         # Linux (v4l2) / Windows (DirectShow) / macOS (AVFoundation)
│   └── types.rs
├── config/               # Config loader (DB → YAML → env → CLI)
├── node/                 # Orchestration and lifecycle
│   └── runner.rs
├── server/               # Local HTTP server (warp) — health, HLS, recordings, snapshots
├── setup/                # Interactive TUI setup wizard (crossterm + inquire)
├── streaming/            # HLS pipeline
│   ├── hls_generator.rs   # FFmpeg subprocess per camera (HLS muxer)
│   ├── hls_uploader.rs    # Watches HLS dir, hands segments to SegmentUploader, updates playlist, drives motion events
│   ├── segment_uploader.rs# Posts each .ts to POST /push-segment with retry
│   ├── motion_detector.rs # Parallel FFmpeg scene-change scorer
│   └── codec_detector.rs  # FFprobe-based codec detection
└── storage/              # SQLite-backed local storage (BLOBs + config)
```

### Cross-compilation

```bash
# Raspberry Pi (ARM64)
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

---

## Platform Notes

<details>
<summary><strong>Linux</strong></summary>

Camera devices appear at `/dev/video*`. Add your user to the `video` group:

```bash
sudo usermod -a -G video $USER
# Log out and back in
```

Install FFmpeg:

```bash
sudo apt install ffmpeg        # Ubuntu / Debian
sudo dnf install ffmpeg        # Fedora
sudo pacman -S ffmpeg          # Arch
```

</details>

<details>
<summary><strong>Windows</strong></summary>

CloudNode runs natively on Windows using DirectShow. FFmpeg is downloaded automatically during setup to `./ffmpeg/bin/`.

Camera names (e.g. `MEE USB Camera`, `Integrated Webcam`) are detected via DirectShow enumeration.

</details>

<details>
<summary><strong>macOS</strong></summary>

Install FFmpeg via Homebrew:

```bash
brew install ffmpeg
```

You may need to grant camera access in **System Settings > Privacy & Security > Camera**.

</details>

---

## Troubleshooting

<details>
<summary><strong>No cameras detected</strong></summary>

**Linux:** Verify device exists and permissions are correct:

```bash
ls -l /dev/video*
# Should show crw-rw---- with group 'video'
```

Add your user to the video group if needed:

```bash
sudo usermod -a -G video $USER
```

**Windows:** Ensure the camera is not in use by another application (Zoom, Teams, etc.).

</details>

<details>
<summary><strong>FFmpeg not found</strong></summary>

**Windows:** Re-run `opensentry-cloudnode setup` — FFmpeg is downloaded automatically.

**Linux / macOS:** Install FFmpeg using your package manager (see [Platform Notes](#platform-notes)).

</details>

<details>
<summary><strong>HLS stream not playing</strong></summary>

1. Verify the node is running: `curl http://localhost:8080/health`
2. Check the dashboard for FFmpeg errors
3. Confirm HLS files are being created in `data/hls/`
4. Watch the dashboard log for `Pushed segment …` lines — those mean segments are reaching the backend
5. Try `/export-logs` from the settings page for detailed diagnostics

</details>

<details>
<summary><strong>Cannot connect to Command Center</strong></summary>

1. Verify your API URL: `curl https://your-backend.example.com/api/health`
2. Open `/settings` in the dashboard to confirm Node ID and API URL
3. Use `/reauth confirm` from settings to re-enter credentials

</details>

<details>
<summary><strong>Motion events not firing</strong></summary>

1. Check that `motion.enabled` is `true` (default)
2. Lower `motion.threshold` (default `0.02`) if the scene is dim / low-contrast
3. The dashboard logs `Motion detected on <camera> (score N%)` when an event fires

</details>

<details>
<summary><strong>Docker container can't access camera</strong></summary>

Pass each camera device explicitly:

```bash
docker run --device /dev/video0:/dev/video0 ...
```

</details>

---

## License

Licensed under the [GNU General Public License v3.0](LICENSE).

CloudNode uses GPL-3.0 to ensure users can always inspect, modify, and verify what runs on their cameras. For commercial licensing, contact [SourceBox LLC](https://github.com/SourceBox-LLC).

---

<p align="center">
  <a href="https://opensentry-command.fly.dev">OpenSentry Command Center</a>
  &middot;
  Made by the OpenSentry Team
</p>
