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

CloudNode runs on your local network, detects USB cameras, and streams live video to the [OpenSentry Command Center](https://github.com/SourceBox-LLC/OpenSentry-Command) via HLS. All configuration is stored locally in an encrypted SQLite database — no cloud dependency for setup.

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
- An [OpenSentry Command Center](https://github.com/SourceBox-LLC/OpenSentry-Command) account with a Node ID and API Key
- **Docker** (recommended) or **Rust 1.70+** with **FFmpeg**

### Setup

```bash
# Clone and build
git clone https://github.com/SourceBox-LLC/OpenSentry-CloudNode.git
cd OpenSentry-CloudNode
cargo build --release

# Run the interactive setup wizard
./target/release/opensentry-cloudnode setup
```

The setup wizard handles everything automatically:

1. Detects your platform and connected cameras
2. Downloads FFmpeg if needed (Windows)
3. Prompts for your Node ID and API Key
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

1. **SQLite database** (`data/node.db`) — created by setup wizard
2. **YAML file** (`config.yaml`) — legacy fallback, auto-migrated to DB on first load
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
                      |
              ┌───────┴───────┐
              │   CloudNode   │
              │               │
              │  Camera       │
              │  Detection ───┼──► FFmpeg (HLS transcoding)
              │               │         |
              │  Dashboard    │    .ts segments + .m3u8
              │  (TUI)        │         |
              │               │    ┌────┴─────┐
              │  HTTP :8080 ◄─┼────┤ Uploader │
              │               │    └────┬─────┘
              │  WebSocket ◄──┼─┐       |
              └───────────────┘ │       ▼
                                │  Command Center
                                └── (cloud API)
```

**Video pipeline:** Camera → FFmpeg subprocess → HLS segments (`.ts`) → uploaded to Command Center via presigned URLs.

**Local storage:** SQLite database (`data/node.db`) stores configuration, snapshots, and recordings as BLOBs. Retention is enforced automatically — oldest data is deleted first when `max_size_gb` is exceeded.

**Hardware encoding:** At startup, CloudNode probes for a hardware encoder (NVENC, QSV, AMF) and caches the result in the database. Falls back to `libx264` if none is found.

---

## API Endpoints

The node runs an HTTP server on port 8080:

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health check |
| `GET /hls/{camera_id}/stream.m3u8` | HLS playlist |
| `GET /hls/{camera_id}/segment_{n}.ts` | Video segment |

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
├── main.rs              # CLI entry point (clap)
├── dashboard.rs         # Live TUI dashboard
├── api/                 # Cloud API client + WebSocket
├── camera/              # Detection and capture (platform-specific)
├── config/              # Config loading (DB → YAML → env → CLI)
├── node/                # Orchestration and lifecycle
├── server/              # HTTP server (warp)
├── setup/               # Interactive setup wizard
├── streaming/           # HLS generation and segment upload
└── storage/             # SQLite database
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
4. Try `/export-logs` from the settings page for detailed diagnostics

</details>

<details>
<summary><strong>Cannot connect to Command Center</strong></summary>

1. Verify your API URL: `curl https://your-backend.example.com/api/health`
2. Open `/settings` in the dashboard to confirm Node ID and API URL
3. Use `/reauth confirm` from settings to re-enter credentials

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
  <a href="https://github.com/SourceBox-LLC/OpenSentry-Command">OpenSentry Command Center</a>
  &middot;
  Made by the OpenSentry Team
</p>
