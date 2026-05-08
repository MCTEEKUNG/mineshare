# MineShare — Linux install

Manual installer for the daemon side. Wraps:

* The udev rule that grants the `input` group rw access to
  `/dev/uinput` (so the daemon can open it without root).
* A **user-mode** systemd unit that auto-starts on login,
  inherits the graphical session env (DISPLAY / WAYLAND_DISPLAY /
  XAUTHORITY / DBUS) and restarts on failure.
* Group-membership setup for the invoking user.

The Slice 4 deb wraps these same artifacts in a `.deb` postinst.

## install

From a fresh checkout, after building the daemon:

```bash
cargo build --release -p mineshare-daemon --bin mineshare-daemon

# from the repo root:
sudo ./installer/linux/install.sh
```

The script will:

1. copy `target/release/mineshare-daemon` → `/usr/local/bin/`
2. drop `60-mineshare.rules` into `/etc/udev/rules.d/` and reload
3. add you to the `input` group (if missing)
4. install + enable `~/.config/systemd/user/mineshare-daemon.service`

If you weren't already in the `input` group, **log out and back in
once** so the new group membership takes effect for `/dev/uinput`.

## verify

```bash
systemctl --user status mineshare-daemon
journalctl --user -u mineshare-daemon -f
```

mDNS should advertise this host inside ~1s of startup; the peer
should pick it up via the existing browse loop.

## uninstall

```bash
sudo ./installer/linux/uninstall.sh
```

## notes

* `INSTALL_PREFIX` env var overrides where the binary lands
  (default `/usr/local`). Useful for distro packagers.
* The unit hooks into `graphical-session.target`, so it won't
  start on a console-only login. That's intentional — the daemon
  needs a desktop session for clipboard + audio.
* The systemd unit imports DISPLAY / WAYLAND_DISPLAY / XAUTHORITY
  via `systemctl --user import-environment` at start. This is the
  same machinery GNOME uses to make GUI apps reachable from
  systemd user services. On non-GNOME compositors that don't
  publish those by default, the import is a best-effort no-op
  and clipboard sync will degrade gracefully.
