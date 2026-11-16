#!/usr/bin/env bash
# Isolated tests: no real app is started and real home/autostart is untouched.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
work=$(mktemp -d)
trap '[[ ! -f "$work/pid" ]] || kill "$(cat "$work/pid")" 2>/dev/null || true; rm -rf -- "$work"' EXIT
export HOME="$work/home with spaces"
export XDG_CONFIG_HOME="$HOME/config"
export XDG_DATA_HOME="$HOME/data"
export XDG_RUNTIME_DIR="$work/runtime"
mkdir -p "$HOME/.local/bin" "$XDG_CONFIG_HOME/autostart" "$XDG_RUNTIME_DIR" "$work/bin"
printf '[Desktop Entry]\nType=Application\nName=original MineShare\nExec=/bin/true\n' > "$XDG_CONFIG_HOME/autostart/MineShare.desktop"
printf '[Desktop Entry]\nType=Application\nName=original Typeless\nExec=/bin/true\n' > "$XDG_CONFIG_HOME/autostart/typeless.desktop"
cat > "$work/mineshare-app.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
int main(void) {
  char path[4096];
  snprintf(path, sizeof(path), "%s/mineshare-keyboard-broker.ready", getenv("XDG_RUNTIME_DIR"));
  FILE *f = fopen(path, "w");
  if (!f) return 1;
  fprintf(f, "%d\n3\n", getpid()); fclose(f);
  snprintf(path, sizeof(path), "%s/pid", getenv("TEST_WORK"));
  f = fopen(path, "w"); fprintf(f, "%d\n", getpid()); fclose(f);
  sleep(120); return 0;
}
EOF
cc "$work/mineshare-app.c" -o "$HOME/.local/bin/mineshare-app"
export TEST_WORK="$work"
for script in install-typeless-compat.sh start-typeless-after-mineshare.sh prepare-typeless-autostart.sh; do
    bash -n "$root/installer/linux/$script"
done
bash "$root/installer/linux/install-typeless-compat.sh" "$HOME/.local/bin/mineshare-app"
[[ -f "$XDG_CONFIG_HOME/MineShare/keyboard-broker.enabled" ]]
cmp "$XDG_CONFIG_HOME/autostart/MineShare.desktop" "$XDG_CONFIG_HOME/autostart/typeless.desktop"
guard="$XDG_CONFIG_HOME/systemd/user/app-typeless@autostart.service.d/50-mineshare-order.conf"
grep -Fq "ConditionPathExists=!$XDG_CONFIG_HOME/MineShare/keyboard-broker.enabled" "$guard"
# GNOME's direct autostart path must be repaired before the manager starts.
session_guard="$XDG_CONFIG_HOME/systemd/user/gnome-session-manager@.service.d/50-mineshare-order.conf"
grep -Fq 'ExecStartPre=-"%h/.local/bin/mineshare-typeless-prepare"' "$session_guard"
# Vendor removal must be repaired without rerunning the installer.
rm "$XDG_CONFIG_HOME/autostart/typeless.desktop"
[[ -f "$guard" ]]
"$HOME/.local/bin/mineshare-typeless-prepare"
cmp "$XDG_CONFIG_HOME/autostart/MineShare.desktop" "$XDG_CONFIG_HOME/autostart/typeless.desktop"
# A vendor rewrite must also be routed back through broker readiness.
printf '[Desktop Entry]\nType=Application\nName=Typeless\nExec=/bin/false\n' > "$XDG_CONFIG_HOME/autostart/typeless.desktop"
"$HOME/.local/bin/mineshare-typeless-prepare"
cmp "$XDG_CONFIG_HOME/autostart/MineShare.desktop" "$XDG_CONFIG_HOME/autostart/typeless.desktop"
# Disabling the opt-in must leave vendor/user startup choices untouched.
mv "$XDG_CONFIG_HOME/MineShare/keyboard-broker.enabled" "$work/broker.enabled"
printf 'user-owned startup\n' > "$XDG_CONFIG_HOME/autostart/typeless.desktop"
"$HOME/.local/bin/mineshare-typeless-prepare"
grep -qx 'user-owned startup' "$XDG_CONFIG_HOME/autostart/typeless.desktop"
mv "$work/broker.enabled" "$XDG_CONFIG_HOME/MineShare/keyboard-broker.enabled"
"$HOME/.local/bin/mineshare-typeless-prepare"
echo 'PASS: pre-session repair survives vendor deletion/rewrite and respects opt-out'
bash "$root/installer/linux/install-typeless-compat.sh" "$HOME/.local/bin/mineshare-app"
cmp "$XDG_CONFIG_HOME/autostart/MineShare.desktop" "$XDG_CONFIG_HOME/autostart/typeless.desktop"
find "$XDG_DATA_HOME/MineShare/backups" -name typeless.desktop -exec grep -q 'original Typeless' {} \;
# Pretend Typeless is already open. A vendor app must NEVER start in this test.
printf '#!/bin/sh\nexit 0\n' > "$work/bin/pgrep"
chmod +x "$work/bin/pgrep"
export PATH="$work/bin:$PATH"
printf '99999999\n3\n' > "$XDG_RUNTIME_DIR/mineshare-keyboard-broker.ready"
"$HOME/.local/bin/mineshare-typeless-start" &
first=$!
"$HOME/.local/bin/mineshare-typeless-start" &
second=$!
wait "$first"; wait "$second"
[[ $(head -n 1 "$XDG_RUNTIME_DIR/mineshare-keyboard-broker.ready") == "$(cat "$work/pid")" ]]
echo 'PASS: spaced HOME, backup, desktop validation, stale PID, readiness and concurrent startup'
kill "$(cat "$work/pid")"; rm "$work/pid"
rm "$XDG_RUNTIME_DIR/mineshare-keyboard-broker.ready"
printf '#!/bin/sh\nexit 1\n' > "$HOME/.local/bin/mineshare-app"
printf '#!/bin/sh\nexit 0\n' > "$work/bin/notify-send"; chmod +x "$work/bin/notify-send"
if "$HOME/.local/bin/mineshare-typeless-start"; then
    echo 'FAIL: startup accepted missing broker readiness' >&2; exit 1
fi
echo 'PASS: missing broker readiness fails closed without starting Typeless'
# Fresh .deb installation has no old ~/.local/bin launcher or autostart entry.
cp "$HOME/.local/bin/mineshare-app" "$work/package-mineshare-app"
rm "$HOME/.local/bin/mineshare-app" "$XDG_CONFIG_HOME/autostart/MineShare.desktop"
bash "$root/installer/linux/install-typeless-compat.sh" "$work/package-mineshare-app"
[[ $(cat "$XDG_DATA_HOME/MineShare/keyboard-broker-launcher") == "$work/package-mineshare-app" ]]
[[ -f "$XDG_CONFIG_HOME/autostart/MineShare.desktop" ]]
echo 'PASS: package executable selected without legacy launcher or existing autostart'
