# Round B: Windows performance validation

Validated on 30 October 2026 using the paired Windows laptop and desktop. Both
machines now run source `c59b6e79ff7a`, version `0.0.16`, protocol `control-v3`.
This is a development build; it does not imply a new published release.

## Observed audio results

Each continuous measurement contains three runs and 192 unique timing markers.
The baseline is the installed Round A source `6ccb459824d1`, measured again before
installation. Direction names describe the physical sound source and receiver.

| Route | Median before | Median after | p95 before | p95 after |
|---|---:|---:|---:|---:|
| Desktop → laptop | 190.29 ms | 180.81 ms | 191.49 ms | 182.19 ms |
| Laptop → desktop | 125.16 ms | 120.32 ms | 136.68 ms | 124.63 ms |

The final build matched **384/384 continuous markers**. Across 18,900 overlapping
40 ms analysis windows, it had zero observed silence gaps of at least 2 ms, zero
correlation windows below .75, zero clipping, and zero capture errors. The reverse
baseline contained one 60 ms gap and five low-correlation windows; the other
baseline route had none. Worst final p05 waveform correlation was approximately
.975, below the roadmap's aspirational .99 target, and similar to the baseline.

Both sparse-sound tests matched **64/64 markers**. Desktop → laptop median/p95
was 183.93/193.49 ms; laptop → desktop was 122.41/132.45 ms. These establish
sparse-sound delivery in this sample. A fresh sparse baseline was not collected,
so they are not presented as a sparse latency improvement.

The observed continuous median reductions were 9.47 ms and 4.84 ms; p95 reductions
were 9.30 ms and 12.05 ms. Endpoint loopback/QPC timestamps and clock calibration
before and after each run were used, rather than half the network RTT. Clock
uncertainty was around 1–2 ms. Three runs on one pair of machines do not establish
statistical significance or a guarantee on other networks. The measurements do
not include headphone/acoustic latency, video synchronization, or Ubuntu.

## Input correctness and physical acceptance

The deterministic replay compares the former broadcast-64/coalescing contract
with the production bounded input pipeline. It checks correctness under a burst,
not physical latency or CPU throughput.

| Replay case | Before | After | Expected |
|---|---:|---:|---:|
| Distance from 1,000 moves of 10 px | 640 px | 10,000 px | 10,000 px |
| Short key tap inside that burst | 0 edges | 2 edges | 2 edges |
| Four individually legal 150 px moves | 200 px | 600 px | 600 px |

On the installed final build, the user confirmed that the full requested physical
matrix passed in both directions and felt smoother: source mouse crossing and
source keyboard, destination-mouse focus followed by source keyboard, Smart and
Pin-to-peer modes, rapid typing, window dragging, Alt+Tab, Win+Shift+S, crossing
back while holding/releasing modifiers, and repetition after about one minute
idle. No stuck keys or commands reaching the wrong machine were reported.
This is human acceptance, not a measured capture-to-inject p95/p99 benchmark.

## Final implementation

- Input queues coalesce eligible adjacent motion before overflow, preserve
  discrete boundaries and the existing 200 px injection safety limit, and fail
  closed on overload/expiry so session teardown can release held input.
- Per-peer audio queues retain three frames per stream, enforce a 60 ms encoded
  handoff age, and service audio after at most eight input sends or 2 ms of work.
  Capture queues and playback ingress also have explicit bounds.
- Windows process-loopback capture and realtime transport workers use the
  existing thread-affine MMCSS registration. The embedded Windows daemon has
  its own two-worker Tokio runtime, independent of Tauri command work.
- Process-loopback capture waits for WASAPI readiness events instead of sleeping
  every 5 ms. A native probe measured only 480 frames of capture capacity for
  both the minimum request and a 100 ms request on this implementation. Increasing
  the requested capacity alone therefore did not solve capture overruns.
- Windows system sound uses 192 kbps Opus. Playback keeps the tested 80 ms PCM
  reserve and 100 ms catch-up boundary, with a 10 ms device request where
  supported. Sparse sounds retain their bounded startup deadline.
- Queue age, capture health, underruns and executor timer lateness are reported
  outside audio/input callbacks. No physical key-content logging was added.

The readiness-event and capture-buffer work follows the
[Microsoft IAudioClient documentation](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-initialize).
The measured native capacity, rather than an assumed requested size, guided the
implementation. Encoded handoff timestamps do not claim to be hardware capture
timestamps. The serialized audio frame and control protocol remain unchanged.

## Rejected experiments and limitations

Earlier experiments were retained, including failures, and automatically rolled
back to Round A. An incomplete refreshed baseline (191/192) was retained and a
new full baseline collected. It was not pooled as a complete latency sample.

| Experiment | Reason it was not accepted |
|---|---|
| 60 ms reserve / 80 ms catch-up | 191/192 markers; frequent PCM catch-up |
| 60 ms reserve / 100 ms catch-up | One 31.6 ms gap and worse p95 |
| 80 ms reserve / 10 ms device request | Reverse route 182/192 markers |
| Added queue-health diagnostics | Reverse route 190/192 markers |
| Added capture MMCSS | No silence gaps, but 21 low-correlation windows |
| Added 192 kbps system sound | Continuous reverse passed; sparse 63/64 |
| Separate transport runtime | One 7.85 ms gap; native capture boundary investigated |
| Event-driven capture (final) | Continuous and sparse gates passed both ways |

The final observation is finite: it does not promise lossless audio through long
OS/network outages. The input protocol still has no ACK/retransmit mechanism for
a complete UDP key press/release lost between state snapshots. This work fixes
local queue loss and scheduling; it does not claim to remove that network limit.

## Verification and installation

- Rust workspace: **276 passed, 0 failed, 15 ignored**. Ignored hardware/manual
  probes are separate from the automated total. The native capacity probe was
  explicitly run and passed.
- Workspace Clippy with warnings denied, GUI Clippy for the runtime change,
  formatting, production MSI build, effective release-profile checks, matching
  symbols, and MSI upgrade/migration checks passed.
- Both installed payload hashes are
  `EC7E6EA34AA2AB394BBFF48A66D11B6834967AFEAB6EBAC0CAFD52F6AC5AD3A1`.
  MSI SHA-256 is
  `E7E815BB3FE408555D3A74C2815264D284269D66A3D59F2F9805394E4581B59A`.
- Installation receipts verified unchanged private configuration and startup.
  Final checks found the original settings bytes, the original startup privilege
  level, three expected application processes per machine, and no probe tasks.

Local source archives, matching PDBs, installer receipts, all run data and failed
experiments are retained under `artifacts/round-b-20261027`. Raw recordings,
private configuration backups and installers are excluded from this source push.
See [measurement tooling](../tools/performance/README.md) for reproduction.
