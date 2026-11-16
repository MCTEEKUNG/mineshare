#!/usr/bin/env bash
set -euo pipefail
[[ $EUID != 0 ]] || { echo 'Run as the graphical user, without sudo.' >&2; exit 1; }
root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
version=$(gnome-shell --version | sed -nE 's/.* ([0-9]+).*/\1/p')
[[ $version =~ ^(45|46|47|48|49|50)$ ]] || { echo 'Unsupported GNOME version' >&2; exit 1; }
uuid=mineshare-pointer@mineshare.local
data=${XDG_DATA_HOME:-$HOME/.local/share}
dest="$data/gnome-shell/extensions/$uuid"
if [[ -e $dest ]]; then
    backup="$data/MineShare/backups/pointer-extension-$(date +%Y%m%d-%H%M%S)-$$"
    mkdir -p "$backup"
    cp -a -- "$dest" "$backup/extension"
fi
mkdir -p "$dest"
install -m 644 "$root/gnome-pointer/metadata.json" "$root/gnome-pointer/extension.js" "$dest/"
if ! gnome-extensions enable "$uuid"; then
    echo 'Extension installed. Log out/in, then run: gnome-extensions enable mineshare-pointer@mineshare.local'
else
    echo 'MineShare read-only pointer extension enabled.'
fi
