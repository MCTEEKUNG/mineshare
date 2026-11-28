# Connection and pairing controls

MineShare discovers nearby installations using mDNS on the LAN. Discovery only
populates the computer picker; it never authorizes a connection to a new device.
Each paired device has an independent encrypted connection. One destination at
a time owns keyboard, mouse, clipboard and file routing. System-audio send and
receive can be configured independently for every connected device.

Windows can also relay a received mouse and keyboard to another connected
device. With Ubuntu to the right of the laptop and a phone below it, move the
Ubuntu mouse left onto the laptop, then down onto the phone. Returning through
the phone's top edge restores the laptop cursor while Ubuntu remains the input
source. Both desktop installations must support receiver-owned edge detection.
An interrupted downstream connection releases the phone and returns to the
middle desktop. Clipboard and files remain associated with the selected peer.

On Android, enable **Send phone audio here** for the laptop and approve the
system sharing prompt. The laptop's receive-audio switch alone cannot grant
Android capture permission. Playback capture copies eligible media/game audio.
With MineShare control enabled, the phone's volume buttons adjust the shared
media stream while its local speaker stays quiet. Stopping sharing or losing
the receiving connection restores the latest level on the phone. While
disconnected, volume buttons keep their ordinary Android behavior. Calls retain
normal phone volume handling. Apps may prohibit capture.

Shared phone media volume is exposed through an Android remote MediaSession
with an absolute VolumeProvider. Phone volume buttons request the system volume
panel, and its slider updates the same shared-stream level. MineShare does not
draw a replacement volume popup. Ending the route releases the remote session
and restores normal phone volume handling.

While the computer controls Android, Windows alone (or Windows+Tab) opens
recent apps, Windows+D goes Home and Esc goes Back. Alt+Tab requests Android's
recent-app quick switch. Click a text field before typing; printable text is
sent through the accessibility input connection, while editing keys retain
their key semantics. Shortcut modifier state is cleared when control ends.

## Pair a computer

1. Open **Add a device** on Home. Names, operating systems, addresses and
   short device IDs distinguish computers with similar names. An older app that
   does not advertise request pairing is shown with **Update required**.
2. Select the intended computer and confirm **Send request**. A confirmation
   identifies the intended device. Existing connected devices keep their sessions.
3. The chosen computer displays the sender's name/address and **Accept request**
   or **Decline**. Acceptance is scoped to that encrypted connection and attempt.
4. For a first-time pairing, the receiver then displays its PIN and the sender
   enters it. Computers that already trust each other's keys need approval to
   switch destinations, but do not repeat the PIN exchange.

**Cancel pairing request** aborts an outgoing request or PIN exchange. It cannot
turn into a blanket LAN-wide pairing window. Declined, expired, cancelled and
failed requests do not keep retrying new pairing in the background.

**Disconnect & pause** cancels all active bridges and pending requests and blocks incoming
connections and automatic dialing until **Resume connections**. Trust and the
selected computer are retained; the pause lasts for the app process.

## Selection, ownership and recovery

Successful pairing enables automatic reconnection for that authenticated device.
Each enabled device reconnects independently. On upgrade, the previous selected
device becomes the initial enabled connection and existing trust keys are retained.
Adding a device keeps the other enabled connections. A trusted device still has
to match its stored Noise public key and device ID on every connection.

While disconnected, the workspace and computer picker show the saved target's
name and short device ID. An OS hint is shown only when discovery has a matching
device ID; legacy saved pairings have no OS metadata, so offline OS labels are
not guessed. Windows and Ubuntu on the same physical PC are separate pairings.
Choose the running installation and send a request to switch destinations.
Nearby computers, including another installation with the same name, never
replace the saved target merely by being discovered.

A new outgoing pairing request leaves established connections active. Pairing
consent remains scoped to its device and attempt. Duplicate TCP connections to
the same device converge on authenticated-key ordering at both ends. Each
device has its own session cleanup, so a departing device leaves the other
connections active. Input target changes release held input before enabling the
new target. Android uses its own display and permission-gated accessibility input.

Requests expire if they have not started within 90 seconds. Consent and PIN
negotiation have a bounded 90-second session deadline after setup, and TCP/Noise
setup has separate timeouts. EOF while waiting for approval/PIN is detected;
stale approvals and submitted PINs are rejected using the attempt ID. A declined
request closes its receiver dialog immediately.

mDNS expiry removes a computer from the nearby picker. A trusted peer's last
endpoint remains available to its reconnect worker, so a temporary LAN outage
does not discard the selected destination. Discovery refreshes while disconnected
and tracks network/interface/address changes. Every reconnection checks the
Noise key, device ID, enabled connection and compatible protocol before input,
audio or clipboard routing starts.

## Compatibility and verification

All devices require `control-v5`: an encrypted identity/request hello precedes
consent, PIN verification, capability negotiation and port exchange. Older
protocols display an update requirement. Existing identities and trust keys
remain valid after the apps are updated.

The following checks record the earlier request-pairing implementation.

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

## Android cursor and scrolling

### Phone keyboard navigation

While the phone owns computer input, press **Windows**, then type an app name (for example `LINE` or `ไลน์`). MineShare opens app search, ranks exact names first, and **Enter** launches the selected app. **Up/Down** changes selection and **Esc** closes search. Windows without typing still opens recent apps; Windows+Tab, Alt+Tab and Windows+D retain their previous actions. Clicking a recent app cancels the pending search. The phone home screen also has a **ค้นหาแอป** button.

**Ctrl+Alt+O** toggles portrait/landscape. **Ctrl+Alt+Shift+O** enables automatic rotation. The **แนวหน้าจอ** card provides the same controls. Android asks for permission to modify system settings on first use; enable it and repeat the command. Apps that require a particular orientation may retain it.

**Ctrl+Alt+Space** cancels a stuck gesture and rebuilds the pointer at the centre of the phone. Pointer windows are also reattached automatically if detached while the phone still owns input. Release cleanup runs immediately on the input handler so it cannot erase a subsequent control handoff.

Open **เคอร์เซอร์และการเลื่อน → ปรับแต่งเมาส์บน Android** to preview and
save cursor size (75–300%), shape, color, outline and shadow. Pointer smoothing,
wheel speed, scroll smoothing and drag smoothing apply without reconnecting.
Cursor rendering follows display frames; hit testing and edge detection retain
the input position. Wheel motion received during a gesture is accumulated in a
bounded queue. Drag continuations preserve intermediate points and the final
release position. Use **คืนค่าเริ่มต้น** to reset these preferences.
