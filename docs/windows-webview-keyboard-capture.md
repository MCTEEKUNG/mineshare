# Keyboard capture while the controller UI is visible

## Observed failure

On Windows 11, the PC's physical mouse crossed to the Laptop while the PC's
MineShare window remained foreground. Windows and Alt+Tab ran on the PC instead
of the Laptop. Minimizing the controller window avoided the failure.

The hardware Windows-key down/up appeared in WM_INPUT, but not in the host's
WH_KEYBOARD_LL callback. Delete and F2 were captured after the Windows key opened
SearchHost and changed foreground. Network and destination injection were not
the earliest divergence. A native child focus proxy did not fix this and was
removed. This matches the observable host-process issue reported in
[Wry #1761](https://github.com/tauri-apps/wry/issues/1761); its internal Chromium
mechanism remains unproven.

## Fix

The GUI starts the same executable with a private keyboard-worker argument,
before initializing Tauri, single-instance handling, or the daemon. This child
owns only WH_KEYBOARD_LL. Physical keyboard records and consume/pass decisions
travel over inherited anonymous pipes. The GUI remains the single owner of
routing, hotkeys, held-key tracking and network delivery. Injected input never
enters the pipe. The child has no GUI, pairing state, discovery or startup entry.

The worker chains unconsumed events to other hooks. Parent-pipe closure exits the
child even while idle. A pipe failure or 100 ms decision timeout fails open and
stops the child; the GUI revokes remote control, releases forwarded keys and
restarts an isolated worker. It never permanently falls back to the WebView
host's broken hook path. Standalone non-GUI capture retains its native hook.
Worker exit status and bounded IPC error details are logged without key contents.
Normal routing is event-driven, not periodic polling; the bounded
response wait is a failure guard, not a claimed latency guarantee.

The app does not minimize windows, steal foreground, inject focus clicks, or
synthesize masking modifiers. Existing foreground-sensitive touchpad handling
and forwarded-state reconciliation remain unchanged.

Ubuntu uses evdev capture outside the WebKit keyboard path, so this worker is
Windows-only. Shared tests cover Meta, Delete and function-key routing and a
release arriving after cursor return. Ubuntu physical acceptance was not run
during this Windows-to-Windows investigation.

## Acceptance

With the worker enabled on both Windows machines, the user confirmed Windows,
Alt+Tab, Delete and F2 work on the Laptop with the PC's physical keyboard/mouse
and the PC MineShare window still open. Source traces then contained the Windows
down/up even while MineShare remained foreground, and the target trace contained
matching edges. Temporary diagnostic key logs were removed after this comparison.

The original recovery path had a second bug: a child exit retired permanently to
the host hook. This was observed on the PC while the Laptop's worker stayed
alive. Regression tests now inject repeated IPC failures and require a restart
after every failure; live acceptance must also stop only the worker and verify
a new PID appears while the GUI PID remains unchanged.

Keep physical regression checks for both directions, rapid chords, long idle,
crossing while holding modifiers, ordinary typing, touchpad, dragging and audio.
Unit tests cannot replace these hardware checks.
