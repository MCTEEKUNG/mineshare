#!/bin/bash
# Dev-test launcher for the Tauri GUI shell. Same env-import logic
# as launch-mineshare.sh (the headless daemon launcher) — Tauri
# needs DISPLAY / WAYLAND_DISPLAY / XAUTHORITY / DBUS to open a
# window and reach the user's clipboard, and SSH sessions don't
# inherit any of those by default.
pkill -f mineshare-app 2>/dev/null
pkill -f mineshare-daemon 2>/dev/null
sleep 0.3
rm -f /tmp/mineshare-gui.log

export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"
if [ -z "$XAUTHORITY" ]; then
  for cand in "$XDG_RUNTIME_DIR"/.mutter-Xwaylandauth.* "$HOME/.Xauthority"; do
    if [ -f "$cand" ]; then
      export XAUTHORITY="$cand"
      break
    fi
  done
fi
if [ -z "$WAYLAND_DISPLAY" ] && [ -S "$XDG_RUNTIME_DIR/wayland-0" ]; then
  export WAYLAND_DISPLAY=wayland-0
fi
# WebKitGTK on stock GNOME-Wayland sometimes goes black inside a
# DMABUF compositor — fall back to the shared-memory renderer to
# avoid the issue. Cheap and reversible.
export WEBKIT_DISABLE_DMABUF_RENDERER=1

# Prefer the release binary (frontend dist embedded). The debug
# binary expects a Vite dev server at localhost:1420 and shows
# "Connection refused" inside the WebView when launched outside
# `bun tauri dev`.
BIN_RELEASE="$HOME/mineshare/ui/src-tauri/target/release/mineshare-app"
BIN_DEBUG="$HOME/mineshare/ui/src-tauri/target/debug/mineshare-app"
if [ -x "$BIN_RELEASE" ]; then
  BIN="$BIN_RELEASE"
else
  BIN="$BIN_DEBUG"
fi
nohup "$BIN" > /tmp/mineshare-gui.log 2>&1 < /dev/null &
PID=$!
disown
sleep 0.6
if kill -0 "$PID" 2>/dev/null; then
  echo "STARTED pid=$PID"
else
  echo "FAILED to start"
  cat /tmp/mineshare-gui.log
fi
