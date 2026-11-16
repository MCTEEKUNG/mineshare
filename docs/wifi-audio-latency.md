# Audio catch-up after Wi-Fi delivery bursts

The laptop moved from Ethernet to Wi-Fi while Tee remained wired. The reported
symptom was intermittent delayed video audio that later caught up, rather than
a constant offset. The route still used the selected trusted peer; laptop
traffic to Tee used Wi-Fi (192.168.1.103 -> 192.168.1.108).

The playback ring had capacity for 200 ms of decoded PCM and only rejected
incoming frames when full. It never advanced past old queued audio. A deterministic
regression at the actual `push_samples` -> `fill_callback` seam queued ten 20 ms
frames. Before the fix, the first rendered frame was 180 ms behind the newest
received frame. The regression failed before changing the callback.

The device callback now skips the oldest stereo-aligned PCM when the ring exceeds
100 ms, retaining an 80 ms reserve (or a complete device callback if larger).
Only the SPSC consumer moves the read cursor. The catch-up path introduces no
allocation, blocking synchronization or logging on the device thread. The
existing five-second worker health report records aggregate skipped samples.

Loss concealment now covers at most two missing packets while buffered sound
remains and leaves space below the high-water limit for the real packet. An
empty ring or a longer sequence gap does not replay up to 100 ms of synthetic
sound after the outage has already elapsed. Single-frame microphone FEC remains
available when continuity can be preserved.

Keep the proven 80 ms startup reserve and eight-frame ingress queue. The
40 ms device period was the original safe baseline; the Windows-only refinement
below uses verified audio scheduling before requesting a shorter period.
Earlier 40 ms startup reserve/four-frame ingress changes caused
crackle during observed 50–160 ms Windows scheduling stalls. This change bounds
decoded catch-up backlog rather than trading those protections away. Audio
capture, pairing, control routing, mouse/keyboard paths and saved audio choices
are unchanged.

Validation: Windows audio tests 73 passed (two hardware probes ignored); Linux
audio tests 51 passed in the prepared Docker builder; audio Clippy on both
platforms and workspace formatting passed. The same ten-frame burst now starts
60 ms behind the newest received frame. Stereo alignment and the existing
60 ms delivery-gap reserve regression pass.

These figures measure queued PCM age at the callback relative to the latest
received frame, not end-to-end speaker latency or video lip sync. Wi-Fi transit,
capture and the output device add their own delay. The installed pair is also measured automatically through the real digital
audio path; see the benchmark below. Video presentation and the physical
headphone transducer still require a separate instrumented AV/acoustic test.

Build and installation evidence is kept in `artifacts/wifi-audio/release/`.

## Automated native measurement

The user requested automated measurement and graphs rather than manual timing.
The benchmark in `artifacts/wifi-audio/measurement/` records Tee's actual output
monitor and the laptop's WASAPI output loopback, calibrates clocks before/after
each run, and matches known audio markers. Initial precise-clock native results
on source `7ca283a00cce` were median 171.1 ms, p95 206.8 ms and max 208.3 ms
(188/192 markers; four source markers were insufficiently clear). These revealed
a remaining 120 ms decoded-queue plateau. The high-water threshold was reduced
to 100 ms while keeping the 80 ms reserve. A new six-frame/40 ms callback test
failed at 100 ms sample age before this refinement, then passed at 60 ms;
a steady-cadence regression verifies that normal callbacks do not skip audio.
The final installed revision and repeated native results are recorded in the
measurement report. The original pre-fix executable was not measured with this
native method; do not label the first native series as that original version.

## Sparse audio deadline discovered by the native benchmark

With no unrelated media playing, the same marker track exposed a second defect:
short 40 ms sounds could remain below the 80 ms prebuffer target indefinitely,
then play when the next sound arrived. On `14d62c029e20`, all 192 markers matched,
but the measured p95 was 611.4 ms (max 617.7 ms). The earlier background-media
series did not expose this sparse-audio wait. This is why a lower queue capacity
alone was insufficient.

The callback now holds a per-stream PlayoutState with a monotonic buffering
start time. It starts once either the normal 80 ms sample reserve is present or
80 ms of wall time has elapsed with a nonempty ring. Empty rings, underruns and
stale sessions reset that deadline. A real callback regression failed after a
90 ms wait before the fix; the final test uses an injected Instant and verifies
release at 80 ms without sleeping. Another test prevents a stale deadline from
being inherited after silence. These changes introduce no callback allocation
or blocking lock and preserve the continuous-audio reserve and session guard.

Final source `8f0afb232018` is compared against `14d62c029e20` in the native
measurement report. Earlier first-pass series remain available separately.

Final native results: median 186.8 ms, p95 206.3 ms,
max 217.2 ms and standard deviation 13.6 ms, with 192/192
matched markers. The comparison series was median 172.9 ms, p95
611.4 ms and max 617.7 ms. The long sparse-audio waits
are reduced; median latency did not improve. Do not equate control RTT with
this end-to-end digital-audio measurement.

## ## Windows output scheduling and endpoint probing

The current deployed source is `73f4e9520e57` (0.0.16 / control-v3) on both
Windows and Tee. CPAL 0.15.3's Windows priority helper casts a thread ID to a
HANDLE; a native probe reproduced ERROR_INVALID_HANDLE (6). MineShare registers
its actual decoder/callback threads with MMCSS via thread-local guards, released
on the owning thread. It prefers Pro Audio + HIGH, with an Audio fallback and
ordinary scheduling if registration fails. This is confined to MineShare threads.

With scheduling available, Windows requests 20 ms rather than 40 ms; failure
keeps 40 ms. This headset allocates 22 ms for either 20 or 10 ms requests, so
requesting 10 ms has no further device-buffer benefit. Linux output remains
40 ms; capture and packets remain 20 ms, with the existing 80 ms jitter reserve,
eight-frame ingress and sparse-audio deadline. A 30 ms Windows trial and 10 ms
Ubuntu capture trial still had gaps and were not retained as final policy.

Default-device probing runs on a bounded background worker with a separate COM
apartment. Decoder request/poll/shutdown do not wait for an OS endpoint query.
Regression tests deliberately block the query to verify this behavior. Native
logs can report slow probes; do not assume an observed gap was caused by a query
without a corresponding trace.

## Quiet system sounds

Speech DTX could learn loud background noise and then suppress quiet system
sounds. An earlier native series delivered 138/192 markers despite a complete
source; offline replay reproduced it without a network. The encoder now takes
StreamKind: Mic retains DTX/FEC; SysOut disables speech DTX. A deterministic
warm-up/noise + quiet-burst regression improves from 13/64 to 64/64. Digital
silence still settles to <=3 bytes and remains transport-suppressed. Do not
re-enable speech DTX for SysOut just to reduce idle traffic.

## Measurement and current result

The selected native continuous series is `priority-continuous/`: 192/192 timing
markers over three runs, median 155.54 ms versus the matched baseline's 177.73 ms,
p95 161.90 versus 187.05 ms. It has zero observed >=2 ms near-silent gaps,
low-correlation windows, clipping and active recorder errors across 9,450
overlapping waveform windows. The unchanged exploratory 155 ms median budget
is narrowly unmet; do not describe the whole evaluation gate as green.

The user's earlier 186.8 ms baseline uses sparse markers and must be compared
only to `priority-sparse/`, with the final values/status in
`artifacts/audio-base-latency/report.md`. Do not mix continuous and sparse
baselines. Earlier failed series are retained, including ready-sparse/ with
174/192 matches. Never report surviving-only medians as a successful result.

The Windows observer has MMCSS, buffered writes and a 200 ms capture queue.
This is separate from MineShare's render queue. Timing still uses endpoint
GetBuffer QPC timestamps, without an arbitrary 200 ms subtraction. Endpoint
ticks late by >=5 ms are reported separately and never used to erase waveform
gaps. A paired-flow trace showed arrival/endpoint timing delays near an event;
this does not prove a specific Wi-Fi driver or power mechanism.

The user explicitly requested application-only work. Adapter, driver, power,
registry and network settings were not changed; the prepared Wi-Fi power test
was declined and must not be run. Measurements are digital output checks, not
physical headset/video latency or a guarantee under every load.

Windows audio tests: 78 passed, 2 manual probes ignored; Linux: 56 passed.
Clippy, formatting, package checks and source/payload verification passed.
Installed/configuration/startup and Tee input evidence is in release-priority/.
