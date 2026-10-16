# Ubuntu readiness

## Platform boundaries

- The desktop app supports Windows and Linux builds; CI checks Windows, Ubuntu 22.04, and Ubuntu 24.04, including the Tauri backend and frontend tests.
- Linux captures physical devices with evdev and injects mouse/keyboard events through uinput. The Windows touch-suppressed-cursor recovery is Windows-only; it must not introduce Windows APIs into Linux builds.
- Windows native Precision Touchpad contact injection is not implemented on Linux. Linux peers retain basic pointer, button, and scroll support; do not assume Windows multi-finger gestures are available there.
- Native Windows clipboard revisions are Windows-only. Linux keeps the existing polling fallback.
- On X11, automatic edge crossing confirms that the desktop pointer is actually at the configured boundary before accumulating edge pressure. Raw evdev deltas alone are not screen coordinates. The query is restricted to candidate edge reports; normal remote motion is unchanged. A failed X11 query pauses automatic crossing (Ctrl+Alt+R remains available). Wayland retains its existing estimated-position fallback; XWayland coordinates are not used as global Wayland pointer coordinates.
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
7. Switch English/Thai with the Ubuntu physical keyboard and verify the typed
   characters, not only the input-source indicator. Repeat after crossing back
   from the Windows peer.

Automated Linux builds/tests cannot certify permissions, compositor behavior, physical input, audio devices, or boot behavior. Those remain pending until Ubuntu is running; no Ubuntu installation is performed while the desktop is booted into Windows.

## GNOME: Alt+Shift indicates Thai but typing remains English

On the tested Ubuntu X11 desktop, the physical keyboard was routed through the
MineShare local keyboard broker with Typeless running. GNOME had English/Thai
input sources, but `Alt+Shift` was configured as the low-level XKB option
`grp:alt_shift_toggle`, while GNOME's input-source shortcut remained `Super+Space`.
During the failure the real XKB keymap contained only English. Choosing a source
and switching the actual keymap must be verified separately from its indicator.

The verified configuration uses GNOME's input-source manager for `Alt+Shift`
instead of the overlapping XKB group toggle. This is an **opt-in desktop setting**,
not a MineShare app update; the application and installer do not overwrite the
user's language preferences automatically.

Run this only in the affected user's GNOME graphical session, **without sudo**.
It prints the previous values for rollback, retains existing GNOME shortcuts and
unrelated XKB options, and changes only the conflicting Alt+Shift configuration:

```sh
python3 - <<'PY'
from gi.repository import Gio

wm = Gio.Settings.new('org.gnome.desktop.wm.keybindings')
sources = Gio.Settings.new('org.gnome.desktop.input-sources')
keys = list(wm.get_strv('switch-input-source'))
options = list(sources.get_strv('xkb-options'))
print('Previous switch-input-source:', keys)
print('Previous xkb-options:', options)
new_keys = list(dict.fromkeys(keys + ['<Alt>Shift_L', '<Shift>Alt_L']))
new_options = [option for option in options if option != 'grp:alt_shift_toggle']
try:
    assert wm.set_strv('switch-input-source', new_keys)
    assert sources.set_strv('xkb-options', new_options)
    Gio.Settings.sync()
except Exception:
    wm.set_strv('switch-input-source', keys)
    sources.set_strv('xkb-options', options)
    Gio.Settings.sync()
    raise
PY
```

To undo, restore the two printed arrays with `gsettings set` for their respective
schema/key. Do not use a blanket reset of all keyboard settings.

Validation on this desktop: synthetic `Alt+Shift` and `Super+Space` each changed
the active XKB group English → Thai → English; the A-position keysym changed
between `a` and `Thai_fofan`. The user then confirmed the physical-keyboard test
passed, with MineShare and Typeless left running. This does not certify all
Wayland sessions or other input methods, and requires no new installer/version.
