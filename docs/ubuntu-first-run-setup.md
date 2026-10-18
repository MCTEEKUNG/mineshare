# Ubuntu first-run setup (release candidate)

Scope: bundle the keyboard broker and read-only GNOME pointer extension in the
`.deb`, and offer first-run detection plus explicit one-click configuration. A
GitHub user must not have to run setup scripts manually. Test before preparing a
new public release; do not silently deploy or publish it.

The `.deb` owns six helper files under `/usr/share/mineshare/setup/installer/linux`.
Detection changes nothing. The wizard opens for incomplete Linux setups and is
also available in Settings → Ubuntu setup. Windows does not show the wizard.
Users may defer, select individual features, retry errors or check again.

- Input setup: only the fixed package helper is elevated via polkit. It obtains
  the authenticating user's UID from pkexec, appends input-group membership and
  loads uinput if absent. Passwords never pass through MineShare.
- Typeless: optional, offered only when installed. Preserve the vendor desktop
  command/login flags, pairing/settings and backups. Enable the existing locked,
  readiness-gated startup coordinator. Never close either app during setup.
- GNOME 45–50 Wayland: copy the packaged read-only extension and enable its UUID
  for the next login, retaining other enabled/disabled extensions and backing up
  those lists. Do not change the global disable-extensions preference. X11 needs
  no extension; unsupported compositors get an honest manual-handoff notice.
- Configured is not active. After configuration, ask users to save work and log
  out/in themselves. Verify broker readiness for the current process and actual
  pointer D-Bus replies before showing active. Never force logout.

Tests: frontend consent/defer/reopen/error/pending-login flows, backend parsing
and path restrictions, disposable CI setup/retry preserving data and extensions,
existing broker/startup/lifecycle tests, and extraction/audit of the real `.deb`.
CI also checks Windows and Ubuntu 22.04/24.04 builds. A real GNOME desktop/polkit
acceptance test remains distinct from mocked CI and must not be claimed by it.

Existing dev-tool npm audit findings are outside this feature change; no major
Tailwind/Vite migration is included. Runtime/payload checks are not an assertion
that all dependencies are vulnerability-free.
