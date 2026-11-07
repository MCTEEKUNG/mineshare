# Touchpad sharing with Linux

## Three-finger window switching

With the pointer on a Linux peer, swipe horizontally with three fingers.
Swiping right advances the peer's Alt+Tab switcher and swiping left moves
backward. Keep the fingers down to step through the list and lift to select.
The first step starts at about 4.6 mm on the virtual pad. Subsequent steps
are about 6.4 mm apart. Reversing direction takes about 3 mm from the
furthest point reached. Version 5 of the MineShare GNOME extension shows the
switcher as soon as it opens and stops the gesture at either end of the list.
Alt+Shift remains available to switch keyboard languages.
Run Linux Setup and sign in again to load an updated extension.
The receiver releases the gesture's Alt modifier on handback, disconnect, or
inactivity if contact updates stop arriving. The Linux desktop must have a
window switcher bound to Alt+Tab. On desktops without the GNOME helper,
backward steps use the desktop's Alt+Shift+Tab binding.

Linux can also send contacts from a supported type-B multitouch pad to a
Windows or Linux peer. Windows uses its configured Precision Touchpad
gestures for the received contacts. Set its three-finger swipes to
**Switch apps and show desktop** for app switching. Other received gestures
such as two-finger scrolling and vertical swipes continue through the
destination's native touchpad handling.

Linux capture uses evdev and injection uses uinput. Ubuntu/Debian and
Fedora/Rocky use the same input code, with their respective DEB or RPM
packages and the MineShare input-permission setup. Window switching needs a
graphical session. Edge handoff also depends on the desktop's pointer
integration. Use **Ctrl+Alt+R** for manual handoff if the desktop cannot report
its screen edge. When the pointer is back on the source desktop, its own
touchpad settings apply.

On Linux, a pad that reports individual contacts can use native forwarding to
a frame-capable peer. Older peers without frame injection can still receive
basic pointer and scroll events. Linux sends its native contact geometry
unchanged and applies the MineShare scroll-speed setting to wheel fallback.

For physical verification, try Windows → Linux, Linux → Windows and Linux →
Linux. On each receiver, open three windows, cycle forward and backward while
keeping three fingers down, and lift to select. Repeat while crossing back or
disconnecting, then verify Alt is released and two-finger scrolling still works.

## Previous Windows-to-Ubuntu validation

Verified on Tee (GNOME Wayland) and the Windows laptop on 2026-10-27, source
`f3606132c7a5`. The user physically confirmed two-finger scrolling and a
three-finger upward swipe work on Ubuntu after installation on both peers.

## Failure and repair

Linux previously advertised no touchpad injection capability and used the default
no-op `InputInject::touchpad`. Windows therefore selected basic wheel fallback.
Linux also rounded every fractional wheel delta separately: eight 0.125 steps
became zero instead of one. That failure is reproduced by the red regression
in `artifacts/linux-touchpad-20261027/red.log`.

Linux now creates a separate virtual touchpad and advertises FRAME_INJECT only
after creation succeeds. Contact snapshots become type-B multitouch slots, with
tracking IDs, finger counts and axis resolution. The OS handles scrolling,
pinching and supported desktop gestures. Windows enables its global contact
controller for frame-capable peers even when Windows-specific action shortcuts
are unsupported; those action events are not sent to Linux. Basic wheel fallback
retains fractional remainders independently for each axis.

Reference: [Linux multitouch protocol](https://docs.kernel.org/input/multi-touch-protocol.html).

## Invariants and acceptance

- Keep the virtual touchpad separate from the keyboard and mouse. Exclude it
  from local capture to prevent feedback. Do not claim Linux ACTION_INJECT:
  Windows-specific three/four-finger tap/hold actions are not mapped here.
- Publish availability only when uinput creation succeeds; creation failure
  keeps basic wheel fallback. Do not enable capture without a receiver.
- Each frame is an authoritative contact set. Missing contacts release their
  slots; a new stream replaces old contacts. Ignore a stale terminal belonging
  to another stream. Reject duplicate IDs and malformed counts before mutation.
- Release all contacts on handoff/disconnect, including when no keyboard keys
  or mouse buttons are held. Reset fractional wheel residue at session cleanup.
- The three-finger horizontal app switcher is handled by MineShare on Linux.
  Other native gestures depend on the destination desktop environment.
- Unit tests cover fractional scroll, invalid frames, missing contacts, stream
  replacement and stale terminal events. Windows input tests: 94 passed,
  12 environment-dependent ignored. Linux input tests: 51 passed, 3 ignored;
  2 X11 integration tests ignored. Linux daemon/network tests also passed.
- Native read-only probe on Tee confirmed `ID_INPUT_TOUCHPAD=1`, libinput gesture
  capability and two-finger scroll support. Windows session negotiated native
  frames with peer capabilities `0x4`; both peers reported the same source ID.
- Physical acceptance confirmed scroll and three-finger swipe. Pinch, other
  desktop gestures, Windows-to-Windows physical behavior, and Xorg have not been
  re-certified by this change. Preserve existing Windows action behavior.

Release, installation and probe evidence: `artifacts/linux-touchpad-20261027/`.
