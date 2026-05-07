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

### M1 caveats

M1 forwards **all** captured events to the peer continuously — there is
no edge detection or source FSM yet (those land in M2). When two daemons
are connected, moving the mouse on either machine will move the cursor on
both. Use the safety flags during testing:

```sh
cargo run --bin mineshare-daemon -- run --no-inject     # capture, but don't inject received events
cargo run --bin mineshare-daemon -- run --no-capture    # only inject what the peer sends
```

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
