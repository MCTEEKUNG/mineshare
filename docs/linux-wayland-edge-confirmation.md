# Accurate edge crossing on GNOME Wayland

Raw evdev deltas are not desktop pixels. Acceleration, fractional scaling and
returning from the peer can put the accumulated estimate at zero while the real
cursor is hundreds of pixels inside the screen. The former X11 fix did not cover
Wayland: `Unsupported` explicitly allowed the estimate-only fallback.

The candidate detector still avoids queries on ordinary interior motion. Before
adding any edge pressure it now requires authoritative coordinates. Missing,
failed or unsupported position reads clear pressure and cannot cross. X11 uses
QueryPointer; GNOME Wayland uses the opt-in read-only extension below. The remote
motion path does not call D-Bus. Failure is retried after one second, never using
a cached successful position as proof. Each method call has a 12 ms timeout.

The extension reports `global.get_pointer()` and logical desktop dimensions, not
XWayland coordinates or physical pixel dimensions ([GNOME pointer API](https://gnome.pages.gitlab.gnome.org/gnome-shell/shell/method.Global.get_pointer.html)). It exposes only `GetPointer`
on the existing user-session `org.gnome.Shell` connection. It cannot inject input,
run commands, read clipboard/window contents or change focus. Locked sessions
return invalid dimensions. Disabling the extension unexports the object.

With explicit user approval, run `installer/linux/install-gnome-pointer.sh` as
the graphical user. It supports GNOME 45–50 and backs up its own existing extension
files. A newly installed extension may require logout/login before GNOME discovers
it; then enable `mineshare-pointer@mineshare.local` as printed by the installer.
Do not enable unsafe GNOME Shell Eval. Other compositors remain fail-closed;
Ctrl+Alt+R remains the manual control handoff.

Tests cover drift far inside the screen, all edge directions, logical scaling,
missing position, read-only extension replies/lock guard and disable cleanup.
Native GNOME activation, query latency and physical crossing acceptance still
require an approved desktop installation; mocked tests are not that evidence.

For Typeless coexistence, also apply the separate opt-in keyboard broker setup.
The two fixes solve independent problems and must not silently change OS settings
or stop Typeless during installation.
