# Ubuntu readiness

## Platform boundaries

- The desktop app supports Windows and Linux builds; CI checks Windows, Ubuntu 22.04, and Ubuntu 24.04, including the Tauri backend and frontend tests.
- Linux captures physical devices with evdev and injects mouse/keyboard events through uinput. The Windows touch-suppressed-cursor recovery is Windows-only; it must not introduce Windows APIs into Linux builds.
- Windows native Precision Touchpad contact injection is not implemented on Linux. Linux peers retain basic pointer, button, and scroll support; do not assume Windows multi-finger gestures are available there.
- Native Windows clipboard revisions are Windows-only. Linux keeps the existing polling fallback.
- Game Lock is manual (`Ctrl+Shift+L`); other hotkeys retain Alt. Linux and Windows peers must use compatible protocol builds (currently control-v2).

## Before installing on Ubuntu

Do not reboot the currently running Windows desktop just to prepare a Linux build. When Ubuntu is running, verify the installed build identity and SHA-256, desktop session (Wayland or X11), input-group membership, and writable `/dev/uinput` before physical acceptance.

Build dependencies for Ubuntu 22.04/24.04:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config cmake file \
  libssl-dev libudev-dev libasound2-dev libopus-dev \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libpipewire-0.3-dev pulseaudio-utils
```

Use the repository's Linux installer or packaged deb to configure input access. Log out/in after changing group membership. Run the app in the logged-in graphical user's session, not as a root GUI or a duplicate headless daemon alongside the desktop app.

## Acceptance when Ubuntu is available

1. Pair with the Windows laptop; confirm negotiated versions and reconnect after logout/restart.
2. Move the PC's physical mouse to Windows immediately after laptop boot, and again after touching its screen. The cursor must appear without using the laptop touchpad.
3. Cross both directions, type on the focused peer, use Alt+Tab and the destination screenshot shortcut, scroll, and drag with no duplicated local input or stuck modifiers.
4. Replace a laptop clipboard copy with a new Ubuntu copy, paste locally, then paste across devices. Check text, screenshots, and files separately.
5. Check audio playback, output-device changes, and microphone routing.
6. Use Ctrl+Shift+L on the focused desktop; confirm other shortcuts and edge crossing recover after unlocking.

Automated Linux builds/tests cannot certify permissions, compositor behavior, physical input, audio devices, or boot behavior. Those remain pending until Ubuntu is running; no Ubuntu installation is performed while the desktop is booted into Windows.
