# Ubuntu setup after an upgrade

Reported on Tee on 2026-10-21: the setup page appeared frozen, with permission
choices and its forward action unavailable, while Windows reported Connected.
The user used Tee's physical mouse and could click other Ubuntu applications.

The live accessibility snapshot showed that input permissions, Typeless startup
compatibility and the GNOME pointer extension were already configured. Input and
pointer support were active; Typeless compatibility awaited the next ordered
login. All three choices were disabled, but displayed unchecked, and the only
primary action was disabled because no new changes were selected. The native
Close action worked, and the user confirmed normal mouse interaction after
closing the dialog. The established network bridge kept working throughout.

The UI conflated configuration actions with existing configuration. It now
checks already-configured, locked choices, explains that authorization need not
be repeated, and offers an enabled Continue to workspace action when no new
changes are selected. New setup changes still require explicit selection and
confirmation. Continuing does not invoke configuration or grant permissions.
Pending-login information stays visible; no logout is forced.

Regression loop: `cd ui; npm test -- src/LinuxSetup.test.tsx`. The added test used
the live upgrade state and failed before the fix with `expected false to be true`
for the existing permission checkboxes. After the fix, all 30 UI tests and the
production build passed on Windows and in the prepared Linux Docker builder.

The MSI and deb were installed on both real computers from source
`1747886a3dcf`, protocol `control-v3`. Package checks, installed-payload hashes,
configuration and startup preservation passed. The real session reconnected with
matching source IDs. On Tee, the native accessibility probe opened setup from
Settings, verified all three configured choices were checked, activated the
enabled Continue action, and verified workspace navigation afterwards.

Diagnostic scripts, the original snapshot, the native probe result, browser
preview and deployment evidence are retained under
`artifacts/ubuntu-setup-freeze/`. Physical LAN interruption/recovery remains a
separate user acceptance test. Connected describes the network bridge; it does
not attest that an optional setup or login step has been completed.
