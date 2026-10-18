# Ubuntu keyboard broker (opt-in)

Typeless's Linux keyboard monitor exclusively grabs physical evdev keyboards.
MineShare's old conditional grab loses with EBUSY when both are running. A
protocol/scancode change cannot recover input the source never receives.

`installer/linux/install-typeless-compat.sh` enables the broker for the graphical
user and installs a per-user, readiness-gated autostart coordinator. Install the
normal desktop launcher first. No vendor files, Typeless shortcuts, or Windows
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

Limitations requiring acceptance testing: physical hotplug is not rediscovered by
the existing capture backend; replugged devices require a new ordered login.
Virtual keyboards/remappers are excluded in broker mode. Bluetooth UHID devices
may be excluded by the physical-only filter. If MineShare crashes, the kernel
releases hardware grabs, but Typeless re-acquisition behavior must be checked.
This is not yet certified for secure-login screens or Wayland session-lock UIs.

Rollback: remove `~/.config/MineShare/keyboard-broker.enabled`, restore the two
autostart entries from the printed backup folder (remove typeless.desktop if it
did not exist before), and log out/in. Do not run the old and new capture at once.

Acceptance, with Typeless open on both machines: PC physical mouse crosses to
Laptop, PC Right Alt activates Laptop only; Super / Win+Shift+S work on Laptop;
return cursor to PC and Right Alt activates PC only. Repeat after idle and with
modifier held across a handoff. Verify ordinary typing, Ctrl+C/V, Alt+Tab, mouse,
touchpad and audio. Unit/integration tests do not replace this physical test.
