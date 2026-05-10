# MineShare

> KVM-over-IP + bidirectional audio/mic bridge between **Windows** and **Ubuntu Linux** PCs over a LAN.

Drive two PCs with one keyboard + one mouse. Hear and speak through both. Cursor flows between machines like a single virtual desktop; clipboard, audio, microphone follow.

**Status:** working end-to-end on Win 11 + Ubuntu 24.04 (Wayland/GNOME). Single user, single LAN. Pre-1.0 — see [PLAN.md](PLAN.md) for the full design.

![status: pre-1.0](https://img.shields.io/badge/status-pre--1.0-orange) ![rust](https://img.shields.io/badge/rust-1.94+-orange) ![license](https://img.shields.io/badge/license-MIT-blue)

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
| 🎮 **Game-mode lock** | Auto-engages when fullscreen anti-cheat games (BattlEye / EAC / Vanguard / RICOCHET / Hyperion) are foregrounded — pins input to the local PC so accidental edge-crosses can't ban your account. Manual toggle: **Ctrl+Alt+L**. |
| 📊 **Latency telemetry** | Round-trip Ping/Pong every 500 ms with rolling histogram in the GUI. |

**Non-goals:** macOS, internet/NAT traversal, video mirroring, file transfer.

---

## How I actually use it

This is the workflow this project was built around. The goal: leave the **Win laptop** as primary input, drop the **Ubuntu PC** beside it as a second screen + workhorse, never plug the keyboard into Ubuntu.

```
┌─────────────────────────┐         LAN         ┌─────────────────────────┐
│  Windows 11 (laptop)    │  ◄─ encrypted ─►   │  Ubuntu 24.04 (desktop) │
│  2880×1800 @ 200% DPI   │                     │  1920×1080              │
│                         │                     │                         │
│  • USB keyboard         │                     │  • USB mouse            │
│  • Trackpad             │                     │  • (no keyboard needed) │
│  • Headset (mic + spk)  │                     │  • Speakers             │
└─────────────────────────┘                     └─────────────────────────┘
```

Daily flow:

1. **Boot both PCs.** GUI launches automatically (Win Startup folder, Linux systemd user service). They find each other via mDNS within ~1 s.
2. **First launch ever:** a 6-digit PIN appears on one machine; type it on the other → trusted forever.
3. **Move cursor right** off the Win laptop → it appears on Ubuntu.
4. **Click an Ubuntu app** with the Ubuntu mouse → Smart mode notices, routes the Win keyboard there. Type Discord messages, terminal commands, whatever — keys land on Ubuntu while the Win laptop stays on whatever it was doing.
5. **Move Win cursor or click Win** → keys snap back to Win within ~1.5 s.
6. **Open Chrome on Ubuntu, play music** → audio comes through the Win headset.
7. **Voice-call from Win Discord** → Ubuntu apps see the Win headset mic as an input device (`Monitor of MineShare-Mic`).

Hotkeys:

| Combo | Effect |
|---|---|
| `Ctrl+Alt+R` | Toggle Remote: enter / exit / ask peer to release |
| `Ctrl+Alt+K` | Cycle keyboard target: Smart → Pin-to-peer → Pin-to-local → Auto |
| `Ctrl+Alt+L` | Game-mode lock: pin all input here, ignore edge crossings |

---

## Install

Pre-built installers are attached to each [GitHub Release](https://github.com/mineshare/mineshare/releases). For end users that's the recommended path — no toolchain required.

### Windows

Download `MineShare_<ver>_x64-setup.exe` (NSIS, ~3 MB) **or** `MineShare_<ver>_x64_en-US.msi` (WiX, ~5 MB) → double-click → installer puts MineShare in `%LOCALAPPDATA%\Programs\MineShare` and adds a Start Menu shortcut.

> **First-launch SmartScreen warning** is expected because the binary isn't code-signed yet (this is a hobby project). Click **More info → Run anyway**. See [SECURITY.md](SECURITY.md) once published for the verification checklist.

### Ubuntu / Debian

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
