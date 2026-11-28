# Android performance

The Android client reuses decoded PCM storage and the first queued frame for mixing. Empty audio polls do not allocate PCM buffers. Playback keeps the existing 5 ms poll interval while frames arrive and for 500 ms afterwards, then backs off to at most 40 ms during silence. AudioTrack is created on first playback, rather than on every connection.

Native events are drained in batches of at most 64, yielding between full batches. The poll interval remains 8 ms while a peer owns input and is 24 ms otherwise. Cursor frame callbacks still follow the display; unchanged window geometry does not trigger another WindowManager update, and cursor artwork paths are rebuilt only when the shape changes.

App search loads launcher labels off the UI thread, caches the catalog until package changes, caches normalized search results, and uses ListView recycling for visible rows. All results remain available through scrolling and keyboard navigation. The installed NDK version is pinned to the version used by the native build so debug symbols are stripped from the packaged libraries; unstripped binaries remain in the native build output.

These rendering changes follow Android's [custom view optimization guidance](https://developer.android.com/develop/ui/views/layout/custom-views/optimizing-view).

## Device verification — 2026-11-27

Samsung SM-S928B, Android 17 / One UI 9, same Wi-Fi connections and phone audio capture enabled. CPU samples use process user + system ticks from `/proc/<pid>/stat`, `CLK_TCK=100`, divided by elapsed time. Each phase has three samples of approximately 8 seconds, without scripted input during sampling.

| Metric | Previous installed build | Optimized build |
| --- | ---: | ---: |
| CPU, percent of one core, median | 27.10% | 18.97% |
| CPU sample range | 26.76–27.37% | 18.77–19.24% |
| Universal debug APK, bytes | 55,982,280 | 34,242,044 |

This is approximately 30% lower process CPU in this connected capture scenario and a 39% smaller APK. It is a short device comparison, not a battery-life estimate or a measurement of mouse latency/FPS. Memory readings vary with graphics buffers and garbage collection; no memory reduction percentage is claimed.

Validation: 15 Kotlin unit tests, 8 Rust tests (including authenticated multi-device audio), Android assemble/lint with no errors, and Rust Clippy with warnings denied. On the phone, app search with arrow navigation and Enter still launches LINE; Accessibility remains bound, no AndroidRuntime fatal error was observed, phone local media volume remains zero while routed, and the laptop receives around 50 audio packets per second.

Detailed samples and the verified APK are in `artifacts/android-preview/performance-release/`.
