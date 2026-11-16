#!/bin/bash
set -euo pipefail
package=${1:?Supply the built .deb or .rpm}
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
case "$package" in
    *.deb)
        dpkg-deb --extract "$package" "$work"
        dpkg-deb -f "$package" Depends | grep -q 'pkexec | policykit-1'
        ;;
    *.rpm)
        bsdtar -xf "$package" -C "$work"
        ;;
    *) echo 'Expected a .deb or .rpm' >&2; exit 1 ;;
esac
root="$work/usr/share/mineshare/setup/installer/linux"
for file in install-typeless-compat.sh start-typeless-after-mineshare.sh prepare-typeless-autostart.sh install-gnome-pointer.sh enable-input.sh; do
    cmp "installer/linux/$file" "$root/$file"
    bash -n "$root/$file"
done
for file in metadata.json extension.js; do cmp "installer/linux/gnome-pointer/$file" "$root/gnome-pointer/$file"; done
cmp installer/linux/60-mineshare.rules "$root/60-mineshare.rules"
# Invalid/missing authenticating UID cannot reach usermod, even from root.
if env -u PKEXEC_UID bash "$root/enable-input.sh"; then echo 'Root helper accepted missing UID' >&2; exit 1; fi
if PKEXEC_UID=0 bash "$root/enable-input.sh"; then echo 'Root helper accepted root UID' >&2; exit 1; fi
echo 'PASS: exact broker/GNOME/root-helper payload present; invalid authentication rejected'
