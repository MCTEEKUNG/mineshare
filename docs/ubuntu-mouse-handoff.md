# Ubuntu mouse handoff after an in-session upgrade

On 2026-10-21, the physical mouse connected to Tee could not cross to the Windows
laptop. The laptop's physical mouse could cross to Tee. Both peers were connected
and used the same build. Layout was correct: laptop peer on Tee's left, Tee peer
on the laptop's right. The GNOME pointer extension returned live coordinates.

The capture failure occurred before edge detection. Typeless's keyboard helper
held `/dev/input/event10` (the Logitech G502 X Keyboard interface). Preparing the
keyboard broker failed with `Device or resource busy`. `EvdevCapture::start`
returned immediately and dropped every prepared device, including the independent
Logitech mouse at `/dev/input/event7`. No capture pumps ran, while uinput injection
remained available. This explains the asymmetry.

Startup now prepares keyboard ownership as a complete group while preserving
independent pointer devices. If a keyboard cannot be acquired, every partial
keyboard grab/proxy is released, broker readiness is not published, and mouse
capture still starts. A failure with no usable pointer still reports capture
unavailable. Disabling the broker retains normal capture routes. No third-party
process is stopped and no permission/startup preference is changed.

## Evidence

The read-only native probe `check-capture.py` reported zero mouse devices and zero
pumps before the fix. Regression tests reproduced the keyboard-conflict failure
and the pointer-only failure before the change. Four startup tests now cover
those cases, resource cleanup, full broker readiness, disabled broker behavior,
and the no-usable-device error.

Linux verification in the prepared Docker builder: 47 input tests passed (three
hardware/private-bus tests skipped), 62 daemon tests and 9 network tests passed;
input Clippy passed with warnings denied and the workspace check passed. Windows
workspace check and formatting passed. Both native packages built with locked
dependencies and passed package audits.

Both computers were updated to source `0e1c7dd7a4c3`, `control-v3`. Installed
executables match their package payloads; identity, settings, layout, trust and
startup hashes were retained. The running Tee process now holds the physical
mouse and has one capture pump, while the same Typeless process remains running.

Real physical handoff is confirmed by the user. Tee logged left-edge handoffs at
20:38:27, 20:38:35, 20:38:51 and 20:39:35 Asia/Bangkok; the laptop logged matching
remote-control acquisition and release. No synthetic input was used as a
substitute for that test. Evidence and rollback artifacts are under
`artifacts/ubuntu-mouse-handoff/`.

Keyboard compatibility was subsequently recovered in-session through a
user-authorized ordered restart; see [ubuntu-keyboard-recovery.md](ubuntu-keyboard-recovery.md).
The user subsequently confirmed successful reconnection after physically interrupting the network on 2026-10-21.
