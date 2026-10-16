<div align="center">
  <img src="docs/media/mineshare-logo.png" width="80" alt="MineShare gold M logo" />
  <h1>MineShare</h1>
  <p><strong>Two computers. One familiar setup.</strong></p>
  <p>Keep your keyboard, mouse and headset plugged into your PC.<br />Bring your laptop to the desk. Let your focus move between them.</p>

  <a href="#the-desk-workflow">See the workflow</a> ·
  <a href="#what-it-does">Explore features</a> ·
  <a href="#install">Set up MineShare</a> ·
  <a href="docs/ubuntu-readiness.md">Ubuntu support</a>
  <br /><br />

  <img alt="Platforms: Windows 11 and Ubuntu" src="https://img.shields.io/badge/Windows_11_+_Ubuntu-LAN_bridge-bd9653?style=flat-square" />
  <img alt="Status: pre-1.0" src="https://img.shields.io/badge/status-pre--1.0-d1ad6c?style=flat-square" />
  <img alt="Rust 1.94+" src="https://img.shields.io/badge/Rust-1.94+-43474b?style=flat-square" />
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-688477?style=flat-square" /></a>
</div>

<br />

[![An illustrated desk workflow: a developer arrives, sets down a Windows laptop, and moves one cursor from the PC to the laptop. Keyboard, mouse and headset cables remain connected to the PC.](docs/media/desk-story.gif)](docs/media/desk-story.mp4)

<p align="center"><a href="docs/media/desk-story.mp4">Watch the 18-second MP4</a> · <a href="docs/media/desk-story-poster.png">Still preview</a> · <a href="media/promo">Edit the animation</a></p>

> **About the animation:** an original workflow illustration, not a screen recording, benchmark, or proof of compatibility. The app interface is simplified for storytelling. Desktop autostart requires setup; trusted peers must already be paired on the same LAN.

MineShare is a Rust + Tauri desktop bridge for **Windows and Ubuntu**: shared input, clipboard, files and bidirectional audio over an encrypted local connection. No video streaming or cloud relay is required for the bridge.

**Development snapshot:** `0.0.12` · keyboard broker commit [`9dd85f7`](https://github.com/MCTEEKUNG/mineshare/commit/9dd85f7). Ubuntu Typeless coexistence is opt-in and still awaits the post-login physical acceptance test. See [setup and limitations](docs/linux-typeless-keyboard-broker.md). Published installers below may predate this source snapshot; do not assume that “latest release” contains every development change.

---

## ⬇️ Download

Grab the newest installer for your OS from the **[latest release ›](https://github.com/MCTEEKUNG/mineshare/releases/latest)** — no toolchain required.

| OS | File | Install |
|---|---|---|
| 🪟 **Windows** | [`MineShare_<ver>_x64-setup.exe`](https://github.com/MCTEEKUNG/mineshare/releases/latest) (NSIS, ~3 MB) or [`_x64_en-US.msi`](https://github.com/MCTEEKUNG/mineshare/releases/latest) (WiX, ~5 MB) | double-click → install |
| 🐧 **Ubuntu / Debian** | [`MineShare_<ver>_amd64.deb`](https://github.com/MCTEEKUNG/mineshare/releases/latest) | `sudo apt install ./<file>` |
| 🎩 **Fedora / RHEL** | [`MineShare-<ver>-1.x86_64.rpm`](https://github.com/MCTEEKUNG/mineshare/releases/latest) | `sudo rpm -i <file>` |
| 🐧 **Any Linux** | [`MineShare_<ver>_amd64.AppImage`](https://github.com/MCTEEKUNG/mineshare/releases/latest) | `chmod +x <file>` → run |

> Binaries aren't code-signed yet (hobby project). On Windows, SmartScreen will warn on first launch — click **More info → Run anyway**. Full setup notes in [Install](#install) below.

---

## What it does

| Feature | Notes |
|---|---|
| 🖱️ **Cursor crossing** | Push the cursor against an edge → it appears on the other PC. Edge configurable per machine via the **Layout** tab. Multi-monitor aware on both sides. |
| ⌨️ **Smart keyboard routing** | Keys follow whichever side you're actively using. Defaults to "Smart" mode: the most-recently-clicked machine wins focus, sticky during typing pauses. Pin to either side with **Ctrl+Alt+K**. |
| 🔊 **System sound bridge** | What's playing on Win → audible on Ubuntu, and vice-versa. WASAPI loopback (Win) ↔ PipeWire monitor (Linux) ↔ Opus over UDP. |
| 🎙️ **Microphone bridge** | Your mic on either PC becomes an input device on the peer (PipeWire null-sink on Linux, VB-CABLE on Win). |
| 📋 **Clipboard sync** | Text copy/paste flows both directions automatically. |
| 📁 **File transfer** | Drag a file onto the MineShare window → streams to peer over the encrypted channel, lands at `Downloads/MineShare/<name>` with SHA-256 integrity check. Live progress + cancel. |
| 🔒 **Encrypted by default** | Noise XX handshake → ChaCha20-Poly1305 AEAD on every byte. PIN-pairing once per device, trust-on-first-use. |
| 📡 **Zero-config discovery** | mDNS announces `_mineshare._tcp.local.`; both ends auto-connect on the same LAN. |
| 🎮 **Game-mode lock** | User-controlled lock on the focused desktop; toggle with **Ctrl+Shift+L**. Opening a game does not automatically enable the lock. |
| 📊 **Latency telemetry** | Round-trip Ping/Pong every 500 ms with rolling histogram in the GUI. |

**Non-goals:** macOS, internet/NAT traversal, video mirroring.

---

## The desk workflow

| Arrive | Connect | Keep going |
|---|---|---|
| Set the Windows laptop in the center of the desk. The desktop PC is already running. | Configured desktop autostart launches MineShare; previously paired peers discover each other over the LAN. | Move the PC mouse across the configured edge. Use the same keyboard on the focused machine. |

The **keyboard, mouse and headset remain physically connected to the desktop PC**. MineShare carries the input and configured audio streams between computers; no unplugging or USB dock handoff is part of this illustration.

1. Install MineShare on both machines, pair them, and configure their relative screen positions.
2. Enable desktop autostart: the per-user Windows logon task or [Ubuntu graphical launcher](installer/linux/install-desktop.sh).
3. Move the mouse against the configured edge to control the other machine. Smart keyboard routing follows input ownership; explicit pin modes are also available.
4. Configure the audio bridge for the playback device you use. Microphone routing has additional platform requirements; see [installation](#install).
5. Return across the edge to keep working on the PC. No video mirror is needed; each machine keeps its own screen.

For Typeless on Ubuntu, use the [ordered keyboard-broker setup](docs/linux-typeless-keyboard-broker.md), then log out/in once. It is designed to let Typeless stay open on both machines and receive Right Alt only on the routed destination. That behavior still needs the documented physical acceptance test; it is not implied by the animation.

Hotkeys:

| Combo | Effect |
|---|---|
| `Ctrl+Alt+R` | Toggle Remote: enter / exit / ask peer to release |
| `Ctrl+Alt+K` | Cycle keyboard target: Smart → Pin-to-peer → Pin-to-local → Auto |
| `Ctrl+Shift+L` | Game-mode lock on the focused desktop |

---

## Install

Pre-built installers are attached to each [GitHub Release](https://github.com/MCTEEKUNG/mineshare/releases). For end users that's the recommended path — no toolchain required.

### Windows

Download `MineShare_<ver>_x64-setup.exe` (NSIS, ~3 MB) **or** `MineShare_<ver>_x64_en-US.msi` (WiX, ~5 MB) → double-click → installer puts MineShare in `%LOCALAPPDATA%\Programs\MineShare` and adds a Start Menu shortcut.

> **First-launch SmartScreen warning** is expected because the binary isn't code-signed yet (this is a hobby project). Click **More info → Run anyway**. See [SECURITY.md](SECURITY.md) once published for the verification checklist.

### Ubuntu / Debian

For current cross-platform checks, runtime requirements, and physical acceptance steps, see [Ubuntu readiness](docs/ubuntu-readiness.md). CI checks both Ubuntu 22.04 and 24.04; passing compilation is not a physical Wayland/X11 input test.

Download `mineshare_<ver>_amd64.deb` and install:

```bash
sudo apt install ./mineshare_<ver>_amd64.deb
# OR via dpkg + apt for dep resolution:
sudo dpkg -i mineshare_<ver>_amd64.deb
sudo apt-get install -f
```

The `.deb` post-install script:
- Adds the invoking user to the `input` group (writable `/dev/uinput`)
- Drops a udev rule so the kernel grants 0660 root:input on `/dev/uinput`
- Reloads udev so the rule applies without a reboot

**Log out and back in** so the input-group membership takes effect, then start MineShare from the application menu.

A portable `.AppImage` is also provided if you don't want a system install:

```bash
chmod +x MineShare_<ver>_amd64.AppImage
./MineShare_<ver>_amd64.AppImage
# (you'll still need to be in the `input` group manually:
#  sudo usermod -aG input "$USER")
```

## Building installers from source

You'll need Rust **1.94+**, Bun (or npm), and the platform deps below.

### Windows
```powershell
git clone https://github.com/<you>/mineshare.git
cd mineshare
.\scripts\build-installer.ps1
# Artifacts at ui\src-tauri\target\release\bundle\{msi,nsis}\
```

### Ubuntu / Debian

System deps for cpal + Tauri 2 WebKit + PipeWire audio:
```bash
sudo apt install -y \
    libwebkit2gtk-4.1-dev libssl-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev \
    libasound2-dev libpipewire-0.3-dev pulseaudio-utils \
    libopus-dev pkg-config build-essential
```

Then:
```bash
git clone https://github.com/<you>/mineshare.git
cd mineshare
./scripts/build-installer.sh
# Artifacts at ui/src-tauri/target/release/bundle/{deb,appimage,rpm}/
```

### Cross-platform via GitHub Actions

`.github/workflows/release.yml` builds both Windows + Linux installers on every `v*` tag push and attaches them to a draft GitHub Release. Tag, push, then publish the draft:

```bash
git tag v0.1.0
git push origin v0.1.0
# CI builds .msi + setup.exe + .deb + .AppImage + .rpm in ~10 minutes
```

### Optional: install the standalone CLI daemon

For headless setups (server-style, no GUI):
```bash
cargo build --release -p mineshare-daemon --bin mineshare-daemon
sudo bash installer/linux/install.sh
```

```powershell
cargo build --release -p mineshare-daemon --bin mineshare-daemon
powershell -ExecutionPolicy Bypass -File installer\windows\install.ps1
```

> ⚠️ Don't run **both** the GUI app and the standalone daemon on the same machine — they'll conflict on mDNS announcements and pair-fail in a loop. Pick one. The Tauri GUI embeds the same daemon code, so you almost always want just the GUI.

---

## Architecture

```
                   ┌─────────────── ui/ ────────────────┐
                   │  React + Vite + TypeScript          │
                   │  Tauri 2 webview + IPC              │
                   └─────────────────────────────────────┘
                                    │ Tauri commands
                                    ▼
┌────────────────────────── ui/src-tauri/ ────────────────────────────┐
│  embedded daemon runtime + system tray + window mgmt                 │
└──────────────────────────────────────────────────────────────────────┘
                                    │
        ┌───────────────────────────┴───────────────────────────┐
        ▼                                                       ▼
┌─────────────────────┐                              ┌─────────────────────┐
│ mineshare-daemon    │  Encrypted UDP + TCP control │ mineshare-daemon    │
│  • runtime FSM      │ ◄──────────────────────────► │  • runtime FSM      │
│  • pairing / trust  │   (ChaCha20-Poly1305, mDNS)  │  • pairing / trust  │
│  • settings, logs   │                              │  • settings, logs   │
└─────────────────────┘                              └─────────────────────┘
        │                                                       │
        ├─ mineshare-input  (capture + inject)                   │
        ├─ mineshare-audio  (Opus + WASAPI/PipeWire/cpal)        │
        ├─ mineshare-net    (Noise XX, mDNS, AEAD framing)       │
        ├─ mineshare-ipc    (in-process daemon ↔ GUI types)      │
        └─ mineshare-core   (layout, peer-side, FSM helpers)     │
                  Windows 11                              Ubuntu 24.04
```

| Crate | Responsibility |
|---|---|
| `mineshare-core` | Layout config, `PeerSide`, FSM helpers shared across crates. |
| `mineshare-net` | mDNS service browsing, Noise XX handshake, AEAD message framing. |
| `mineshare-input` | OS capture (WH_*_LL hooks on Win, evdev on Linux) and injection (enigo on Win, uinput on Linux). Smart keyboard routing, held-key tracking, click-to-focus. |
| `mineshare-audio` | WASAPI loopback (Win), PipeWire monitor via parec (Linux), cpal playback, Opus codec, virtual-mic sinks (PipeWire null-sink / VB-CABLE). |
| `mineshare-ipc` | Daemon ↔ GUI shared message types. |
| `mineshare-daemon` | Runtime that orchestrates everything: peer discovery, pairing, control channel writer/reader, audio/input wiring, latency tracking, settings. |
| `ui/` (React) | Settings GUI: Status pills, Layout drag editor, Devices picker, Audio toggles, Hotkeys reference, Latency histogram. |
| `ui/src-tauri/` | Tauri 2 shell that bundles the daemon as an in-process library, plus tray icon and window event handling. |

---

## Build & develop

```bash
# Workspace cargo
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace -- -D warnings

# Headless daemon (no GUI)
cargo run --bin mineshare-daemon -- run

# Tauri GUI (frontend hot-reload)
cd ui
bun install
bun tauri dev          # spins up Vite + Tauri shell, hot-reload on save

# Production build (per-platform installer artifacts)
bun tauri build
```

Logs land in `<config_dir>/MineShare/logs/daemon.YYYY-MM-DD`:
- Win: `%APPDATA%\MineShare\logs\`
- Linux: `~/.config/MineShare/logs/`

Crash files land in `<config_dir>/MineShare/crashes/`.

To bundle a snapshot of both machines' logs into one repo for diffing:
```bash
cargo run --bin mineshare-daemon -- collect          # local only
cargo run --bin mineshare-daemon -- collect --push   # commit + push to repo
```

---

## License

MIT — see [LICENSE](LICENSE).
