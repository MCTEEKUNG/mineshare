#!/bin/bash
# A disposable runner only: never replace an installed app on a user's system.
set -euo pipefail
[[ ${GITHUB_ACTIONS:-} == true && $EUID != 0 ]] || { echo 'Disposable CI runner required'; exit 1; }
root=$(cd "$(dirname "$0")/../.." && pwd)
dest=/usr/share/mineshare/setup
[[ ! -e $dest && ! -e /usr/bin/mineshare-app && ! -e /opt/Typeless/typeless ]]
work=$(mktemp -d)
cleanup() {
    sudo rm -f -- /usr/bin/mineshare-app /opt/Typeless/typeless
    sudo rmdir /opt/Typeless 2>/dev/null || true
    sudo rm -rf -- /usr/share/mineshare/setup
    rm -rf -- "$work"
}
trap cleanup EXIT
sudo mkdir -p "$dest/installer/linux" /opt/Typeless
sudo cp "$root/installer/linux/"{install-typeless-compat.sh,start-typeless-after-mineshare.sh,install-gnome-pointer.sh,enable-input.sh} "$dest/installer/linux/"
sudo cp -r "$root/installer/linux/gnome-pointer" "$dest/installer/linux/"
sudo ln -s /usr/bin/true /usr/bin/mineshare-app
sudo ln -s /usr/bin/true /opt/Typeless/typeless
mkdir -p "$work/home/.config/autostart" "$work/bin"
printf '[Desktop Entry]\nType=Application\nName=Typeless\nExec=/opt/Typeless/typeless --preserve-login-flags\n' > "$work/home/.config/autostart/typeless.desktop"
printf '#!/bin/sh\necho "GNOME Shell 50.1"\n' > "$work/bin/gnome-shell"
printf '#!/bin/sh\necho "fixture input"\n' > "$work/bin/id"
printf '#!/bin/sh\nexit 1\n' > "$work/bin/gnome-extensions"
printf '#!/bin/sh\nexit 1\n' > "$work/bin/gdbus"
cat > "$work/bin/gsettings" <<'EOF'
#!/bin/bash
set -eu
file="$MINESHARE_SETUP_WORK/$3"
if [[ $1 == get ]]; then
    if [[ $3 == disable-user-extensions ]]; then echo false
    elif [[ -f $file ]]; then cat "$file"
    else echo "['other@extension']"; fi
elif [[ $1 == set ]]; then printf '%s\n' "$4" > "$file"
else exit 1; fi
EOF
chmod +x "$work/bin/"*
# cargo/rustup must retain their real HOME; the ignored test passes fixture HOME
# to its own setup invocation, after the test executable has already started.
export MINESHARE_SETUP_WORK="$work" MINESHARE_SETUP_FIXTURE=1
export PATH="$work/bin:$PATH"
cd "$root/ui/src-tauri"
cargo test --lib linux_setup::linux::tests::fixture_configuration --locked -- --ignored
