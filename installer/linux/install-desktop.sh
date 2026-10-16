#!/usr/bin/env bash
# Per-user graphical launcher/autostart. Run as the desktop user, never sudo.
set -euo pipefail
if [[ $EUID -eq 0 ]]; then echo 'Run as the graphical user, without sudo.' >&2; exit 1; fi
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
binary=$(realpath -e "${1:-$root/ui/src-tauri/target/release/mineshare-app}")
icons=$(realpath -e "${2:-$root/ui/src-tauri/icons}")
[[ -x "$binary" && -f "$icons/icon.png" && -f "$icons/128x128.png" && -f "$icons/32x32.png" ]]
data=${XDG_DATA_HOME:-$HOME/.local/share}
config=${XDG_CONFIG_HOME:-$HOME/.config}
launcher="$HOME/.local/bin/mineshare-app"
[[ ! -d "$launcher" ]]
mkdir -p "${launcher%/*}" "$data/applications" "$config/autostart"
backup="$data/MineShare/backups/desktop-$(date +%Y%m%d-%H%M%S)-$$"
save() {
    if [[ -e "$1" || -L "$1" ]]; then
        mkdir -p "$backup"
        cp -a -- "$1" "$backup/$2"
    fi
}
save "$launcher" launcher
ln -sfn -- "$binary" "$launcher"
for size in 32 128 512; do
    case $size in 512) source=icon.png;; *) source=${size}x${size}.png;; esac
    dest="$data/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dest"
    save "$dest/mineshare-app.png" "icon-$size.png"
    install -m 644 "$icons/$source" "$dest/mineshare-app.png"
done
# Desktop Exec has its own quoting rules, separate from shell quoting.
exec_path=${launcher//\\/\\\\}
exec_path=${exec_path//\"/\\\"}
exec_path=${exec_path//\$/\\$}
exec_path=${exec_path//\`/\\\`}
exec_path=${exec_path//%/%%}
entry="$data/applications/MineShare.desktop"
save "$entry" application.desktop
cat > "$entry" <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=MineShare
Comment=Cross-PC keyboard, mouse, audio, and file bridge
Exec="$exec_path"
Icon=mineshare-app
Terminal=false
Categories=Utility;
StartupWMClass=mineshare-app
StartupNotify=false
EOF
save "$config/autostart/MineShare.desktop" autostart.desktop
cp -- "$entry" "$config/autostart/MineShare.desktop"
printf '\nX-GNOME-Autostart-enabled=true\n' >> "$config/autostart/MineShare.desktop"
desktop_dir=$(xdg-user-dir DESKTOP 2>/dev/null || true)
if [[ -n "$desktop_dir" && -f "$desktop_dir/MineShare.desktop" ]]; then
    save "$desktop_dir/MineShare.desktop" desktop-shortcut.desktop
    install -m 755 "$entry" "$desktop_dir/MineShare.desktop"
fi
# The GUI embeds the daemon. Never autostart the old headless unit as well.
if command -v systemctl >/dev/null && systemctl --user is-enabled --quiet mineshare-daemon.service 2>/dev/null; then
    systemctl --user disable --now mineshare-daemon.service
fi
if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$entry" "$config/autostart/MineShare.desktop"
fi
if command -v update-desktop-database >/dev/null; then update-desktop-database "$data/applications"; fi
if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -f -t "$data/icons/hicolor"; fi
echo "Launcher: $entry"
echo "Autostart: $config/autostart/MineShare.desktop"
echo "Executable: $(readlink "$launcher")"
echo "Backups (if needed): $backup"
