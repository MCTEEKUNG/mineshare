# Connection and pairing controls

MineShare discovers nearby installations using mDNS on the LAN. Discovery only
populates the computer picker; it never authorizes a connection to a new device.
There is one input/audio/clipboard bridge at a time.

## Pair a computer

1. Open **Choose a computer** on Home. Names, operating systems, addresses and
   short device IDs distinguish computers with similar names. An older app that
   does not advertise request pairing is shown with **Update required**.
2. Select the intended computer and confirm **Send request**. A confirmation
   explains when the current connection will be disconnected.
3. The chosen computer displays the sender's name/address and **Accept request**
   or **Decline**. Acceptance is scoped to that encrypted connection and attempt.
4. For a first-time pairing, the receiver then displays its PIN and the sender
   enters it. Computers that already trust each other's keys need approval to
   switch destinations, but do not repeat the PIN exchange.

**Cancel pairing request** aborts an outgoing request or PIN exchange. It cannot
turn into a blanket LAN-wide pairing window. Declined, expired, cancelled and
failed requests do not keep retrying new pairing in the background.

**Disconnect & pause** cancels the active bridge/request and blocks incoming
connections and automatic dialing until **Resume connections**. Trust and the
selected computer are retained; the pause lasts for the app process.

## Selection, ownership and recovery

Successful pairing/selection persists `selected_device_id` alongside the existing
trust entries. Automatic reconnection only targets that selected, authenticated
computer. A legacy trust list with one entry keeps its reconnect behavior; if
several entries exist without a selection, choose one explicitly rather than
letting the first LAN responder win. Other saved pairings are retained.

While disconnected, the workspace and computer picker show the saved target's
name and short device ID. An OS hint is shown only when discovery has a matching
device ID; legacy saved pairings have no OS metadata, so offline OS labels are
not guessed. Windows and Ubuntu on the same physical PC are separate pairings.
Choose the running installation and send a request to switch destinations.
Nearby computers, including another installation with the same name, never
replace the saved target merely by being discovered.

A new outgoing request temporarily takes priority over the former destination
and waits for old routing workers to finish. Unrelated incoming requests cannot
replace an active connection or a request aimed at another computer. Duplicate
TCP connections converge on the same authenticated-key ordering at both ends;
retired tasks cannot erase a replacement's state.

Requests expire if they have not started within 90 seconds. Consent and PIN
negotiation have a bounded 90-second session deadline after setup, and TCP/Noise
setup has separate timeouts. EOF while waiting for approval/PIN is detected;
stale approvals and submitted PINs are rejected using the attempt ID. A declined
request closes its receiver dialog immediately.

mDNS expiry removes a computer from the nearby picker. A trusted peer's last
endpoint remains available to its reconnect worker, so a temporary LAN outage
does not discard the selected destination. Discovery refreshes while disconnected
and tracks network/interface/address changes. Every reconnection checks the
Noise key, device ID, selected destination and compatible protocol before input,
audio or clipboard routing starts.

## Compatibility and verification

Both computers require `control-v3`: an encrypted identity/request hello precedes
consent, PIN verification and port exchange. `control-v2` peers are not silently
paired or admitted through the old window. Existing identities and trust keys
remain valid after both apps are updated.

Windows checks: 64 daemon tests, 9 network tests, 30 UI tests, frontend production
build and Tauri `cargo check`. Linux checks run the same relevant tests and
frontend build in the prepared Docker builder (62 daemon tests and 9 network
tests; two daemon tests are Windows-specific). The encrypted TCP/Noise consent
test verifies that approval is required, decline is honored, stale answers are
rejected, and a disconnected requester does not strand the receiver.

Browser fixtures exercise a three-computer picker, confirmation, sending only to
the selected ID, waiting/cancellation, receiver approval, and PIN display only
after approval. Preview screenshots are in `artifacts/request-pairing/`.

Installed on the real Windows/Tee pair on 2026-10-21 from source
`75c6aa848d2c`, with matching `control-v3` session logs on both machines.
The installed executables match their MSI/deb payloads. Identity, layout,
settings, saved trust entries and startup configuration were retained; the
successful explicit selection added the chosen destination to the trust file.
See `artifacts/request-pairing/release/verification.md` for deployment evidence.

Physical LAN interruption/recovery remains to be tested by the user. A third
LAN instance is still needed to check that it cannot take over, together with
decline/accept, pause/resume, and interruption during pairing.
