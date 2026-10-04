# Game Lock border visibility hotfix

## Evidence

Both PC request logs and Laptop state-change logs confirm that Ctrl+Alt+L reached
the controlled Laptop. The missing border was not evidence of a lost lock command.
No feedback-worker error was logged for those transitions.

The previous regression checked `IsWindowVisible`, not displayed pixels. A new
interactive screen-pixel check reproduced a missing border. Further checks also
passed with the old renderer under different foreground conditions, so the old
painting method alone has not been isolated as the sole trigger. A fullscreen
browser was present during investigation; exclusive-fullscreen games have not
been validated.

## Changes

- Render each animation frame into a 32-bit off-screen bitmap and explicitly
  present it with `UpdateLayeredWindow` and per-pixel transparency.
- Set per-monitor DPI awareness on the feedback worker for physical-screen bounds.
- Reassert topmost placement without activation during the short animation.
- Pump window messages while idle instead of blocking indefinitely on the
  animation channel. The hidden window must remain responsive to Windows.
- Preserve click-through/no-activate styles, duration, custom effects, and unlock
  animation. No mouse click or foreground activation is added to production code.

The rendering approach follows Microsoft's
[UpdateLayeredWindow documentation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-updatelayeredwindow).

## Verification

Interactive tests pass for an actual red border pixel on Laptop, unchanged
foreground window, click-through/no-activate flags, automatic hiding, and a
responsive hidden window after animation. The pixel check observes the composed
desktop, not merely the visibility flag. A bounded manual visual-probe test is
also available for future hardware diagnosis.

Physical hotkey acceptance remains to be confirmed by the user after deployment.

Workspace tests: 212 passed, 12 hardware/manual tests ignored by default. Three
interactive feedback tests were invoked separately and passed. Clippy with
warnings denied, formatting, and whitespace checks passed.

Both installed binaries were verified against SHA-256
`4B0FBE3CB32BCB6E622BA77404C4C5746164971EDBAD9FE94D9776A948320CC1`.
Previous binaries are retained as `mineshare-app.backup-20260912-124350.exe` on
each PC. Peer WebView initialization stalled on the first launch after install;
only MineShare and its directly owned WebView processes were restarted.
Both peers subsequently reconnected automatically. Live stats showed zero
injection/decryption errors during the post-install idle check.

## Follow-up: physical PC keyboard still does not show the effect

The user reported failure on both targets after the hotfix. This remains
unresolved; standalone pixel tests were insufficient acceptance evidence.

A diagnostic build is installed on both machines (SHA-256
`C032A466FCF35EA340DFC7001D97EC8B613C077485A9B0B7A1D4030ABB005A84`,
source `2835d0ca57af`). It records request, window creation, animation start,
frame count/completion, monitor lookup failure, and a closed feedback channel.

Computer Use inspection of the installed Laptop application confirmed a visible
rainbow border from both Preview and the real UI lock toggle. The preview and
lock each rendered 49 frames in roughly 1.16 seconds; unlock rendered 32 frames
in 0.71 seconds. The temporary UI lock was restored to off. No effect setting was
changed. This does not establish success for the user's physical PC keyboard.
Next required evidence is a physical PC Ctrl+Alt+L press on the diagnostic build
so its feedback trace can be compared with these successful UI-triggered traces.

## Physical retest: Laptop passes; PC visibility still unresolved

The user confirmed that the physical PC keyboard shows the border when controlling
the Laptop. On the PC itself locking works, but the border remains invisible.
PC logs at 06:20:45 and 06:20:49 UTC show successful 1920x1080 animations
with 54 and 57 frames; no presentation error was recorded. This rules out a
missing hotkey request, but does not prove visibility on the physical display.

An interactive-session metadata probe found the PC's hidden idle Static feedback
window at 0,0,1920,1080 with DWM cloak=0. RobloxPlayerBeta was foreground
(rect -8,-8,1928,1040); exclusive fullscreen has not been established.
Desktop/Notepad comparison is requested before changing the working renderer.
`scripts/probe-lock-window.ps1` reads window metadata only and optionally observes
visibility changes for at most 60 seconds; it does not activate or capture windows.

### Desktop retest and topmost initialization fix

The user also reported failure outside Roblox. Comparing live window styles found
the Laptop feedback window retained WS_EX_TOPMOST (`0x080800a8`), while the PC
feedback window lacked it (`0x080800a0`). Both had no owner and cloak=0.
The overlay now requests WS_EX_TOPMOST at creation instead of depending solely
on later promotion by a background process. Activation and click-through styles
are unchanged. Animation completion logs now include the actual topmost state.
This is a targeted correction; physical hotkey acceptance is still required.

After this change both interactive desktops passed the visible-red-pixel and
no-activation/hide tests (2 each). Input tests: 86 passed, 10 hardware tests ignored;
input clippy with warnings denied passed. A prior standalone PC pixel run failed,
while subsequent runs before the change passed, so isolated pixel success alone
must not be treated as proof that the installed hotkey path is fixed.

Release SHA-256:
`695B58CC5BFC8A6CBC7EC345324D0DAB0E2EB9D49AB6E06530F18DCBB7A62354`.
