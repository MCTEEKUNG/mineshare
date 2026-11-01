#!/usr/bin/env bash
# Run before GNOME reads autostart entries. Typeless may remove/rewrite its
# user override while running, exposing the unordered system-wide entry.
set -euo pipefail
config=${XDG_CONFIG_HOME:-$HOME/.config}
data=${XDG_DATA_HOME:-$HOME/.local/share}
[[ -f "$config/MineShare/keyboard-broker.enabled" ]] || exit 0
canonical="$data/MineShare/typeless-ordered.desktop"
[[ -r "$canonical" ]] || { echo 'Missing MineShare ordered startup entry.' >&2; exit 1; }
grep -q mineshare-typeless-start "$canonical" || { echo 'Invalid MineShare ordered startup entry.' >&2; exit 1; }
mkdir -p "$config/autostart"
temporary=''
trap '[[ -z "$temporary" ]] || rm -f -- "$temporary"' EXIT
for name in MineShare.desktop typeless.desktop; do
    destination="$config/autostart/$name"
    if [[ -f "$destination" ]] && cmp -s -- "$canonical" "$destination"; then
        continue
    fi
    temporary=$(mktemp "$config/autostart/.mineshare-order.XXXXXX")
    install -m 644 -- "$canonical" "$temporary"
    mv -f -- "$temporary" "$destination"
    temporary=''
done
