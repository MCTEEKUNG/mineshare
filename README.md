# MineShare

> KVM-over-IP + bidirectional audio/mic bridge between a Windows PC and an Ubuntu Linux PC.

Use one mouse, keyboard, mic, and headset to drive both PCs at once. Cursor flows seamlessly between every monitor of every machine. Keystrokes go to whichever PC the cursor is on. System audio and microphone bridge in both directions.

**Status:** early development — milestone **M0 (skeleton)**.

See [PLAN.md](PLAN.md) for the full design and roadmap.

## Repository layout

```
crates/
  mineshare-core/      Layout, source/routing FSMs, clipboard
  mineshare-net/       Discovery (mDNS), pairing (Noise XX), wire protocol
  mineshare-input/     Cross-platform mouse/keyboard capture + injection
  mineshare-audio/     System audio + mic capture/playback, virtual sink
  mineshare-ipc/       GUI <-> daemon protocol
  mineshare-daemon/    Background process binary
ui/                    Tauri 2 + React settings app
```

## Build & run

Requires Rust 1.94+.

```sh
cargo build --workspace
cargo test  --workspace
cargo run   --bin mineshare-daemon
```

Run the daemon on two machines on the same LAN. They discover each other
via mDNS (`_mineshare._tcp.local.`), complete a Noise XX handshake,
exchange UDP ports over the encrypted control channel, and start
forwarding mouse + keyboard input.

### M2 Slice 1 — edge-triggered cursor handover

The Windows side now gates forwarding by cursor location. When the cursor
reaches the **right edge** of the primary screen, the daemon enters
*remote* mode: the local cursor is warped to a centre anchor, mouse and
keyboard events are forwarded to the peer instead of being processed
locally, and a virtual `(virt_x, virt_y)` is tracked. When `virt_x` falls
back below zero, control is released and the Windows cursor is restored
at the right edge.

**Hardcoded layout**: Ubuntu sits to the right of Windows (single
monitor each). Multi-monitor and a draggable layout editor land in later
M2 slices.

For the test you'll want:

```sh
# Windows (the side with the physical mouse / keyboard)
cargo run --bin mineshare-daemon -- run

# Ubuntu (cursor target only — no forwarding back to Windows yet)
cargo run --bin mineshare-daemon -- run --no-capture
```

Push the cursor against the right edge of Windows; it should disappear
and the Ubuntu cursor should start moving. Drag back left until the
Windows cursor reappears at the right edge.

### Linux setup

Capture reads `/dev/input/event*`, injection writes `/dev/uinput`. Add
your user to the `input` group (recommended) or apply a udev rule:

```sh
sudo usermod -aG input "$USER"
# log out + back in for the group change to take effect
```

### Windows setup

Capture uses low-level hooks (`SetWindowsHookEx`) which require an
interactive desktop session — this works when running the daemon from a
normal terminal. Hooks ignore events flagged `LLMHF_INJECTED` so events
synthesised by our own injection don't loop back.

## Collecting logs from both machines

Each daemon writes a daily-rotating log to `<config_dir>/MineShare/logs/`
(`%APPDATA%\MineShare\logs` on Windows, `~/.config/MineShare/logs` on Linux).

To bundle the recent log + system info into the repo's `logs/` folder so we
can compare both sides:

```sh
# Capture a snapshot
cargo run --bin mineshare-daemon -- collect

# …and push it to GitHub in one go
cargo run --bin mineshare-daemon -- collect --push
```

The bundle ends up at `logs/<hostname>-<os>.log` (one file per machine).

## License

MIT — see [LICENSE](LICENSE).
