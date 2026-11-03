#!/bin/bash
# Shared source embedded into each maintainer script: preinst cannot use files
# from the new package before unpack. Never source user files or run the GUI as root.
set -euo pipefail

ms_valid_exe() {
    [[ $1 == /usr/bin/mineshare-app || $1 == '/usr/bin/mineshare-app (deleted)' ]]
}

ms_identity() {
    local pid=$1 exe uid='' key real rest statline
    [[ $pid =~ ^[0-9]+$ ]] || return 1
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || return 1
    ms_valid_exe "$exe" || return 1
    while read -r key real rest; do
        if [[ $key == Uid: ]]; then uid=$real; break; fi
    done 2>/dev/null < "/proc/$pid/status" || return 1
    [[ $uid =~ ^[0-9]+$ && $uid != 0 ]] || return 1
    # Field 22 is process start time. Removing the final ') ' handles spaces in comm.
    IFS= read -r statline 2>/dev/null < "/proc/$pid/stat" || return 1
    rest=${statline##*) }
    local fields=()
    read -r -a fields <<< "$rest"
    [[ ${fields[19]:-} =~ ^[0-9]+$ ]] || return 1
    printf '%s:%s\n' "$uid" "${fields[19]}"
}

ms_same_process() {
    [[ $(ms_identity "$1") == "$2" ]]
}

ms_stop_one() {
    local pid=$1 identity=$2 i
    ms_same_process "$pid" "$identity" || return 0
    kill -TERM "$pid" 2>/dev/null || { ms_same_process "$pid" "$identity" && return 1; return 0; }
    for ((i=0; i<50; i++)); do
        ms_same_process "$pid" "$identity" || return 0
        sleep 0.1
    done
    # Recheck start time before escalating: never signal a recycled PID.
    ms_same_process "$pid" "$identity" || return 0
    kill -KILL "$pid" 2>/dev/null || true
    for ((i=0; i<20; i++)); do
        ms_same_process "$pid" "$identity" || return 0
        sleep 0.1
    done
    echo "MineShare could not stop PID $pid; update aborted before replacing files." >&2
    return 1
}

ms_state_directory() {
    local state=$1
    [[ ! -L $state ]] || { echo 'Unsafe MineShare upgrade state symlink' >&2; return 1; }
    if [[ ! -e $state ]]; then mkdir -m 700 -- "$state"; fi
    [[ -d $state && $(stat -c %u "$state") == 0 && $(stat -c %a "$state") == 700 ]]
}

ms_prepare_upgrade() {
    local state=/run/mineshare-upgrade pid identity uid mode vendor
    [[ $EUID == 0 ]] || return 1
    umask 077
    ms_state_directory "$state"
    if [[ -e $state/users || -L $state/users ]]; then
        [[ -f $state/users && ! -L $state/users && $(stat -c %u "$state/users") == 0 ]] || return 1
    fi
    for proc in /proc/[0-9]*; do
        pid=${proc##*/}
        identity=$(ms_identity "$pid") || continue
        uid=${identity%%:*}
        mode=plain
        vendor=$(ms_user_command "$uid" bash -c '
            [[ -f ${XDG_CONFIG_HOME:-$HOME/.config}/MineShare/keyboard-broker.enabled ]] || exit 0
            [[ -r ${XDG_DATA_HOME:-$HOME/.local/share}/MineShare/typeless-login.desktop ]] || exit 0
            for pid in $(pgrep -u "$(id -u)" -x typeless); do
                [[ $(readlink "/proc/$pid/exe") == /opt/Typeless/typeless ]] && printf "%s\n" "$pid"
            done
            exit 0
        ' 2>/dev/null) || vendor=
        if [[ -n $vendor ]]; then mode=ordered; fi
        # Record intent before stopping so a configure retry can resume it.
        printf '%s:%s\n' "$uid" "$mode" >> "$state/users"
        if [[ $mode == ordered ]]; then
            if ! ms_user_command "$uid" bash -c '
                # Run as the desktop user and use pidfds to avoid PID reuse.
                exec python3 - "$@" <<'"'"'PY'"'"'
import os, select, signal, sys
import shutil, subprocess
from pathlib import Path

if shutil.which("pactl"):
    audio = subprocess.run(["pactl", "list", "source-outputs"], capture_output=True, text=True)
    if audio.returncode == 0 and "typeless" in audio.stdout.lower():
        sys.exit("Typeless is recording. Finish the recording before upgrading MineShare.")

pids = []
for arg in sys.argv[1:]:
    pid = int(arg)
    try:
        fd = os.pidfd_open(pid)
        proc = Path("/proc") / str(pid)
        if proc.stat().st_uid == os.getuid() and os.readlink(proc / "exe") == "/opt/Typeless/typeless":
            pids.append(fd)
    except (FileNotFoundError, ProcessLookupError):
        pass
for fd in pids:
    try:
        signal.pidfd_send_signal(fd, signal.SIGTERM)
    except ProcessLookupError:
        pass
for fd in pids:
    if not select.select([fd], [], [], 3)[0]:
        sys.exit("Typeless did not close. MineShare upgrade paused before keyboard ownership changed.")
PY
            ' bash $vendor; then
                ms_resume_upgrade || true
                return 1
            fi
        fi
        echo "==> Stopping MineShare for user $uid before upgrade"
        ms_stop_one "$pid" "$identity"
    done
}

ms_user_command() {
    local uid=$1 username runtime=/run/user/$1 account
    [[ $uid =~ ^[0-9]+$ && $uid != 0 ]] || return 1
    shift
    [[ -d $runtime && ! -L $runtime && $(stat -c %u "$runtime") == "$uid" && -S $runtime/bus ]] || return 1
    account=$(getent passwd "$uid") || return 1
    username=${account%%:*}
    # Drop privileges before talking to the user's bus. No eval or saved shell commands.
    runuser -u "$username" -- env XDG_RUNTIME_DIR="$runtime" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" "$@"
}

ms_resume_one() {
    local uid=$1 mode=${2:-plain} proc identity environment
    for proc in /proc/[0-9]*; do
        identity=$(ms_identity "${proc##*/}") || continue
        if [[ ${identity%%:*} == "$uid" && $mode == plain ]]; then return 0; fi
    done
    environment=$(ms_user_command "$uid" systemctl --user show-environment 2>/dev/null) || {
        echo "==> MineShare not restarted for user $uid: no active user manager."
        return 0
    }
    if ! grep -Eq '^(DISPLAY|WAYLAND_DISPLAY)=.+' <<< "$environment"; then
        echo "==> MineShare not restarted for user $uid: no graphical session."
        return 0
    fi
    local unit="--unit=mineshare-package-resume-$$-$uid"
    if [[ $mode == ordered ]]; then
        # The coordinator exits after launching the apps. Keep its children alive.
        ms_user_command "$uid" systemd-run --user --wait --collect --property=Type=exec --property=KillMode=process \
            "$unit" -- /bin/bash /usr/share/mineshare/setup/installer/linux/start-typeless-after-mineshare.sh
    else
        ms_user_command "$uid" systemd-run --user --collect --property=Type=exec --property=PartOf=graphical-session.target \
            "$unit" -- /usr/bin/mineshare-app
    fi
}

ms_resume_upgrade() {
    local state=/run/mineshare-upgrade uid mode failed=0
    [[ -e $state/users ]] || return 0 # Fresh install: do not auto-open a GUI.
    ms_state_directory "$state"
    [[ -f $state/users && ! -L $state/users && $(stat -c %u "$state/users") == 0 ]] || return 1
    [[ -x /usr/bin/mineshare-app ]] || return 1
    while IFS=: read -r uid mode; do
        [[ $uid =~ ^[0-9]+$ && $uid != 0 ]] || { failed=1; continue; }
        if ! ms_resume_one "$uid" "${mode:-plain}"; then
            echo "MineShare updated, but automatic restart failed for user $uid. Open MineShare from the application menu." >&2
            failed=1
        fi
    done < <(sort -t: -k1,1 -k2,2r "$state/users" | awk -F: '!seen[$1]++')
    if [[ $failed == 0 ]]; then rm -- "$state/users"; rmdir -- "$state"; fi
    # A desktop restart failure must not break package configuration. Retain state for retry.
    return 0
}
