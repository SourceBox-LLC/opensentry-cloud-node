# OpenSentry CloudNode

**Turn any USB webcam into a cloud-connected security camera.**

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

This node runs locally at your home/office, captures video from USB cameras, and streams to the OpenSentry Command Center in the cloud.

---

## 🎨 Beautiful Animated Setup Experience

OpenSentry CloudNode now features a stunning animated terminal UI for setup:

```
   ██████╗ ███████╗███████╗██████╗ ██████╗
   ██╔══██╗██╔════╝██╔════╝██╔══██╗██╔══██╗
   ██████╔╝█████╗  █████╗  ██████╔╝██║  ██║
   ██╔══██║██╔══╝  ██╔══╝  ██╔══██╗██║  ██║
   ██║  ██║███████╗███████╗██║  ██║██████╔╝
   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝  ╚═╝╚═════╝
   
   ✓ CloudNode Setup v0.1.0

┌────────────────────────────────────────────────────────┐
│  Step 1/5: System Prerequisites                      │
╰────────────────────────────────────────────────────────┯

[████████████████░░░░░░░░░░] 40% ━━━━━━━━ 8s

  ✓ Platform: Windows 11 (x86_64)
  ✓ Camera: MEE USB Camera (1920x1080)
  ⏳ Downloading FFmpeg...
  
[████████████░░░░░░░░░░░░░░░] 60% Downloaded 24MB of 40MB
```

### One Command Setup

```bash
opensentry-cloudnode setup
```

The setup wizard will:
- ✅ Detect your platform (Windows/Linux/macOS)
- ✅ Auto-detect USB cameras
- ✅ Download FFmpeg automatically (Windows)
- ✅ Prompt for credentials from Command Center
- ✅ Store config in SQLite database (API key encrypted at rest)
- ✅ Detect and configure hardware video encoder (NVENC/QSV/AMF)
- ✅ Verify everything works
- ✅ Display next steps

---

## Platform Support

| Platform | Camera API | FFmpeg Input | Status |
|----------|-----------|--------------|--------|
| **Linux** | Video4Linux2 (v4l2) | `-f v4l2 -i /dev/video0` | ✅ **Production Ready** |
| **Windows** | DirectShow | `-f dshow -i "video=Camera Name"` | ✅ **Working** |
| **macOS** | AVFoundation | `-f avfoundation -i "0"` | ⚠️ **Untested** |

---

## Quick Start

### Prerequisites

- **Camera**: USB webcam (required)
- **OpenSentry Command Center** account
- **One of the following:**
  - **Docker Desktop** (recommended)
  - **Rust toolchain + FFmpeg** (for native build)

### Installation

#### **Windows (Native)**

**NEW:** OpenSentry CloudNode now runs natively on Windows using DirectShow for camera access.

1. **Open PowerShell** and navigate to the repository:
   ```powershell
   cd C:\path\to\OpenSentry-CloudNode
   ```

2. **Run the setup script:**
   ```powershell
   .\setup.ps1
   ```

3. **Choose deployment method:**
   - **Option 1: Windows Native** (Recommended)
     - Runs directly on Windows
     - Uses DirectShow for camera access
     - FFmpeg auto-downloaded (~40MB)
     - Requires Rust toolchain
   
   - **Option 2: WSL2 (Linux)**
     - Runs inside Windows Subsystem for Linux
     - Requires USB camera passthrough
     - Good for testing Linux deployment

4. **Follow the prompts:**
   - Enter Node ID and API Key from Command Center
   - FFmpeg will be downloaded automatically (native Windows)
   - Binary will be built using `cargo build`

5. **Access your camera:**
   - Dashboard: http://localhost:5173
   - Health check: http://localhost:8080/health

**Windows Native Notes:**
- FFmpeg is downloaded to `.\ffmpeg\bin\` during setup
- Add `.\ffmpeg\bin` to PATH or use full path
- For production, create a Windows service or startup shortcut

#### **Windows (WSL2)**

If you choose WSL2 deployment:

1. **Open PowerShell as Administrator** and navigate to the repository:
   ```powershell
   cd C:\path\to\OpenSentry-CloudNode
   ```

2. **Run the setup script:**
   ```powershell
   .\setup.ps1
   ```

3. **Choose Option 2 (WSL2)** when prompted

4. **USB Camera Passthrough:**
   - **Windows 11 (build 22000+):** Automatic USB support
   - **Windows 10:** Requires `usbipd` tool:
     ```powershell
     winget install usbipd
     usbipd wsl attach --busid <BUSID>
     ```

#### **Linux / macOS**

1. **Open a terminal** and navigate to the repository:
   ```bash
   cd /path/to/OpenSentry-CloudNode
   ```

2. **Make the setup script executable:**
   ```bash
   chmod +x setup.sh
   ```

3. **Run the setup script:**
   ```bash
   ./setup.sh
   ```

4. **Choose deployment method:**
   - **Docker** (Recommended): Includes FFmpeg, easy updates
   - **Native**: Requires Rust + FFmpeg, for development

5. **Follow the prompts** to enter credentials and complete setup.

**Linux Notes:**
- Camera devices appear as `/dev/video*`
- Ensure your user is in the `video` group: `sudo usermod -a -G video $USER`

**macOS Notes:**
- Install FFmpeg: `brew install ffmpeg`
- Camera access may require permission in System Preferences → Security & Privacy

### Manual Installation

<details>
<summary>Click to expand manual installation steps</summary>

#### **Docker (Linux/macOS)**

```bash
# Build and run
docker build -t opensentry-cloudnode:latest .
docker run -d \
  --name opensentry-cloudnode \
  --device /dev/video0:/dev/video0 \
  -e OPENSENTRY_NODE_ID=your_node_id \
  -e OPENSENTRY_API_KEY=org_sk_your_key \
  -e OPENSENTRY_API_URL=https://your-backend.fly.dev \
  -p 8080:8080 \
  -v ./data:/app/data \
  opensentry-cloudnode:latest
```

#### **Native (Linux/macOS)**

```bash
# Install FFmpeg
sudo apt install ffmpeg  # Ubuntu/Debian
# or: brew install ffmpeg  # macOS

# Build
cargo build --release

# Run setup wizard (creates config in data/node.db)
./target/release/opensentry-cloudnode setup

# Or run directly with env vars
export OPENSENTRY_NODE_ID=your_node_id
export OPENSENTRY_API_KEY=org_sk_your_key
export OPENSENTRY_API_URL=https://your-backend.fly.dev
./target/release/opensentry-cloudnode
```

#### **Docker Desktop (Windows - Experimental)**

USB passthrough in Docker Desktop is experimental. WSL2 is recommended instead.

If you want to try Docker Desktop:
```powershell
# Build
docker build -t opensentry-cloudnode:latest .

# Run (USB device may not work reliably)
docker run -d `
  --name opensentry-cloudnode `
  --env-file .env `
  -p 8080:8080 `
  -v ${PWD}\data:/app/data `
  opensentry-cloudnode:latest
```

</details>

---

## Configuration

### SQLite Database (Primary)

Configuration is stored in `data/node.db` — a local SQLite database created automatically by the setup wizard. The API key is **encrypted at rest** using AES-256-GCM with a machine-derived key (moving the database to another machine makes the key unreadable).

View your current configuration with the `/settings` command in the dashboard TUI.

### Environment Variables (Override)

Environment variables can override database values (useful for debugging or CI):

| Variable | Description |
|----------|-------------|
| `OPENSENTRY_NODE_ID` | Override node ID |
| `OPENSENTRY_API_KEY` | Override API key |
| `OPENSENTRY_API_URL` | Override Command Center URL |
| `OPENSENTRY_ENCODER` | Override video encoder (e.g. `h264_nvenc`, `libx264`) |
| `RUST_LOG` | Log level (trace, debug, info, warn, error) |

### Configuration File (Legacy)

A `config.yaml` file is still supported as a fallback. On first run with a YAML/env config, values are automatically migrated to the SQLite database.

```yaml
node:
  name: "Home Camera"

cloud:
  api_url: "https://your-backend.fly.dev"
  heartbeat_interval: 30

streaming:
  fps: 30
  jpeg_quality: 85
  hls:
    enabled: true
    segment_duration: 2
    playlist_size: 5
    bitrate: "2000k"

storage:
  path: "./data"
  max_size_gb: 64

server:
  port: 8080
  bind: "0.0.0.0"

logging:
  level: "info"
```

---

## Docker Reference

### Build Image

```bash
docker build -t opensentry-cloudnode:latest .
```

### Run Container

```bash
docker run -d \
  --name opensentry-cloudnode \
  --device /dev/video0:/dev/video0 \
  -e OPENSENTRY_NODE_ID=your_node_id \
  -e OPENSENTRY_API_KEY=org_sk_your_key \
  -e OPENSENTRY_API_URL=https://your-backend.fly.dev \
  -p 8080:8080 \
  -v ./data:/app/data \
  opensentry-cloudnode:latest
```

### Multiple Cameras

```bash
docker run -d \
  --device /dev/video0:/dev/video0 \
  --device /dev/video1:/dev/video1 \
  --device /dev/video2:/dev/video2 \
  ...
```

---

## API Endpoints (Node HTTP Server)

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/health` | GET | Health check |
| `/hls/{camera_id}/stream.m3u8` | GET | HLS playlist for camera |
| `/hls/{camera_id}/segment_{n}.ts` | GET | HLS video segment |
| `/recordings` | GET | List recordings |
| `/recordings/{path}` | GET | Download recording |
| `/snapshots` | GET | List snapshots |
| `/snapshots/{path}` | GET | Download snapshot |

---

## HLS Streaming

OpenSentry CloudNode uses **HTTP Live Streaming (HLS)** for video delivery:

1. **FFmpeg** transcodes camera video into HLS segments (`.ts` files)
2. **Playlist** (`stream.m3u8`) references the latest segments
3. **HTTP Server** serves playlist and segments on port 8080
4. **Frontend** (HLS.js) plays the stream in any browser

### Stream URL Format

```
http://localhost:8080/hls/{camera_id}/stream.m3u8
```

---

## Dashboard TUI

While running, the node displays a live terminal dashboard with camera status, upload stats, and a command bar.

### Slash Commands

Type `/` and press Enter to see available commands. Commands vary by page:

**Main dashboard:**
| Command | Description |
|---------|-------------|
| `/settings` | Open settings page |
| `/status` | Show node status summary |
| `/clear` | Clear the log |
| `/quit` | Stop the node |

**Settings page:**
| Command | Description |
|---------|-------------|
| `/export-logs` | Save all logs to a timestamped file |
| `/wipe` | Erase all stored data and reset the node |
| `/reauth` | Clear credentials and re-run setup |
| `/back` | Return to dashboard |

Press `Esc` to go back from the settings page. Destructive commands (`/wipe`, `/reauth`) require a `confirm` argument (e.g. `/wipe confirm`).

---

## Development

### Prerequisites

- Rust 1.70+ 
- FFmpeg (for HLS generation)

### Build

```bash
cargo build
```

### Run in dev mode

```bash
OPENSENTRY_NODE_ID=test \
OPENSENTRY_API_KEY=org_sk_xxx \
cargo run
```

### Run tests

```bash
cargo test
```

### Build for Raspberry Pi (ARM64)

```bash
cargo build --release --target aarch64-unknown-linux-gnu
```

---

## Troubleshooting

### Windows (WSL2) Issues

#### **USB Camera Not Detected in WSL2**

**Windows 11 (Build 22000+):**
USB devices are automatically attached to WSL2. If your camera isn't detected:

1. Restart WSL2:
   ```powershell
   wsl --shutdown
   ```

2. Reopen WSL2 and check:
   ```bash
   ls -l /dev/video*
   ```

**Windows 10:**
USB devices require `usbipd` to attach to WSL2:

1. Install usbipd on Windows:
   ```powershell
   winget install usbipd
   ```

2. List USB devices:
   ```powershell
   usbipd list
   ```

3. Attach camera to WSL2:
   ```powershell
   usbipd bind --busid <BUSID>
   usbipd wsl attach --busid <BUSID>
   ```

4. Verify in WSL2:
   ```bash
   ls -l /dev/video*
   ```

#### **Ubuntu Not Installed in WSL2**

Run the setup script again, or install manually:

```powershell
wsl --install -d Ubuntu
```

Create a UNIX username and password when prompted.

#### **WSL2 Network Issues**

If CloudNode can't connect to the Command Center:

1. Check WSL2 network:
   ```bash
   ping ulises-nonruinable-shapelessly.ngrok-free.dev
   ```

2. Use Windows host IP from WSL2:
   ```bash
   # In WSL2
   export OPENSENTRY_API_URL=http://$(hostname).local:8000
   ```

### No cameras detected (Linux)

Ensure your user has permission to access video devices:

```bash
sudo usermod -a -G video $USER
# Log out and log back in
```

Verify device exists:
```bash
ls -l /dev/video*
```

Check permissions:
```bash
# Should show crw-rw---- video
ls -l /dev/video0
```

### FFmpeg not found (native build)

Install FFmpeg:

```bash
# Ubuntu/Debian
sudo apt update && sudo apt install ffmpeg

# Fedora
sudo dnf install ffmpeg

# Arch Linux
sudo pacman -S ffmpeg

# macOS
brew install ffmpeg
```

### Docker container can't access camera

Make sure to pass the device:

```bash
docker run --device /dev/video0:/dev/video0 ...
```

For multiple cameras:
```bash
docker run \
  --device /dev/video0:/dev/video0 \
  --device /dev/video1:/dev/video1 \
  ...
```

### HLS stream not playing

1. Check FFmpeg is running:
   ```bash
   docker logs opensentry-cloudnode | grep FFmpeg
   # or
   ps aux | grep ffmpeg
   ```

2. Check HLS files exist:
   ```bash
   ls -l ./data/hls/
   ```

3. Verify HTTP server:
   ```bash
   curl http://localhost:8080/health
   ```

4. Check camera permissions:
   ```bash
   # Linux
   ls -l /dev/video*
   ```

### "Permission denied" errors

Add your user to the `video` group:

```bash
sudo usermod -a -G video $USER
# Log out and log back in
```

### Connection refused to Command Center

1. Verify API URL:
   ```bash
   curl https://your-backend.fly.dev/api/health
   ```

2. Check credentials: use the `/settings` command in the dashboard TUI to verify your Node ID and API URL.

3. Re-authenticate: use `/reauth` from the settings page to clear credentials and re-run setup.

---

---

## Updating

To update CloudNode to the latest version:

### **Windows (WSL2)**
```powershell
.\setup.ps1 -Update
```

### **Linux / macOS**
```bash
./setup.sh --update
```

---

## Uninstalling

To uninstall CloudNode:

### **Windows (WSL2)**
```powershell
.\uninstall.ps1
```

Options:
- `-RemoveData`: Remove all data directories (recordings, snapshots, HLS)
- `-RemoveImage`: Remove Docker image
- `-RemoveWSL`: Remove Ubuntu from WSL2 (removes ALL WSL data)

### **Linux / macOS**
```bash
./uninstall.sh
```

Options:
- `--remove-data`: Remove all data directories
- `--remove-image`: Remove Docker image

---

## Architecture

**Linux / macOS:**
```
┌─────────────────────────────────────┐
│         Docker Container            │
│  ┌───────────────────────────────┐  │
│  │      CloudNode (Rust)        │  │
│  │  ┌─────────┐   ┌───────────┐  │  │
│  │  │ Camera  │   │   HLS     │  │  │
│  │  │ Capture │ → │ Generator │  │  │
│  │  └─────────┘   └───────────┘  │  │
│  │  ┌───────────────────────────┐│  │
│  │  │   HTTP Server (port 8080) ││  │
│  │  └───────────────────────────┘│  │
│  └───────────────────────────────┘  │
│                                      │
│  FFmpeg (bundled)                    │
└─────────────────────────────────────┘
         │
         ↓ API requests
    ┌────────────┐
    │  Backend   │ (Command Center)
    │  (Fly.io)  │
    └────────────┘
         │
         ↓ HLS streaming
    ┌────────────┐
    │  Frontend  │ (Browser)
    │  HLS.js    │
    └────────────┘
```

**Windows (WSL2):**
```
┌─────────────────────────────────────────────┐
│              Windows Host                    │
│  ┌───────────────────────────────────────┐  │
│  │         WSL2 (Ubuntu)                  │  │
│  │  ┌─────────────────────────────────┐  │  │
│  │  │   Docker Container               │  │  │
│  │  │   ┌──────────────────────────┐  │  │  │
│  │  │   │    CloudNode (Rust)     │  │  │  │
│  │  │   │  ┌────────┐  ┌────────┐ │  │  │  │
│  │  │   │  │Camera  │  │  HLS   │ │  │  │  │
│  │  │   │  │Capture │→ │Generator│ │  │  │  │
│  │  │   │  └────────┘  └────────┘ │  │  │  │
│  │  │   │  ┌──────────────────────┐│  │  │  │
│  │  │   │  │ HTTP Server :8080   ││  │  │  │
│  │  │   │  └──────────────────────┘│  │  │  │
│  │  │   └──────────────────────────┘  │  │  │
│  │  │   FFmpeg (bundled)              │  │  │
│  │  └─────────────────────────────────┘  │  │
│  │                                        │  │
│  │   USB Camera ←─(usbipd/WSL2 passthrough)│
│  └───────────────────────────────────────┘  │
└─────────────────────────────────────────────┘
         │
         ↓ API requests
    ┌────────────┐
    │  Backend   │ (Command Center)
    │  (Fly.io)  │
    └────────────┘
         │
         ↓ HLS streaming (localhost:8080)
    ┌────────────┐
    │  Frontend  │ (Browser)
    │  HLS.js    │
    └────────────┘
```

---

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| Linux (x86_64) | ✅ Supported | Primary platform |
| Linux (ARM64) | ✅ Supported | Raspberry Pi 4+ |
| Linux (ARM) | ✅ Supported | Raspberry Pi 3+ |
| macOS (Intel) | ✅ Supported | Requires FFmpeg |
| macOS (Apple Silicon) | ✅ Supported | Requires FFmpeg |
| Windows 10 | ⚠️ Experimental | Requires usbipd for USB camera |
| Windows 11 | ✅ Supported | WSL2 with automatic USB support |

---

## Development

### Project Structure

```
OpenSentry-CloudNode/
├── setup.ps1              # Windows setup script
├── setup.sh               # Linux/macOS setup script
├── Cargo.toml             # Rust project configuration
├── src/
│   ├── main.rs            # Entry point (clap CLI)
│   ├── dashboard.rs       # Live TUI dashboard with slash commands
│   ├── camera/            # Camera detection and capture
│   ├── config/            # Configuration loading (DB → YAML → env)
│   ├── node/              # Node orchestration
│   ├── server/            # HTTP server (warp)
│   ├── streaming/         # HLS generation and upload
│   ├── storage/           # SQLite database (snapshots, recordings, config)
│   ├── setup/             # Interactive TUI setup wizard
│   └── api/               # Cloud API + WebSocket client
└── data/                  # Data directory (created at runtime)
    ├── node.db            # SQLite database (config, snapshots, recordings)
    └── hls/               # HLS segment cache (per-camera subdirectories)
```

### Building from Source

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Run in development mode
export OPENSENTRY_NODE_ID=test
export OPENSENTRY_API_KEY=org_sk_xxx
cargo run
```

### Cross-Compilation

Build for Raspberry Pi (ARM64):
```bash
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

---

## License

This project is licensed under the **GNU General Public License v3.0** - see the [LICENSE](LICENSE) file for details.

### Why GPL-3.0?

OpenSentry CloudNode uses GPL-3.0 because:

- **Freedom Preservation**: Ensures users always have access to source code for the software running on their cameras
- **Community Contributions**: Improvements must be shared back to the community when distributed
- **Surveillance Transparency**: Users can inspect, modify, and verify what the software does with their video feeds
- **Patent Protection**: Includes explicit patent grants, important for video/surveillance technology

### For Commercial Use

If you need to use OpenSentry CloudNode in a commercial product that cannot comply with GPL-3.0, please contact SourceBox LLC for dual-licensing options.

### Contributing

By contributing to this project, you agree that your contributions will be licensed under GPL-3.0.

---

**[OpenSentry Command Center](https://github.com/SourceBox-LLC/OpenSentry-Command)** · Made with ❤️ by the OpenSentry Team