#!/usr/bin/env bash
# Per-user opt-in. Does not stop apps, log out, or edit vendor files.
set -euo pipefail
[[ $EUID -ne 0 ]] || { echo 'Run without sudo.' >&2; exit 1; }
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
config=${XDG_CONFIG_HOME:-$HOME/.config}
data=${XDG_DATA_HOME:-$HOME/.local/share}
[[ -x "$HOME/.local/bin/mineshare-app" && -x /opt/Typeless/typeless ]]
[[ -f "$config/autostart/MineShare.desktop" ]]
backup="$data/MineShare/backups/typeless-compat-$(date +%Y%m%d-%H%M%S)-$$"
mkdir -p "$backup" "$config/MineShare" "$config/autostart" "$HOME/.local/bin"
mkdir -p "$data/MineShare"
# Preserve the original vendor command, not just the executable name. Idempotent:
# never replace it with our own coordinator on a second install.
original="$data/MineShare/typeless-login.desktop"
if [[ ! -f "$original" ]]; then
    if [[ -f "$config/autostart/typeless.desktop" ]] && ! grep -q mineshare-typeless-start "$config/autostart/typeless.desktop"; then
        source="$config/autostart/typeless.desktop"
    elif [[ -f /etc/xdg/autostart/typeless.desktop ]]; then
        source=/etc/xdg/autostart/typeless.desktop
    else
        source=/usr/share/applications/typeless.desktop
    fi
    install -m 644 "$source" "$original"
fi
for name in MineShare.desktop typeless.desktop; do
    if [[ -e "$config/autostart/$name" ]]; then
        cp -a -- "$config/autostart/$name" "$backup/$name"
    fi
done
install -m 755 "$root/installer/linux/start-typeless-after-mineshare.sh" "$HOME/.local/bin/mineshare-typeless-start"
printf 'enabled\n' > "$config/MineShare/keyboard-broker.enabled"
exec_path="$HOME/.local/bin/mineshare-typeless-start"
exec_path=${exec_path//\\/\\\\}
exec_path=${exec_path//\"/\\\"}
exec_path=${exec_path//\$/\\$}
exec_path=${exec_path//\`/\\\`}
exec_path=${exec_path//%/%%}
# Both desktop entries use the same locked coordinator: alphabetical ordering
# and GNOME's parallel launch order no longer matter.
for name in MineShare.desktop typeless.desktop; do
cat > "$config/autostart/$name" <<EOF
[Desktop Entry]
Type=Application
Name=MineShare + Typeless ordered startup
Exec="$exec_path"
Terminal=false
X-GNOME-Autostart-enabled=true
StartupNotify=false
EOF
done
if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$config/autostart/MineShare.desktop" "$config/autostart/typeless.desktop"
fi
printf 'Installed keyboard broker + ordered startup. Log out/in once when ready.\nBackups: %s\n' "$backup"
