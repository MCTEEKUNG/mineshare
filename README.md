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

## Build (M0)

Requires Rust 1.94+.

```sh
cargo build --workspace
cargo test  --workspace
cargo run   --bin mineshare-daemon
```

Run the daemon on two machines on the same LAN — they will discover each other via mDNS (`_mineshare._tcp.local.`) and complete a Noise XX handshake.

## License

MIT — see [LICENSE](LICENSE).
