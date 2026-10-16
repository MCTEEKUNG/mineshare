#!/usr/bin/env bash
# Readiness, not an arbitrary delay, orders exclusive keyboard owners.
set -euo pipefail
: "${XDG_RUNTIME_DIR:?A graphical login session is required}"
exec 9>"$XDG_RUNTIME_DIR/mineshare-typeless-start.lock"
flock -x 9
ready="$XDG_RUNTIME_DIR/mineshare-keyboard-broker.ready"
is_ready() {
    [[ -r "$ready" ]] || return 1
    local pid count
    { read -r pid; read -r count; } < "$ready" || return 1
    [[ "$pid" =~ ^[0-9]+$ && "$count" =~ ^[1-9][0-9]*$ ]] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    [[ $(readlink "/proc/$pid/exe" 2>/dev/null) == *mineshare-app* ]]
}
if ! is_ready; then
    "$HOME/.local/bin/mineshare-app" >>"$XDG_RUNTIME_DIR/mineshare-startup.log" 2>&1 9>&- &
    for ((attempt=0; attempt<150; attempt++)); do
        is_ready && break
        sleep 0.2
    done
fi
if ! is_ready; then
    message='MineShare could not acquire all physical keyboards. Typeless was not auto-started, to avoid losing remote keys. Check MineShare logs; start MineShare before Typeless.'
    printf '%s\n' "$message" >&2
    command -v notify-send >/dev/null && notify-send 'MineShare keyboard startup conflict' "$message" || true
    exit 1
fi
# Give listeners a device-discovery turn; ordering is enforced by readiness above.
sleep 0.3
is_ready || exit 1
if ! pgrep -u "$(id -u)" -x typeless >/dev/null; then
    # Preserve the vendor login flags/environment (e.g. secret-store backend).
    original="${XDG_DATA_HOME:-$HOME/.local/share}/MineShare/typeless-login.desktop"
    [[ -r "$original" ]] || { echo 'Missing saved Typeless startup entry.' >&2; exit 1; }
    gio launch "$original" >>"$XDG_RUNTIME_DIR/typeless-startup.log" 2>&1 9>&- &
fi
