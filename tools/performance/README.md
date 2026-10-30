# Round A performance measurements

These tools measure microphone framing, the effective production build, and an
already paired Windows audio route. They do not change application routing or
system volume. Reports must identify the installed payload hash and source ID;
`git HEAD` alone is insufficient for a dirty working tree.

## Build and correctness

```powershell
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
python tools/performance/check-release-profile.py
```

Run `npm test` and `npm run build` in `ui`. Capture the real installer build with
`npm run tauri -- build --ci -- --locked -vv` (append `--bundles msi` before `--`
for the Windows MSI only). A fresh binary compilation is required for the log
audit; touching `ui/src-tauri/src/main.rs` suffices for a warm cache.

```powershell
python tools/performance/check-release-profile.py --log release-build.log --crate mineshare_app --symbols-dir ui/src-tauri/target/release
```

The root and Tauri directories are separate Cargo workspaces. Both use `opt-level
= 3` and packed debug information. The daemon retains ThinLTO and one codegen
unit. The GUI explicitly retains its established `lto = false` / 16 codegen-unit
policy: the ThinLTO / one-unit candidate reduced size but did not pass the native
audio quality gate in Round A. Matching PDB/DWP
files are retained as a separate CI artifact, outside distributable installers.
Linux builds and hardware validation must be reported separately from Windows.

## Controlled microphone benchmark

Build `round_a_audio` twice, with separate `CARGO_TARGET_DIR` locations. For the
old GUI policy set `CARGO_PROFILE_RELEASE_LTO=false`,
`CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`, `CARGO_PROFILE_RELEASE_DEBUG=0`, and
`CARGO_PROFILE_RELEASE_STRIP=none`. For the selected production GUI policy keep
the LTO/codegen overrides and unset the debug/strip overrides, which then inherit
the shared packed-symbol policy. Do not benchmark the root daemon's ThinLTO
profile and label that executable as the GUI policy.
Use `cargo build --release --locked -p mineshare-audio --example round_a_audio`.
Retain the environment and logs alongside each executable.

```powershell
python tools/performance/run-mic-benchmark.py --baseline before/audio-bench.exe --optimized after/audio-bench.exe --output comparison
```

The same benchmark source contains the original `round(rate*960/48000)+1`
consumption rule and the corrected production `MicPcm`. It interleaves five runs
of old framing/old policy, new framing/old policy, and new framing/selected GUI policy at
44.1/48/96 kHz, with and without Opus encoding. Each run has 500 warm-up blocks
and 5,000 measured blocks. This isolates framing from compiler policy. It is a
CPU microbenchmark, not microphone-to-speaker latency or whole-app CPU usage.
`excess_consumed_frames` includes warm-up and directly tests the sample-count
contract. Keep every run, including outliers.

`cargo run --release --locked -p mineshare-audio --example mic_capture_probe`
opens the default microphone for 250 frames, decodes them, then closes the
route. It saves no microphone audio and reports delivery intervals only.

## Native Windows pair

Python needs NumPy and SciPy on the receiver. The source helper uses only the
Python standard library. Build the recorder with
`cargo build --release --locked --manifest-path tools/performance/windows-recorder/Cargo.toml --bin mineshare-audio-measure-recorder`.
Place that executable, `windows-pair-agent.py`, and Python on the source. Use
an already verified SSH host key; the harness enforces strict verification.
Both desktops must be signed in. Enable the source's existing system-output
route and receiver playback before starting; preserve these settings across
the comparison.

```powershell
python tools/performance/windows-pair.py --host SOURCE_IP --user SOURCE_USER --identity SSH_KEY --remote-root C:/Users/SOURCE_USER/AppData/Local/Temp/MineShareRoundA --remote-python C:/Python314/python.exe --recorder recorder.exe --output native-before --runs 3 --continuous
python tools/performance/audio-quality.py native-before
```

Each 46-second recording contains a 36-second synthetic signal and 64 distinct
markers. Run three trials before and after installing the verified payload.
Do not compile, copy large packages or run CPU benchmarks during acquisition.
The source uses a temporary interactive scheduled task, removed on completion.
It never modifies `MineShareLaunch`. A failed source playback is an invalid
experiment, not a transport-quality result. Retain its files and reason, then
repeat the entire series with the corrected harness on both builds.

The observer records native WASAPI digital-output timestamps at both endpoints.
Its 200 ms capture queue is **not** subtracted from latency. SSH clock probes
before/after bound clock offset; report that uncertainty and run-to-run spread.
The recorded source is named `tee.f32` for historical tool compatibility but is
Windows in this protocol. The measurement excludes acoustic speaker/headphone
delay and is not interchangeable with an Ubuntu-source measurement.

Every marker must match before latency is aggregated. Quality checks include
40 ms windows at a 10 ms stride, gaps of at least 2 ms, clipping, correlation,
device-position discontinuities and invalid timestamps at both endpoints.
All failed markers and low-quality windows remain in the report. A passing
finite test does not establish absence of all possible audio glitches.

## Resource sample and installation

`windows-process-sample.ps1 -Output process.json -Seconds 60` captures CPU,
private committed bytes, working set and handles for MineShare and its WebView
descendants. CPU is normalized by logical processor count. Run it separately
from stimulus/benchmarks, keep visibility/routing state fixed, and allow a
warm-up after restart. Short samples are descriptive; they do not prove the
absence of long-term memory growth. Summed working sets include shared pages;
do not present them as unique physical memory.

`install-round-a.ps1` is an explicit two-machine acceptance helper, not a general
installer. Stage it with the MSI and `deployment.json` containing `MsiHash`,
`PayloadHash`, `BeforeHash`, and `SourceId`, outside the repository. Run it with
`-ExpectedComputer` on the intended Windows account, with the privileges of the
existing startup task. It backs up locally, installs the exact package and
asserts payload/configuration/startup preservation. Private backups stay on the
machine. It refuses to overwrite an existing backup.

Validate the MSI with `installer/windows/test-msi-package.ps1`, extract its
payload with WiX `dark.exe`, then use `verify-msi-payload.py PAYLOAD UNBUNDLED_EXE
SOURCE_ID REPORT_JSON`. Only Tauri's three-byte `UNK` to `MSI` bundle marker may
differ. Keep the matching source archive, compiler evidence, payload hash and
debug symbols with the report.

## Round B: both Windows directions and bounded input replay

`windows-pair.py --reverse` uses the local machine as the audio source and the
SSH peer as the output observer. It keeps the same verified SSH connection;
the peer does not need SSH access back to the source. Clock offset is applied
to the correct endpoint in either direction. Upload the matching
`windows-pair-agent.py`, which supports both `capture` and `record` modes.

Audio routing must match the chosen direction. The acceptance helper can stage
`windows-audio-route.ps1` beside `windows-processes.ps1` outside the repository.
It restarts only the known MineShare executable using its existing task,
temporarily changes the two system-audio direction flags, and restores the
original bytes when no unrelated preference has changed. It preserves other
user edits. Installation of the current all-users MSI requires Windows
administrator confirmation, even when process control is permitted to the
current account. Do not alter installation scope or startup privileges to
work around that requirement.

Run the local pair coordinator in PowerShell 7. This avoids inheriting an
incompatible module search path when launching Windows PowerShell from Core.
If resuming after a long interruption, use its `-RefreshBaseline` option and
retain the previous acquisition folders as historical evidence.

`cargo run --locked -p mineshare-daemon --example round_b_pipeline -- legacy`
and the same command ending in `bounded` replay the preserved input broadcast
and wire-coalescing contract versus the production input ingress. Compare
distance, key edges and ordering, not the one-run `wall_us` value. These tests
do not synthesize desktop input or measure physical capture-to-inject latency.

For both directions, retain all native continuous runs, separate sparse-sound
runs, and physical mouse/keyboard acceptance results. A passing model or
connection handshake does not establish that physical keyboard shortcuts
reach the intended foreground window.

## Validated Windows configuration (30 October 2026)

The final Round B measurements and rejected experiments are documented in
[the validation report](../../docs/performance-round-b-2026-10-30.md). The chosen
configuration uses event-driven Windows process-loopback capture, capture and
transport MMCSS registration, an isolated embedded Windows runtime, 192 kbps
system audio, an 80 ms PCM reserve, and a 10 ms device request when supported.
The source ID and installed payload hash must still be checked for every run.

`windows-pair.py` gives each remote acquisition a unique directory, preventing a
previous completion marker from making the coordinator copy files still being
written. Keep incomplete and failed acquisitions instead of pooling their
surviving markers as a complete latency result.

To isolate capture/codec content from network and playback, build
`cargo build --locked -p mineshare-audio --example sysout_capture_probe` and run
the resulting executable with a fresh output directory while playing synthetic
test audio from a separate process. It records system-output content for 46 s;
use synthetic audio only. Its timestamps estimate encoded-frame handoff and
must not be used to claim native endpoint or end-to-end latency.

The ignored `native_capture_buffer_capacities` audio test is an explicit Windows
hardware probe. Run it with `--ignored --nocapture` to inspect actual WASAPI
capacity rather than assuming the requested buffer duration was honored. On the
tested process-loopback implementation, both the minimum and 100 ms requests
returned 480 frames. Ordinary automated tests do not execute this probe.
