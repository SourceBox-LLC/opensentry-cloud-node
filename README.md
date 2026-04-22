<p align="center">
  <h1 align="center">SourceBox Sentry CloudNode</h1>
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

CloudNode runs on your local network, detects USB cameras, and streams live video to the [SourceBox Sentry Command Center](https://opensentry-command.fly.dev) via HLS. All configuration is stored locally in an encrypted SQLite database — no cloud dependency for setup.

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
- An [SourceBox Sentry Command Center](https://opensentry-command.fly.dev) account with a Node ID and API Key (generated from the Settings page)
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

Press **Esc** to return from settings. Destructive commands (`/wipe`, `/reauth`) require confirmation: either press the command **twice within 30 seconds** — the first press arms the confirmation, the second executes — or pass the `confirm` argument explicitly (`/wipe confirm`). Any other command in between clears the armed state.

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

The API key is **encrypted at rest** using AES-256-GCM with a machine-derived key (SHA-256 of the OS machine identifier + application salt). CloudNode reads the identifier from `/etc/machine-id` on Linux, `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` on Windows, and `IOPlatformUUID` on macOS — values that are set once at OS install time, unique per host, and not user-modifiable. Moving `node.db` to a different host makes the stored key unreadable.

DBs written by older CloudNode versions that derived the key from the hostname are transparently re-encrypted with the new machine-ID-derived key on first load.

**Docker:** Alpine-based images don't ship with `/etc/machine-id`, so CloudNode generates a per-container ID on first run and stores it inside the mounted data volume (`$OPENSENTRY_DATA_DIR/.machine-id`). The ID persists across container rebuilds because it lives in the volume. For stronger encryption — a key tied to the host rather than the data volume — run the container with `-v /etc/machine-id:/etc/machine-id:ro`.

---

## Docker

Prebuilt multi-arch images (`linux/amd64`, `linux/arm64`) are published to GitHub Container Registry on every release tag.

### Quick run (prebuilt image — recommended)

```bash
docker pull ghcr.io/sourcebox-llc/opensentry-cloudnode:latest

docker run -d \
  --name opensentry-cloudnode \
  --device /dev/video0:/dev/video0 \
  -e OPENSENTRY_NODE_ID=your_node_id \
  -e OPENSENTRY_API_KEY=your_api_key \
  -e OPENSENTRY_API_URL=https://your-backend.example.com \
  -p 8080:8080 \
  -v ./data:/app/data \
  ghcr.io/sourcebox-llc/opensentry-cloudnode:latest
```

Pin to a specific release instead of `:latest` when you want reproducible deploys — e.g. `ghcr.io/sourcebox-llc/opensentry-cloudnode:0.1.16`. Major.minor tags like `:0.1` are also published and float to the newest patch. See [releases](https://github.com/SourceBox-LLC/opensentry-cloud-node/releases) for the current version.

### Docker Compose

```bash
cp .env.example .env
# Edit .env with your credentials
docker-compose up -d
```

> The bundled `docker-compose.yml` currently builds from source (`build: .`). To use the prebuilt image instead, swap the `build:` line for `image: ghcr.io/sourcebox-llc/opensentry-cloudnode:latest` and run `docker compose pull && docker compose up -d`.

### Build from source (dev / airgapped)

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
  ghcr.io/sourcebox-llc/opensentry-cloudnode:latest
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
| GET | `/health` | Health check (also used by the Docker `HEALTHCHECK`) |
| GET | `/hls/{camera_id}/stream.m3u8` | HLS playlist (local) |
| GET | `/hls/{camera_id}/segment_{n}.ts` | Video segment (local) — filename must be `segment_<digits>.ts` |

The server has **no authentication** and binds to `127.0.0.1` by default — only the local machine can reach it. If you need LAN-local HLS playback (e.g. `VITE_LOCAL_HLS=true` on the Command Center frontend from another device), set `server.bind = "0.0.0.0"` in your config — and understand you're exposing live video to everyone on that network. In normal cloud operation the browser fetches video through the backend proxy, not directly from the node.

Recordings and snapshots now live inside the encrypted SQLite database rather than on the filesystem; the old `/recordings/*` and `/snapshots/*` routes were removed.

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
<summary><strong>Raspberry Pi (4 / 5, 64-bit)</strong></summary>

CloudNode runs on 64-bit Raspberry Pi OS. Build from source (the prebuilt ARM64 Docker image also works, but a native build skips the container USB-passthrough setup):

```bash
sudo apt install -y build-essential pkg-config libssl-dev ffmpeg
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
git clone https://github.com/SourceBox-LLC/opensentry-cloud-node.git
cd opensentry-cloud-node
cargo build --release
./target/release/opensentry-cloudnode setup
```

The first `cargo build --release` on a Pi 4 takes 15–20 minutes. Subsequent incremental builds after `git pull` are 1–3 minutes.

**Software encoding only.** CloudNode deliberately does **not** use the Pi's `h264_v4l2m2m` hardware encoder — it produces a malformed SPS that the browser's Media Source Extensions layer rejects (video never appears, even though FFmpeg reports success). The node encodes with `libx264 -preset ultrafast` instead, which sustains 1080p30 at about 1.5 cores per camera on a Pi 4. A Pi 4 comfortably runs 2 cameras at 1080p30; a Pi 5 runs 3–4.

**USB cameras.** Plug webcams **directly into the Pi's USB ports** rather than through a hub when possible — unpowered hubs frequently brown out under the combined draw of two UVC cameras streaming at 1080p, and a hub fault can wedge the entire xhci controller until reboot. Use the blue USB 3.0 ports (top pair) for more power budget even if the camera only needs USB 2.0 bandwidth.

**Thermal.** Two simultaneous `libx264` streams will push a bare Pi 4 past 80 °C and trip the kernel's thermal throttle, dropping frame rate. A heatsink and fan are a small upgrade that make this non-issue. Check with `vcgencmd measure_temp` and `vcgencmd get_throttled` (anything other than `throttled=0x0` indicates a power or thermal event).

</details>

<details>
<summary><strong>Windows</strong></summary>

CloudNode runs natively on Windows using DirectShow. FFmpeg is downloaded automatically during setup to `./ffmpeg/bin/`.

Camera names (e.g. `MEE USB Camera`, `Integrated Webcam`) are detected via DirectShow enumeration.

**WSL2 deployment (optional):** The setup wizard on Windows can also deploy inside WSL2 (useful for Linux-native builds and tighter V4L2 integration). The wizard's WSL preflight detects whether WSL is installed, finds a usable distro, installs FFmpeg inside it, and prints the `usbipd bind` / `usbipd attach --wsl` commands you need to run in an admin PowerShell to forward USB cameras from the host into the distro. Elevation-required steps (installing WSL, installing `usbipd-win`, `usbipd bind`) are printed for the operator to run rather than executed on their behalf.

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

For the full end-to-end "live video isn't showing up in the dashboard" workflow, see [`docs/runbooks/video-not-showing.md`](docs/runbooks/video-not-showing.md). The most common causes are also captured below.

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

<details>
<summary><strong>FFmpeg exits with status 234 in a restart loop (encoder open failure)</strong></summary>

The dashboard log shows repeated `FFmpeg exited with exit status: 234` and messages like `Could not open encoder before EOF` or `Error parsing option '...' with value '...'`. This means the selected encoder can't be initialized with the current argument set.

1. Look for the line `Selected encoder: <name>` or `Using software encoder (configured)` in the startup log to see which encoder was picked.
2. If the encoder is a hardware codec (`h264_nvenc`, `h264_qsv`, `h264_amf`) and the driver on this machine is broken, override with software: set `OPENSENTRY_ENCODER=libx264` or change it via the setup wizard.
3. On CloudNode ≥ v0.1.14 the `h264_v4l2m2m` Pi codec is automatically retired — a stale DB entry naming it is cleared on next launch (look for `Retired encoder '…' in config — clearing for re-detection`). If you're on an older node, run `/wipe` from the dashboard's settings page to clear the cached encoder and let auto-detect re-run.
4. If libx264 itself crashes with `Error parsing option 'level' with value 'auto'`, upgrade — that was a bug in the libx264 branch of `build_encoding_args` fixed in v0.1.15.

</details>

<details>
<summary><strong>Raspberry Pi: cameras drop off the USB bus under load</strong></summary>

Symptoms: cameras appear at startup then disappear after minutes/hours; `lsusb` shows only root hubs; `dmesg` shows `usb usb1-port1: disabled by hub (EMI?)` or `device descriptor read/64, error -110` or `xhci_hcd ... Setup ERROR`.

This is almost always a USB hub fault or power issue, not a software problem.

1. **Reboot** (`sudo reboot`). The xhci controller can wedge in a state hot-replugging doesn't recover; only a kernel restart clears it.
2. **Plug cameras directly into the Pi's ports** — remove any external hub, splitter, or extension cable from the path. Unpowered hubs routinely brown out under two 1080p UVC cameras.
3. **Check power** — `vcgencmd get_throttled` must return `0x0`. Any non-zero value means under-voltage or over-current events have happened; use the official 5V/3A USB-C supply.
4. **Try USB 3.0 (blue) ports** — more power budget than USB 2.0 (black) even for USB 2.0 devices.
5. **Verify post-reboot** — `lsusb` should show your camera's VID:PID (e.g. `1bcf:2283 Sunplus ... MEE USB Camera`), and `/dev/video0` / `/dev/video2` should exist.

If a full power cycle, direct-to-Pi connection, and official PSU all fail, the camera itself is the most likely cause — test each camera alone on the Pi before replacing hubs.

</details>

<details>
<summary><strong>Video plays in the dashboard but browser shows a black frame</strong></summary>

The CloudNode dashboard's `STREAMING` status and the ↑ segs counter only prove that segments are being produced and pushed to the backend — not that they're decodable by the browser's MSE (Media Source Extensions) layer.

1. In the Command Center dashboard, check the camera card's codec badge. A valid line like `avc1.42e01f` (Baseline, Level 3.1) or `avc1.4d401e` (Main, Level 3.0) means the SPS is parseable. A missing codec badge or `avc1.000000` means the encoder produced a malformed bitstream.
2. On the node, run `ffprobe` against a recent segment:
   ```bash
   ls -t data/hls/*/segment_*.ts | head -1 | xargs ffprobe 2>&1 | head -20
   ```
   Valid output shows a recognizable `Profile` (Baseline / Main / High) and a positive `Level` (e.g. `Level: 31`). If you see `Profile: unknown` or `Level: -99`, the encoder is producing garbage — force `libx264` as described in the encoder-crash entry above.

</details>

---

## License

Licensed under the [GNU General Public License v3.0](LICENSE).

CloudNode uses GPL-3.0 to ensure users can always inspect, modify, and verify what runs on their cameras. For commercial licensing, contact [SourceBox LLC](https://github.com/SourceBox-LLC).

---

<p align="center">
  <a href="https://opensentry-command.fly.dev">SourceBox Sentry Command Center</a>
  &middot;
  Made by the SourceBox Sentry Team
</p>
