# Ubuntu keyboard broker (opt-in)

Typeless's Linux keyboard monitor exclusively grabs physical evdev keyboards.
MineShare's old conditional grab loses with EBUSY when both are running. A
protocol/scancode change cannot recover input the source never receives.

`installer/linux/install-typeless-compat.sh` enables the broker for the graphical
user and installs a per-user, readiness-gated autostart coordinator. Install the
app first (a `.deb` or the standalone desktop launcher). No vendor files, Typeless shortcuts, or Windows
settings are modified. The original Typeless desktop command and its login flags
are preserved and launched through GIO. Log out/in once after installation; do not launch Typeless
first. Its normal Launch at login toggle must not overwrite the coordinator.

On a `.deb` installation the setup now selects `/usr/bin/mineshare-app` and can
create the coordinator without any pre-existing MineShare autostart entry. The
older standalone `~/.local/bin/mineshare-app` is only a fallback. An explicit
executable path can be passed as the first argument (no shell command is stored).
After reinstalling Ubuntu, these **per-user** broker/startup files must be set up
again; installing the app package alone intentionally does not opt users in.

The broker acquires all physical keyboard devices before advertising readiness.
It keeps them grabbed in LOCAL and REMOTE, and creates a separate local uinput
proxy for each device. Typeless can monitor the proxies while MineShare exclusively
owns hardware. Local events use the proxy, remote events use the existing encrypted
input transport, never both. Right Alt / Super retain their original codes.
Local repeats/releases retain their press owner across a cursor crossing. Consumed
MineShare hotkeys do not leak orphan key-up events to the proxy. Virtual sources
(including Typeless re-emission devices) are excluded to prevent feedback.

The coordinator launches Typeless only when the broker's PID is live and readiness
has been published after acquisition; two autostart entries are serialized by
flock. A conflict times out visibly rather than silently launching two competing
exclusive readers. It does not terminate or suspend an already-running Typeless.

On GNOME/systemd, setup also installs the per-user drop-in
`app-typeless@autostart.service.d/50-mineshare-order.conf`. Its negated
`ConditionPathExists` skips the independent Typeless login unit while the broker
opt-in marker exists. Typeless can remove its own user desktop override, exposing
the system-wide desktop entry again; relying on that override alone caused the
2026-10-27 regression. The separate unit guard survives that removal. It does not
block manual Typeless launches or change vendor files. Validate the condition
without quotes around the path: this systemd condition treats quotes literally.

GNOME 50 can instead launch desktop entries directly in `app-gnome-*.scope`.
The `app-typeless@autostart.service` guard does not protect that path. On
2026-11-01, login logs showed Typeless starting directly before MineShare after
its user override disappeared. Setup now also installs
`gnome-session-manager@.service.d/50-mineshare-order.conf`. Its `ExecStartPre`
runs the per-user `mineshare-typeless-prepare` helper before GNOME reads the
autostart entries. The helper atomically restores both managed entries from
`~/.local/share/MineShare/typeless-ordered.desktop`, so vendor deletion or
rewriting of the Typeless override cannot bypass the readiness coordinator at
the next login. No vendor or system-wide desktop file is modified.

The preparation helper does nothing when the broker opt-in marker is absent.
Its optional pre-start command cannot prevent desktop login if MineShare's
private setup files are damaged. Setup reloads user units without restarting
GNOME or either application. Re-run setup from the current helper files to
install this guard on an existing installation.

Validation on the real Ubuntu machine covered systemd parsing and transient
units executing the same pre-start helper after deleting or replacing the user
entry. Both restored the coordinator before the unit's main command; existing
applications stayed running and identity/settings/layout/trust hashes were
unchanged. Isolated tests also cover opt-out, concurrent coordinators, missing
readiness and spaced home paths. A new graphical login remains the final
acceptance step; it was not forced while the user had work open.

Limitations requiring acceptance testing: physical hotplug is not rediscovered by
the existing capture backend; replugged devices require a new ordered login.
Virtual keyboards/remappers are excluded in broker mode. Bluetooth UHID devices
may be excluded by the physical-only filter. If MineShare crashes, the kernel
releases hardware grabs, but Typeless re-acquisition behavior must be checked.
This is not yet certified for secure-login screens or Wayland session-lock UIs.
Quitting/restarting MineShare while Typeless is already open can also release
first ownership; the login guard does not implement a persistent broker process.

Rollback: remove `~/.config/MineShare/keyboard-broker.enabled`, restore the two
autostart entries from the printed backup folder (remove typeless.desktop if it
did not exist before), and log out/in. Do not run the old and new capture at once.
Also remove the MineShare-owned `50-mineshare-order.conf` user drop-in and run
`systemctl --user daemon-reload`. Removing the broker marker already makes its
condition allow normal Typeless login startup.
Remove the separate MineShare-owned drop-in under
`gnome-session-manager@.service.d/` as well. Removing the marker already makes
its preparation helper a no-op; the private canonical desktop entry and
`~/.local/bin/mineshare-typeless-prepare` may then be removed if no longer needed.

Acceptance, with Typeless open on both machines: PC physical mouse crosses to
Laptop, PC Right Alt activates Laptop only; Super / Win+Shift+S work on Laptop;
return cursor to PC and Right Alt activates PC only. Repeat after idle and with
modifier held across a handoff. Verify ordinary typing, Ctrl+C/V, Alt+Tab, mouse,
touchpad and audio. Unit/integration tests do not replace this physical test.
