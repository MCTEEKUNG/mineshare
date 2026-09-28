# Focus-aware Game Lock and stability release — 2026-09-12

## Cause and fix

Ctrl+Alt+L previously toggled the lock on the keyboard's physical host. When
that host was forwarding its mouse to the peer, this also exited remote control:
the wrong desktop received both the lock and its visual feedback.

The hotkey now follows concrete cursor ownership (the same ownership decision
used by Smart keyboard routing). A remote target receives an encrypted control
command; the source keeps forwarding mouse/keyboard input and clamps its virtual
cursor at the target boundary. The second press unlocks that target. GUI/tray
buttons remain explicit local-desktop actions. Game detection stays advisory;
it never automatically enables the lock.

Commands and acknowledgements carry ownership epochs and request IDs. Old
handoff replies and superseded acknowledgements cannot re-lock a new owner.
TakeControl is published before a subsequent lock command. Disconnect/teardown
clears lock state. Ctrl+Alt+R remains the manual recovery path. No simulated
click or foreground activation was added.

## Observability and deployment

- Build IDs now include a source fingerprint and `control-v2` marker. A peer
  with an incompatible control protocol is rejected before accepting input.
- RTT uses monotonic microsecond timestamps, preserving fractional milliseconds
  without dependence on wall-clock synchronization. Periodic RTT logging also
  continues after the sample ring fills.
- Pair deployment verifies the staged binary before interruption, saves each
  previous executable, installs both PCs, and compares final SHA-256 hashes.
- Settings, trust, and identity are preserved. Rollback must restore both peers
  together because the control protocol changed.

## Verification

- Rust workspace: 212 tests passed; 10 hardware/manual tests ignored by default.
- Clippy across workspace/all targets with warnings denied: passed.
- Rust formatting and whitespace checks: passed.
- UI: 3 tests passed; TypeScript and Vite production build passed.
- Separately invoked Windows hook regression: passed. It exercises the actual
  keyboard-hook function with controlled key records, including held-key repeat,
  target routing, locked boundary clamping, stale requests, and teardown.
- Encrypted control round-trip and protocol compatibility tests: passed.
- Native mouse-injection benchmark, 1,000 paced samples on Laptop: p99 514.9 us,
  maximum 733.8 us. This measures injection only, not physical-device-to-display
  latency and not an improvement percentage against a recorded baseline.

## Acceptance limits

The test suite is not proof that the application has no bugs. Physical two-PC
shortcut/gesture acceptance and acoustic audio latency require real hardware
interaction. End-to-end latency below 1 ms has not been established.

After installation, test each direction with the source PC's physical mouse and
keyboard: cross to the other desktop, press Ctrl+Alt+L, verify the effect appears
only there, move toward its edges, type, then press the hotkey again to unlock.
Also check Alt+Tab, Win+Shift+S and screenshot paste, wheel/drag, and audio while
crossing. The critical acceptance condition is that no foreground window is
minimized and no key, cursor, or lock remains stuck.

## Installed release

Installed on the Laptop and peer PC at approximately 11:53 Bangkok time.
Both installed executables match the release SHA-256:

`9700C0B854F398ACAD4FA9802E4275AB736BEA2EC7C42694E2C6EB19D1C82DF3`

Source fingerprint: `06bac022668a`; protocol: `control-v2`.
Previous binary retained on each PC as
`mineshare-app.backup-20260912-115331.exe` in its per-user MineShare install
directory. This is a binary backup, not a backup of all user settings.

Both scheduled tasks are running with exactly one app process each, in their
interactive user sessions (Laptop 2, PC 1). They automatically reconnected after
deployment and negotiated native touchpad capabilities. Observed idle control
RTT samples include 1.021 ms and 1.101 ms on PC, and 1.351 ms on Laptop; these are
individual round-trip samples, not end-to-end input percentiles. Observed idle
input injection/decryption error counts are zero.
RTT logging was also observed after the 128-sample ring filled (peer sample at
04:55:02 UTC: 0.963 ms), confirming the logging fix in the installed build.
Both backup binaries match the prior release hash
`31DC672E0254E5C43FC4F8140071CFD6234C3AA1931FCD8F8344061303D32336`.

Laptop playback opened the configured headset and peer process-loopback capture
started successfully. No active audio was observed in this idle check; audible
playback quality and acoustic latency have not been revalidated. Optional virtual
microphone playback still reports VB-CABLE absent; no driver was installed and
this is separate from receiving normal peer system audio.

Physical hotkey acceptance has been requested from the user and remains pending.
