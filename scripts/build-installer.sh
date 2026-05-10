#!/bin/bash
# Build all distributable installers for the host platform.
#
# Run on:
#   * Linux (Ubuntu / Debian) → produces .deb + .AppImage + .rpm
#   * macOS                   → produces .app + .dmg (untested)
#   * Windows                 → use scripts/build-installer.ps1
#                               (this script needs a real shell;
#                               WSL works but won't bundle .msi)
#
# Tauri's bundler can't cross-compile (each OS's WebView is a
# native lib), so producing both Linux + Win artifacts means
# running on each OS — locally, or via the GitHub Actions
# workflow at .github/workflows/release.yml.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/ui"

# Frozen lockfile so `bun install` doesn't silently bump deps
# on a release build. Use plain `bun install` for first-time
# checkouts that don't have a lock yet.
if [ -f bun.lock ]; then
    bun install --frozen-lockfile
else
    bun install
fi

echo
echo "==> tauri build"
bun run tauri build

echo
echo "==> Installer artifacts:"
ARTIFACTS_DIR="src-tauri/target/release/bundle"
if [ -d "$ARTIFACTS_DIR" ]; then
    cd "$ARTIFACTS_DIR"
    find . -type f \( \
        -name '*.deb' -o \
        -name '*.AppImage' -o \
        -name '*.rpm' -o \
        -name '*.msi' -o \
        -name '*-setup.exe' -o \
        -name '*.dmg' \
    \) -exec ls -lh {} \;
else
    echo "  (no bundle dir found — check tauri build output above)"
fi
