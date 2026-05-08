# MineShare — Windows install

Per-user installer for the daemon side. No admin needed.

The Slice 4 MSI wraps the same artifacts in a real installer for
distribution; this script is for development + advanced users.

## why not a Windows Service

A SYSTEM-context Windows Service runs in its own session, with no
desktop, no audio endpoint, no clipboard, and no ability to install
WH_MOUSE_LL / WH_KEYBOARD_LL hooks for the user's input devices. We
need all of those, so the daemon registers as a **logon-time**
entry in the user's Startup folder instead. That keeps it scoped to
the user's interactive session, where everything it needs is
available.

## install

From a fresh checkout, after building the daemon:

```powershell
cargo build --release -p mineshare-daemon --bin mineshare-daemon

# from the repo root:
powershell -ExecutionPolicy Bypass -File .\installer\windows\install.ps1
```

The script will:

1. stop any running daemon so it can overwrite the exe
2. copy `target\release\mineshare-daemon.exe` →
   `%LOCALAPPDATA%\Programs\MineShare\`
3. create a Startup-folder shortcut so the daemon launches at
   logon (minimized window style)
4. start the daemon immediately

## verify

```powershell
Get-Process mineshare-daemon
```

mDNS announce should advertise this host inside ~1s; pair with the
Linux side via the existing browse loop. Logs go to
`%APPDATA%\MineShare\logs\` (whatever the daemon's `logs::init`
writes).

## uninstall

```powershell
powershell -ExecutionPolicy Bypass -File .\installer\windows\uninstall.ps1
```

## notes

* The console window briefly appears on logon because the daemon
  is built as a console app. Replacing the binary with a
  windows-subsystem build (no console) is M5 polish.
* If you want VB-CABLE-routed peer-mic playback, install
  https://vb-audio.com/Cable/ separately, then restart the daemon.
  The daemon auto-detects "CABLE Input" at startup and skips it
  with a warning if missing — install order doesn't matter.
