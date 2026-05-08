#!/bin/bash
# MineShare Linux installer (manual).
#
# Steps:
#   1. Copy daemon binary to /usr/local/bin
#   2. Drop udev rule + reload
#   3. Add invoking user to `input` group (for /dev/uinput access)
#   4. Install systemd user unit + enable for the invoking user
#
# After running, log out and back in once so the new group
# membership takes effect, then verify with:
#   systemctl --user status mineshare-daemon

set -euo pipefail

INSTALL_PREFIX="${INSTALL_PREFIX:-/usr/local}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILT_BIN="$REPO_ROOT/target/release/mineshare-daemon"

if [ ! -x "$BUILT_BIN" ]; then
  echo "error: $BUILT_BIN not found"
  echo "build first:  cargo build --release -p mineshare-daemon --bin mineshare-daemon"
  exit 1
fi

# Re-exec under sudo for the system-level steps. We keep the user's
# UID via $SUDO_USER so we can later install the systemd USER unit
# into the right home dir.
if [ "$EUID" -ne 0 ]; then
  echo "==> elevating to root for system-level steps"
  exec sudo --preserve-env=INSTALL_PREFIX "$0" "$@"
fi

TARGET_USER="${SUDO_USER:-$USER}"
if [ -z "$TARGET_USER" ] || [ "$TARGET_USER" = "root" ]; then
  echo "error: cannot determine non-root target user"
  echo "       run as a normal user: sudo not required at the entry point."
  exit 1
fi
TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"

echo "==> installing $BUILT_BIN -> $INSTALL_PREFIX/bin/mineshare-daemon"
install -m 0755 "$BUILT_BIN" "$INSTALL_PREFIX/bin/mineshare-daemon"

echo "==> installing udev rule -> /etc/udev/rules.d/60-mineshare.rules"
install -m 0644 "$SCRIPT_DIR/60-mineshare.rules" /etc/udev/rules.d/60-mineshare.rules
udevadm control --reload-rules
udevadm trigger --subsystem-match=misc --attr-match=name=uinput || true

echo "==> ensuring 'input' group membership for $TARGET_USER"
if id -nG "$TARGET_USER" | grep -qw input; then
  echo "    already in input group"
else
  usermod -aG input "$TARGET_USER"
  NEEDS_RELOGIN=1
fi

UNIT_DIR="$TARGET_HOME/.config/systemd/user"
echo "==> installing systemd user unit -> $UNIT_DIR/mineshare-daemon.service"
install -d -o "$TARGET_USER" -g "$TARGET_USER" "$UNIT_DIR"
install -m 0644 -o "$TARGET_USER" -g "$TARGET_USER" \
  "$SCRIPT_DIR/mineshare-daemon.service" "$UNIT_DIR/mineshare-daemon.service"

echo "==> enabling unit for $TARGET_USER"
sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$(id -u "$TARGET_USER")" \
  systemctl --user daemon-reload
sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$(id -u "$TARGET_USER")" \
  systemctl --user enable --now mineshare-daemon.service || {
    echo "    enable failed (likely no graphical session yet)."
    echo "    will start automatically on next login."
  }

echo
echo "==> done"
if [ "${NEEDS_RELOGIN:-0}" = 1 ]; then
  echo
  echo "  IMPORTANT: log out and back in once so the input-group"
  echo "             membership takes effect for /dev/uinput access."
fi
echo
echo "  status:    systemctl --user status mineshare-daemon"
echo "  logs:      journalctl --user -u mineshare-daemon -f"
echo "  uninstall: sudo $SCRIPT_DIR/uninstall.sh"
