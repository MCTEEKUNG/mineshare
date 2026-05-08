#!/bin/bash
# Reverse of install.sh — stop + disable the unit, drop the udev
# rule and the binary. We deliberately leave the user in the
# `input` group (cheap to keep, and other apps may rely on it).

set -euo pipefail

INSTALL_PREFIX="${INSTALL_PREFIX:-/usr/local}"

if [ "$EUID" -ne 0 ]; then
  echo "==> elevating to root"
  exec sudo --preserve-env=INSTALL_PREFIX "$0" "$@"
fi

TARGET_USER="${SUDO_USER:-$USER}"
TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"
UNIT_PATH="$TARGET_HOME/.config/systemd/user/mineshare-daemon.service"

if [ -f "$UNIT_PATH" ]; then
  echo "==> stopping + disabling mineshare-daemon.service"
  sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$(id -u "$TARGET_USER")" \
    systemctl --user disable --now mineshare-daemon.service 2>/dev/null || true
  rm -f "$UNIT_PATH"
  sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$(id -u "$TARGET_USER")" \
    systemctl --user daemon-reload || true
fi

echo "==> removing /etc/udev/rules.d/60-mineshare.rules"
rm -f /etc/udev/rules.d/60-mineshare.rules
udevadm control --reload-rules || true

echo "==> removing $INSTALL_PREFIX/bin/mineshare-daemon"
rm -f "$INSTALL_PREFIX/bin/mineshare-daemon"

echo
echo "==> done. ($TARGET_USER kept in 'input' group; remove manually with"
echo "    'sudo gpasswd -d $TARGET_USER input' if you don't want it.)"
