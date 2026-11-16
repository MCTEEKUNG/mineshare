#!/bin/bash
# Fixed root-owned package helper. pkexec supplies the authenticating user's UID;
# never accept a username, command, or path from the webview/environment.
set -euo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
[[ $EUID == 0 && ${PKEXEC_UID:-} =~ ^[1-9][0-9]*$ ]] || { echo 'Authenticate through the MineShare setup button.' >&2; exit 1; }
account=$(getent passwd "$PKEXEC_UID")
user=${account%%:*}
[[ -n $user && $user != root ]]
getent group input >/dev/null || groupadd --system input
usermod -aG input -- "$user"
if [[ ! -c /dev/uinput ]]; then modprobe uinput; fi
[[ -c /dev/uinput ]] || { echo 'Kernel uinput device is unavailable.' >&2; exit 1; }
install -m 644 /usr/share/mineshare/setup/installer/linux/60-mineshare.rules /etc/udev/rules.d/60-mineshare.rules
udevadm control --reload-rules
udevadm trigger --subsystem-match=misc --attr-match=name=uinput
echo 'Input permissions configured. Log out and back in to apply group membership.'
