# Recovering keyboard ownership after an update

The 2026-10-21 in-session update restarted MineShare while Typeless remained
running. Typeless had exclusive access to a physical keyboard interface. The
existing readiness marker referenced a stopped MineShare PID, so the new process
could receive input from Windows but could not capture the Ubuntu keyboard.
Pointer-only fallback restored the mouse without restoring keyboard ownership.

The user explicitly authorized a Typeless restart. Recovery stopped that user's
Typeless app and its descendants with TERM, restarted MineShare, waited for a
live broker readiness marker with a positive keyboard count, then launched the
saved Typeless desktop entry with its original flags. No logout was required.
The marker now references the running MineShare PID and reports three keyboards.
Typeless is running again while MineShare retains that ownership.

Launching a desktop app through SSH must import the active graphical environment
from the user's systemd manager. Missing DISPLAY caused the first vendor relaunch
to exit. The recovery scripts now import only the graphical/IME environment
fields, and the saved launcher was used successfully afterwards. Four isolated
checks cover PID reuse, TERM-only signalling, live readiness and graphical
environment parsing.

The Typeless autostart entry was missing after vendor relaunch. The existing
ordered-startup entry was restored from the verified backup only after checking
that the MineShare coordinator remained unchanged. Both startup entries, the
coordinator and launcher preference now match the pre-recovery hashes. Identity,
layout, settings and saved pairings were retained. No application build changed:
both peers remain on `0e1c7dd7a4c3`, `control-v3`.

Scripts and evidence are in `artifacts/ubuntu-keyboard-recovery/`. The recovery
script defaults to read-only inspection; applying it requires explicit approval
because restarting Typeless can interrupt typing or recording. No forced KILL,
global input-permission changes or third-party file patches are used.

After an update with Typeless enabled, a connected peer and working mouse are
insufficient verification. Check that the broker marker belongs to the running
MineShare process, that its keyboard count is positive, that Typeless has reopened,
and that the physical Ubuntu keyboard can type in the intended Windows window.
The user confirmed the final physical typing check: after using Tee's mouse to
cross to the laptop, the physical keyboard connected to Tee successfully typed
in the laptop's Notepad. Keyboard recovery is verified with real source input.
