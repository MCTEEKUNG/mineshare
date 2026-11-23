# Linux realtime scheduling for input threads

Since the build that follows 0.0.25, MineShare asks the system for realtime
scheduling for the threads that move mouse input, so a busy desktop (a game,
a launcher, a build) cannot delay the cursor.

## Why

On the Ubuntu machine used for testing, the sender's input queue showed a wake
latency tail while another program used the CPU (`hub_wake_latency_bench`, 3 x
20,000 events at 1 kHz):

| Scheduling | p50 | p99 | p99.9 |
|---|---:|---:|---:|
| normal (`SCHED_OTHER`) | 39 us | 0.95-1.4 ms | 2.3-2.5 ms |
| `SCHED_RR` through rtkit (what the app now does) | 35 us | 52-61 us | 62-100 us |

Lowering `nice` did not help; keeping CPUs out of deep C-states did not help the
tail either. Windows already had the equivalent (a dedicated MMCSS runtime and
`THREAD_PRIORITY_HIGHEST`).

## What is promoted

Through RealtimeKit (`org.freedesktop.RealtimeKit1`, priority 10, below
PipeWire's 20):

- the two tokio workers of the dedicated transport runtime (`ui/src-tauri/src/state.rs`);
- the input injection worker;
- the evdev flush watchdog and the pointer devices' evdev pump threads.

Keyboards, tokio's blocking pool, and everything else stay on normal scheduling.

## Safety

A realtime thread that never blocks can freeze a machine, so the process sets
`RLIMIT_RTTIME` to 100 ms soft / 200 ms hard (rtkit's maximum). Exceeding it
would normally kill the process with `SIGXCPU`; a handler instead puts the
offending thread back on `SCHED_OTHER` (keeping `SCHED_RESET_ON_FORK`, which an
unprivileged thread cannot clear) and re-arms the soft limit, because the kernel
raises it by one second after each signal and a second runaway thread would
otherwise be SIGKILLed at the hard limit (two permanently saturated transport
workers killed the process before this was fixed). If rtkit is missing, refuses (it only serves
the active graphical session and user services, not an SSH login), or rate-limits
the request, the thread simply keeps normal scheduling.

Set `MINESHARE_DISABLE_REALTIME=1` in the environment to turn this off.

## Manual checks

Run inside a user service so rtkit's polkit rule applies:

```sh
systemd-run --user --pipe --wait <test-binary> --ignored --nocapture runaway_realtime_thread_is_demoted_not_killed
systemd-run --user --pipe --wait <test-binary> --ignored --nocapture a_burst_of_promotions_is_not_dropped_by_the_rtkit_rate_limit
systemd-run --user --pipe --wait --setenv=HUB_RT=1 <daemon-test-binary> --ignored --nocapture hub_wake_latency_bench
```

`journalctl -u rtkit-daemon` lists each grant.
