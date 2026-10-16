# MineShare Windows installation and updates

## Packaged downloads (0.0.13 onward)

Double-click either the MSI or the EXE. Both use the same MSI payload, upgrade
identity and registration; changing download format must not install a second
copy. Windows may request administrator approval. Install/update using the same
Windows account that owns the existing per-user installation.

The installer detects older MSI and NSIS installations, plus the fixed legacy
script location at `%LOCALAPPDATA%\\MineShare`. Existing installations get an
Update page and reuse their directory. Pairing, settings and logs are outside
the MSI's component ownership and are not reset. Existing startup tasks (including
disabled state), Run entries and their arguments are left unchanged. A fresh
installation does not enable startup without the user's choice.

A verified NSIS registration is removed after the MSI succeeds, without running
the old NSIS uninstaller against new files. MSI file replacement is transactional;
the helper restarts a previously running app after success or rollback. A failed
migration must not be described as a successful update.

Silent deployment:

```powershell
msiexec /i "MineShare_0.0.13_x64_en-US.msi" /qn /norestart AUTOLAUNCHAPP=0
.\\MineShare_0.0.13_x64-setup.exe /S
```

Keep the MSI UpgradeCode unchanged. Do not replace assets of a published version.
Use a higher application version for subsequent releases. Installers remain
unsigned; verify SHA-256 before running them.

## Development / advanced per-user deployment

Build the GUI, not a second standalone daemon:

```powershell
cd ui
npm ci
npm run tauri -- build --no-bundle
cd ..
powershell -ExecutionPolicy Bypass -File installer\\windows\\install.ps1
```

The script installs the GUI at `%LOCALAPPDATA%\\MineShare`, refreshes the icon and
registers one `MineShareLaunch` task in the interactive user's session. It is not
a Windows service and must not run the bridge as SYSTEM. Packaged MSI upgrades
can detect this legacy directory; no fake MSI registration is created by the
development script.

## Verification and release gate

- `test-msi-package.ps1`: stable upgrade identity, legacy detection, Update UI
  and migration/rollback actions are present in the compiled MSI.
- `test-upgrade.ps1`: **GitHub-hosted disposable Windows runners only**. Tests
  MSI→MSI, MSI→EXE, EXE→MSI, EXE→EXE, script→MSI/EXE, fresh install, repeat-install,
  downgrade rejection and injected MSI/NSIS migration failures. Check file version, original location, one registration,
  unchanged pairing/settings hashes and exact startup-task XML/Run values.
- Do not publish until normal CI, both platform builds and every upgrade scenario
  pass. Silent-install tests do not replace a visual review of the wizard.
- Do not run destructive acceptance tests on the user's Laptop or PC.

For developer uninstall use `installer\\windows\\uninstall.ps1`; for a packaged
installation use Windows Settings → Apps. Do not run a leftover old NSIS
uninstaller after migrating to MSI. User configuration and logs must remain.

VB-CABLE is optional and installed separately for peer-microphone playback.
