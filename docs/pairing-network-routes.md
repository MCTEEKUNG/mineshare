# Pairing from a computer with multiple advertised routes

On 2026-10-25, MCTEEKUNG-PC could receive a pairing request from the laptop,
but sending in the other direction stalled. Both machines had the same
0.0.16 / 73f4e9520e57 / control-v3 executable. The user confirmed the button
worked, but the request waited and failed.

The laptop advertised several IPv6 link-local and IPv4 169.254 addresses along
with its working LAN IPv4 address. The old dialer tried the unordered list
sequentially, with a four-second timeout per address. Consequently, unreachable
adapter addresses delayed reaching the actual listener. PC logs show the Noise
handshake eventually completing, followed by `pairing request was cancelled or
expired`; the receiving laptop saw EOF. Sending from the laptop then paired.

A bounded native TCP probe from PC timed out on two advertised IPv6 addresses
and both 169.254 addresses, while 192.168.1.108 connected in 3.81 ms. The new
production connection helper, supplied with all five candidates, selected the
LAN route in 14.79 ms. The reverse probe connected in 4.75 ms. These are TCP
route timings, not the duration of human approval/PIN entry or audio latency.

## Implementation

`mineshare-net::connect::connect_peer` deduplicates candidates and prefers
ordinary LAN IPv4, then non-link-local IPv6, then link-local routes. It gives
the first route a brief head start and starts fallbacks within 300 ms. All
candidates share one four-second deadline; address count no longer multiplies
the wait. Losing futures and their sockets are dropped when a route succeeds
or the caller cancels. No background connection tasks survive the attempt.

The daemon calls this helper before the existing Noise handshake and logs the
chosen route and elapsed time. Request consent, PIN checks, identity validation,
pause/disconnect and trusted reconnect retain their existing rules. No adapter,
driver, firewall, power, or Wi-Fi settings were changed. Audio code is unchanged.

## Verification

- Regression first failed with the previous sequential algorithm: a dead first
  address prevented reaching a live address within the test's 500 ms limit.
- Regression now passes; additional checks cover multi-adapter preference and
  deduplication, one overall timeout, cancellation of losing connections, and a
  real TCP listener following a stale port.
- Windows: 65 daemon + 14 network tests passed. Linux in the prepared Docker
  builder: 63 daemon + 14 network tests passed. Clippy and formatting passed.
- MSI source 30f1a39488c1 is verified against its source manifest and extracted
  payload. Deployment/native evidence is in artifacts/pc-pairing-20261025/.
- Installed the identical MSI payload on LAPTOP-4E1649F3 and MCTEEKUNG-PC;
  both installers returned 0 and verified unchanged configuration and startup
  tasks. Each desktop has one app process and its keyboard worker.
- After installation, MCTEEKUNG-PC initiated a real trusted reconnect to the
  laptop. Its native log records the LAN TCP route in 3 ms, followed by an
  established encrypted input channel with peer source 30f1a39488c1. The laptop
  independently reports the same peer source. Subsequent live input traffic has
  zero injection/decryption errors in the inspected sample.
- The native probe deliberately does not send a pairing request or alter trust.
  It verifies route selection. The live check above verifies trusted reconnect;
  a fresh PC-initiated request with human approval was not exercised after the
  installation. Consent/PIN regressions are covered by the daemon tests.

Do not fix this by enlarging request lifetimes, allowing arbitrary auto-pairing,
or changing the user's network adapters. Those would leave the route-order bug
in place. Keep a dead first route in the regression when changing this code.
