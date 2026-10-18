#!/bin/bash
# Function-level lifecycle tests: all process signals and user-bus calls are mocked.
set -euo pipefail
directory=$(cd "$(dirname "$0")" && pwd)
source "$directory/deb-lifecycle.sh"
tmp=$(mktemp -d /tmp/mineshare-deb-test.XXXXXX)
trap '[[ $tmp == /tmp/mineshare-deb-test.* ]] && rm -rf -- "$tmp"' EXIT
ms_valid_exe /usr/bin/mineshare-app
ms_valid_exe '/usr/bin/mineshare-app (deleted)'
for invalid in /tmp/mineshare-app /usr/bin/typeless /home/user/mineshare-app /usr/bin/mineshare-app-helper; do
    if ms_valid_exe "$invalid"; then echo "Matched unrelated executable: $invalid"; exit 1; fi
done

ms_identity() {
    [[ ! -e $tmp/gone ]] || return 1
    if [[ -e $tmp/reused ]]; then echo 1000:99; else echo 1000:10; fi
}
sleep() { :; }
kill() {
    printf '%s\n' "$*" >> "$tmp/signals"
    if [[ $mode == reused ]]; then touch "$tmp/reused";
    elif [[ $mode == graceful || $1 == -KILL ]]; then touch "$tmp/gone"; fi
}
for mode in graceful hung reused; do
    rm -f "$tmp/gone" "$tmp/reused" "$tmp/signals"
    ms_stop_one 123 1000:10
    grep -qx -- '-TERM 123' "$tmp/signals"
    if [[ $mode == hung ]]; then
        [[ $(wc -l < "$tmp/signals") == 2 ]]
        grep -qx -- '-KILL 123' "$tmp/signals"
    else
        [[ $(wc -l < "$tmp/signals") == 1 ]]
    fi
done
rm -f "$tmp/signals"
touch "$tmp/gone"
ms_stop_one 123 1000:10
[[ ! -e $tmp/signals ]]

# Restart runs through the user's systemd manager; no GUI is run as root.
ms_identity() { return 1; }
ms_user_command() {
    printf '%s\n' "$*" >> "$tmp/bus"
    if [[ ${*:2} == 'systemctl --user show-environment' ]]; then
        case "$session" in
            x11) echo DISPLAY=:0;;
            wayland) echo WAYLAND_DISPLAY=wayland-0;;
            headless) echo LANG=C;;
            absent) return 1;;
        esac
    elif [[ $launch_fails == yes ]]; then return 1; fi
}
launch_fails=no
for session in x11 wayland headless absent; do
    rm -f "$tmp/bus"
    ms_resume_one 1000
    if [[ $session == x11 || $session == wayland ]]; then
        grep -q '^1000 systemd-run --user --collect .* -- /usr/bin/mineshare-app$' "$tmp/bus"
    else
        [[ $(wc -l < "$tmp/bus") == 1 ]]
    fi
done
session=wayland launch_fails=yes
if ms_resume_one 1000; then echo 'Restart failure was not detected'; exit 1; fi
ms_identity() { echo 1000:10; }
rm -f "$tmp/bus"
ms_resume_one 1000
[[ ! -e $tmp/bus ]] # Configure retry must not launch a second GUI.

mkdir "$tmp/state"; ln -s "$tmp/state" "$tmp/link"
if ms_state_directory "$tmp/link"; then echo 'State symlink accepted'; exit 1; fi
echo 'PASS: executable scope, graceful/forced stop, PID reuse, X11/Wayland restart, headless skip, duplicate/retry and state symlink checks'
