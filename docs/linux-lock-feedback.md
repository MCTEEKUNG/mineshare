# Linux Game Lock feedback

The prior implementation guarded both lock-change feedback and preview with
Windows-only cfgs. Linux lock routing worked, but preview was a no-op. An Xvfb
integration test reproduced the absence of native feedback windows before the
fix. The renderer now runs on a separate worker on Linux and draws short-lived
one shaped window per RandR monitor through X11/XWayland. Border segments are
painted into an offscreen pixmap and copied in four narrow regions. Empty SHAPE input regions,
override-redirect notification windows, and no focus/input calls preserve click
and keyboard ownership. Flow, pulse, fade, custom RGB, rainbow, duration and
thickness use the saved LockEffect configuration. A newer event replaces the
in-flight effect; each dedicated connection cleans up windows even on failure.

The test verifies actual custom-color pixels at an unoccluded strip center,
empty input regions, unchanged focus, unchanged lock state during preview, and
window cleanup. It passes in Xvfb and Tee's real desktop XWayland session.
Sampling the corner of overlapping strips initially returned undefined pixels;
the test was corrected to sample the center rather than weaken pixel checks.
Native compositor rendering still requires X11/XWayland DISPLAY; headless or
Wayland-only sessions without XWayland log an unavailable warning. The app's
GNOME pointer confirmation extension remains read-only and unchanged.

Home and Settings display editors now show persistent thin lock rings and a
lock glyph on the locked computer. The desktop animation is transient; the
workspace ring lasts until unlock. Local effect comes from get_status. Optional
lock_visual metadata on existing encrypted Ping heartbeats carries the peer's
own color/configuration and lock state even when the mouse has moved away.
It never calls the lock router. Legacy heartbeat decoding and old-peer acceptance
of extra fields are tested; disconnect clears visual metadata. Older peers fall
back to their existing control lock snapshot and local display preferences.
Rainbow rings honor reduced-motion. Peer lock descriptions remain accessible
without changing drag handles, arrow keys, cancellation or save rollback.

Verification: frontend 31 tests; Windows daemon 65 tests; Linux input 48 tests
passed with three hardware/private-bus checks skipped; Linux daemon 63 tests;
Xvfb and Tee native rendering integration pass. Input Clippy on Windows and Linux
passes with warnings denied. Linux workspace check passes. A broader Windows
workspace Clippy run is blocked by pre-existing collapsible_if in runtime.rs
and nonminimal_bool in session.rs, outside this change. Deployment and current
build identity are recorded separately under artifacts/linux-lock-feedback/.


## Performance correction

The first renderer created 64 compositor surfaces per monitor and configured,
restacked, repainted and changed opacity on each surface every frame. The user
reported that the whole desktop/input lagged only during the animation. The
original correctness tests passed, but did not exercise compositor responsiveness.
A live Tee benchmark reproduced X11 query p95 952–1225 ms during effects versus
about 0.1 ms median idle. A new surface-budget assertion fails the old renderer
(64 surfaces for one monitor). This was a real regression introduced by the
initial Linux renderer, not the input-routing implementation.

The corrected renderer has one stable window and an offscreen buffer per
monitor, groups rainbow segments into drawing operations, shapes only when
geometry changes, avoids repainting static colors, stacks once and paces frames
against a deadline. It retains empty input regions and the shaped center hole.
Native Tee tests of the corrected rainbow renderer report p50 0.097 ms,
p95 0.122 ms, max 0.243 ms for synchronous X11 queries. These are desktop X-server
responsiveness measurements, not network RTT or end-to-end physical input delay.
The final installed-app comparison is in artifacts/linux-lock-performance/.


User acceptance on 2026-10-21: on installed source f5a6aec5a28a, the user
physically pressed Ctrl+Shift+L and confirmed that the animation is much smoother
and the mouse/keyboard no longer lag while it runs.
