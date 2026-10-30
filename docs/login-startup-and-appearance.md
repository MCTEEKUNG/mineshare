# Login startup and appearance

Development change after 0.0.14; not part of the published 0.0.14 installers.

Settings contains **Start with operating system**. This means graphical user
sign-in, not the boot/login screen. Changing it does not stop the running bridge.

- Windows: reuses `MineShareLaunch`, preserving its executable, working directory,
  triggers and principal. Existing Administrator privileges are retained.
  Enabling startup adds `--mineshare-autostart` to a task with no arguments (or
  keeps that flag if already present); custom arguments are refused, not replaced.
  The login launcher retries abnormal process exits up to three times, three
  seconds apart. Normal Quit and duplicate-instance exit code 0 stop the launcher.
  Keyboard-worker launches are handled first and never enter this launcher.
  Native Task Scheduler retry is disabled so it cannot multiply the app's retry
  budget. That setting alone did not retry exit 101 in the Windows 11 probe.
  A fresh setup creates a limited, interactive per-user logon task; it does not
  silently grant Administrator privileges. A matching legacy Run entry is retired
  when the task is used, avoiding duplicate startup. Errors are shown in the UI.
- Ubuntu: manages `~/.config/autostart/MineShare.desktop` (or `XDG_CONFIG_HOME`).
  Existing Exec and other desktop-entry sections are retained. No root access is
  needed. Where startup is managed by the optional Typeless coordinator, the
  setting reports that state but does not rewrite or disable the coordinator.

Appearance already offered Light, Dark and System. New/unrecognized preferences
now default to **System**, following `prefers-color-scheme` at first paint and on
OS theme changes. Explicit existing Light/Dark choices are retained. Preferences
are stored per-device; an unavailable storage backend no longer prevents launch.

## Fresh Ubuntu input access

If Laptop input works on Ubuntu but Ubuntu hardware input cannot cross back,
check the first startup logs for `no evdev mouse/keyboard found`. Installing the
package grants input-device access, but an already-running graphical process may
still have its old supplementary groups. Sign out/in after new group membership,
then restart MineShare; pairing again does not grant device access. In the Ubuntu
26.04.1 installation checked here, later logs showed evdev devices and outward
crossings, and physical acceptance confirmed Ubuntu mouse to Laptop worked after
restarting the app. This is not certification of all Wayland features.

## Checks

Frontend tests cover new preferences following dark/light OS settings, retaining
explicit choices, and unavailable storage. Backend tests cover desktop-entry
quoting, toggle idempotency, filesystem readback, and coordinator preservation.
Windows startup status is also checked read-only against the installed task.
The new UI/backend still require native packaged acceptance on both operating
systems, including startup after logout/login; unit tests do not prove that.

The October 27, 2026 laptop task was enabled but had returned 101 before the GUI
logger was initialized. Its precise original failure remains unknown. Logging
and the panic hook now initialize before Tauri creates the window/WebView, so a
future early failure can be diagnosed. Windows launcher tests include a real
benign child that exits 101, retries after three seconds, then exits 0; persistent
failure and normal-exit tests verify the retry bound and Quit behavior. They do
not replace a physical sign-in or tray-Quit acceptance test.
