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
    local state=/run/mineshare-upgrade pid identity uid
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
        # Record intent before stopping so a configure retry can resume it.
        printf '%s\n' "$uid" >> "$state/users"
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
    local uid=$1 proc identity environment
    for proc in /proc/[0-9]*; do
        identity=$(ms_identity "${proc##*/}") || continue
        [[ ${identity%%:*} == "$uid" ]] && return 0
    done
    environment=$(ms_user_command "$uid" systemctl --user show-environment 2>/dev/null) || {
        echo "==> MineShare not restarted for user $uid: no active user manager."
        return 0
    }
    if ! grep -Eq '^(DISPLAY|WAYLAND_DISPLAY)=.+' <<< "$environment"; then
        echo "==> MineShare not restarted for user $uid: no graphical session."
        return 0
    fi
    ms_user_command "$uid" systemd-run --user --collect --property=Type=exec \
        "--unit=mineshare-package-resume-$$-$uid" -- /usr/bin/mineshare-app
}

ms_resume_upgrade() {
    local state=/run/mineshare-upgrade uid failed=0
    [[ -e $state/users ]] || return 0 # Fresh install: do not auto-open a GUI.
    ms_state_directory "$state"
    [[ -f $state/users && ! -L $state/users && $(stat -c %u "$state/users") == 0 ]] || return 1
    [[ -x /usr/bin/mineshare-app ]] || return 1
    while IFS= read -r uid; do
        [[ $uid =~ ^[0-9]+$ && $uid != 0 ]] || { failed=1; continue; }
        if ! ms_resume_one "$uid"; then
            echo "MineShare updated, but automatic restart failed for user $uid. Open MineShare from the application menu." >&2
            failed=1
        fi
    done < <(sort -u "$state/users")
    if [[ $failed == 0 ]]; then rm -- "$state/users"; rmdir -- "$state"; fi
    # A desktop restart failure must not break package configuration. Retain state for retry.
    return 0
}
