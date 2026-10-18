#!/bin/bash
# Real dpkg integration, restricted to disposable GitHub runners.
set -euo pipefail
[[ $EUID == 0 && ${GITHUB_ACTIONS:-} == true ]] || { echo 'Disposable GitHub runner/root required'; exit 1; }
[[ ! -e /usr/bin/mineshare-app ]]
! dpkg-query -W -f='${db:Status-Status}' mineshare-lifecycle-fixture 2>/dev/null | grep -qx installed
directory=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$directory/../.." && pwd)
tmp=$(mktemp -d /tmp/mineshare-dpkg-test.XXXXXX)
user=mineshare_fixture
! getent passwd "$user" >/dev/null
cleanup() {
    pkill -u "$user" -x mineshare-app 2>/dev/null || true
    dpkg --remove mineshare-lifecycle-fixture >/dev/null 2>&1 || true
    userdel "$user" 2>/dev/null || true
    [[ $tmp == /tmp/mineshare-dpkg-test.* ]] && rm -rf -- "$tmp"
}
trap cleanup EXIT
useradd --no-create-home --home-dir "$tmp/home" --shell /bin/sh "$user"
mkdir -p "$tmp/home/.config/MineShare" "$tmp/home/.config/autostart"
chmod 755 "$tmp"
printf 'pairing-preserved\n' > "$tmp/home/.config/MineShare/identity.json"
printf 'startup-preserved\n' > "$tmp/home/.config/autostart/MineShare.desktop"
chown -R "$user:$user" "$tmp/home"
sha256sum "$tmp/home/.config/MineShare/identity.json" "$tmp/home/.config/autostart/MineShare.desktop" > "$tmp/hashes"
for version in 1 2; do
    pkg="$tmp/package-$version"
    mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin"
    printf 'Package: mineshare-lifecycle-fixture\nVersion: %s\nArchitecture: amd64\nMaintainer: CI <ci@example.invalid>\nDescription: isolated MineShare update fixture\n' "$version" > "$pkg/DEBIAN/control"
    printf '#include <stdio.h>\n#include <unistd.h>\nint main(void){puts("fixture-%s");fflush(stdout);for(;;)pause();}\n' "$version" | cc -x c - -o "$pkg/usr/bin/mineshare-app"
    if [[ $version == 2 ]]; then
        cp "$root/ui/src-tauri/target/deb-hooks/preinst" "$pkg/DEBIAN/preinst"
        # Isolate GUI lifecycle from udev/group setup; real postinst still embeds this source.
        cp "$directory/deb-lifecycle.sh" "$pkg/DEBIAN/postinst"
        printf '\n[[ ${1:-} != configure ]] || ms_resume_upgrade\n' >> "$pkg/DEBIAN/postinst"
        chmod 755 "$pkg/DEBIAN/preinst" "$pkg/DEBIAN/postinst"
    fi
    dpkg-deb --build "$pkg" "$tmp/$version.deb" >/dev/null
done
dpkg -i "$tmp/1.deb"
runuser -u "$user" -- /usr/bin/mineshare-app > "$tmp/old.log" &
wrapper=$!
sleep 0.3
old=$(pgrep -u "$user" -x mineshare-app)
[[ -n $old ]]
dpkg -i "$tmp/2.deb"
wait "$wrapper" || true
[[ ! -e /proc/$old/exe ]]
grep -qx fixture-1 "$tmp/old.log"
! pgrep -u "$user" -x mineshare-app >/dev/null
[[ $(dpkg-query -W -f='${Version}' mineshare-lifecycle-fixture) == 2 ]]
sha256sum -c "$tmp/hashes"
# Same-version explicit reinstall is idempotent, and does not start a headless GUI.
dpkg -i "$tmp/2.deb"
! pgrep -u "$user" -x mineshare-app >/dev/null
runuser -u "$user" -- /usr/bin/mineshare-app > "$tmp/new.log" &
wrapper=$!
sleep 0.3
new=$(pgrep -u "$user" -x mineshare-app)
grep -qx fixture-2 "$tmp/new.log"
kill "$new"; wait "$wrapper" || true
[[ $(dpkg-query -W -f='${Version}' mineshare-lifecycle-fixture) == 2 ]]
echo 'PASS: real dpkg upgrade stops old inode, replaces the package once, preserves pairing/startup, and supports reinstall'
