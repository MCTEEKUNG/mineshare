# Windows touchpad to Ubuntu

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
- Do not replace contact gestures with Windows shortcut chords on Ubuntu.
  Available gestures/actions depend on the destination desktop environment.
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
