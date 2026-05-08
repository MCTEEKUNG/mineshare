#!/bin/bash
# Launch wrapper used by the dev-test harness over SSH.
#
# When invoked from an SSH session, DISPLAY / WAYLAND_DISPLAY /
# XDG_RUNTIME_DIR / DBUS_SESSION_BUS_ADDRESS aren't inherited from
# the active graphical login. Without them `arboard` (clipboard
# sync) fails its init with "X11 server connection timed out" and
# the daemon comes up otherwise healthy but with clipboard
# permanently disabled. Filling sensible defaults here lets the
# same script work from both a real terminal and SSH.
pkill -f mineshare-daemon 2>/dev/null
sleep 0.3
rm -f /tmp/mineshare-test.log
export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"
# arboard's default X11 backend needs MIT-MAGIC-COOKIE-1 from the
# user's xauth file to talk to the running display server. Over SSH
# we don't inherit XAUTHORITY, so point it at the home file the
# desktop session writes (works on stock Ubuntu/GNOME).
if [ -z "$XAUTHORITY" ]; then
  # GNOME on Wayland keeps Xwayland's auth cookie under XDG_RUNTIME_DIR
  # as `.mutter-Xwaylandauth.<random>` rather than the legacy
  # ~/.Xauthority. Pick whichever exists.
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
ARGS="${@:-run --no-inject}"
nohup /home/tee/mineshare/target/debug/mineshare-daemon $ARGS > /tmp/mineshare-test.log 2>&1 < /dev/null &
PID=$!
disown
sleep 0.4
if kill -0 $PID 2>/dev/null; then
  echo "STARTED pid=$PID"
else
  echo "FAILED to start"
  cat /tmp/mineshare-test.log
fi
