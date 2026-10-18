# Ubuntu package upgrade lifecycle

The `.deb` package is named `mine-share`. `apt install ./MineShare_<version>_amd64.deb`
upgrades that existing package in place: dpkg replaces its old application files.
Do not uninstall first, delete user profiles, or remove arbitrary downloaded
installers/backups. Pairing, settings and autostart entries are not package payload.

Previously, a running GUI could retain the old executable inode after an upgrade;
`/proc/<pid>/exe` then ended in `(deleted)`. The package version on disk could say
0.0.15 while the running application still executed 0.0.14.

New hooks (not included in the original 0.0.15 candidate):

- A self-contained `preinst` records users running `/usr/bin/mineshare-app` in
  root-owned temporary state under `/run`, requests termination and waits before
  unpacking. If needed it escalates to KILL after five seconds, checking UID and
  process start time before every signal to avoid recycled PIDs. Other paths and
  applications are excluded. Failure to stop aborts before replacing files.
- `postinst configure` resumes only previously running users, via their existing
  graphical systemd user manager after dropping root privileges. Fresh installs
  and headless sessions do not open a GUI. No second GUI is started if one already
  exists; startup failures are reported and leave retry state, not a broken dpkg
  configuration. Pairing, settings, startup and Typeless files are not rewritten.
- `prerm upgrade` no longer deletes the udev device-access rule. Actual removal
  still removes the rule, preserving users' input-group membership.

`node installer/linux/build-deb-hooks.mjs` embeds lifecycle functions into each
hook during Linux bundling: preinst cannot depend on a new-package helper that
has not been unpacked yet. Tauri's Linux bundle hook runs the generator.

If apt says **already the newest version**, no installation transaction happens,
so maintainer scripts do not run. Use a newer package normally; use `apt install
--reinstall ./file.deb` only when intentionally reinstalling the same version.
It is not possible for a package hook to intercept apt's no-op decision.

Tests cover stop escalation/PID reuse, executable scope, graphical/headless
restart decisions and idempotence. Disposable GitHub runners also perform a real
dpkg upgrade/reinstall with fixture ELF binaries, check the old process stopped,
and verify pairing/autostart fixture hashes. Native desktop restart acceptance
must still be verified on a graphical Ubuntu session with a newly built package.
