//! Realtime scheduling for the threads that forward and inject input.
//!
//! On a busy Linux desktop (a game or launcher using several cores) a normal
//! (`SCHED_OTHER`) thread that was just woken by an input event can wait
//! 1.5-3 ms for a CPU: measured on the live Ubuntu machine, p99 wake latency
//! of the sender's queue was ~1.9 ms and p99.9 ~2.6 ms, versus ~50 us and
//! ~60 us under `SCHED_FIFO`. Raising `nice` did not help. Windows already
//! has the equivalent (MMCSS runtime and `THREAD_PRIORITY_HIGHEST`).
//!
//! Unprivileged processes get `SCHED_RR` through RealtimeKit (rtkit), the same
//! service PipeWire and browsers use. Everything here is best effort: any
//! failure leaves the thread on ordinary scheduling.
//!
//! Safety net: a realtime thread that never blocks would freeze the machine,
//! so rtkit requires `RLIMIT_RTTIME`. Exceeding it normally kills the whole
//! process with `SIGXCPU`, so a handler demotes the offending thread back to
//! `SCHED_OTHER` instead.

/// Ask for realtime scheduling for the calling thread. Returns whether it was
/// granted. `MINESHARE_DISABLE_REALTIME=1` turns this off.
pub fn promote_current_thread_realtime(role: &'static str) -> bool {
    #[cfg(target_os = "linux")]
    {
        imp::promote(role)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = role;
        false
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use nix::libc;
    use std::sync::{Once, OnceLock};
    use tracing::{debug, info};

    /// Below PipeWire's data loop (20) so audio keeps precedence.
    const RTKIT_PRIORITY: u32 = 10;
    /// rtkit refuses a hard limit above its own `RTTimeUSecMax` (200 ms).
    const RTTIME_SOFT_US: libc::rlim_t = 100_000;
    const RTTIME_HARD_US: libc::rlim_t = 200_000;
    const RATE_LIMIT_RETRIES: usize = 6;
    const RATE_LIMIT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(300);

    extern "C" fn demote_on_sigxcpu(_signal: libc::c_int) {
        // Async-signal-safe: one syscall, delivered to the offending thread.
        // rtkit sets SCHED_RESET_ON_FORK, and an unprivileged thread may not
        // clear that flag (EPERM), so the demotion must keep it.
        let param = libc::sched_param { sched_priority: 0 };
        unsafe {
            libc::sched_setscheduler(0, libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK, &param)
        };
    }

    fn install_runaway_guard() -> bool {
        static ONCE: Once = Once::new();
        static OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        ONCE.call_once(|| unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = demote_on_sigxcpu as extern "C" fn(libc::c_int) as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(libc::SIGXCPU, &action, std::ptr::null_mut()) != 0 {
                return;
            }
            let limit = libc::rlimit {
                rlim_cur: RTTIME_SOFT_US,
                rlim_max: RTTIME_HARD_US,
            };
            if libc::setrlimit(libc::RLIMIT_RTTIME, &limit) == 0 {
                OK.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        OK.load(std::sync::atomic::Ordering::Acquire)
    }

    fn system_bus() -> Option<&'static zbus::blocking::Connection> {
        static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
        BUS.get_or_init(|| zbus::blocking::Connection::system().ok())
            .as_ref()
    }

    fn request(bus: &zbus::blocking::Connection, tid: u64) -> zbus::Result<zbus::Message> {
        zbus::blocking::Proxy::new(
            bus,
            "org.freedesktop.RealtimeKit1",
            "/org/freedesktop/RealtimeKit1",
            "org.freedesktop.RealtimeKit1",
        )
        .and_then(|proxy| {
            proxy.call_method(
                "MakeThreadRealtimeWithPID",
                &(u64::from(std::process::id()), tid, RTKIT_PRIORITY),
            )
        })
    }

    pub(super) fn promote(role: &'static str) -> bool {
        if std::env::var_os("MINESHARE_DISABLE_REALTIME").is_some() {
            return false;
        }
        if !install_runaway_guard() {
            debug!(
                role,
                "realtime scheduling unavailable: no RLIMIT_RTTIME guard"
            );
            return false;
        }
        let Some(bus) = system_bus() else {
            debug!(role, "realtime scheduling unavailable: no system bus");
            return false;
        };
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u64;
        // rtkit rate-limits requests (a burst of about five per second per
        // user). Startup asks for several threads at once, so wait out a
        // limit instead of silently leaving a latency-critical thread normal.
        let mut result = request(bus, tid);
        for _ in 0..RATE_LIMIT_RETRIES {
            match &result {
                Err(zbus::Error::MethodError(name, ..))
                    if name.as_str().ends_with("LimitsExceeded") =>
                {
                    std::thread::sleep(RATE_LIMIT_BACKOFF);
                    result = request(bus, tid);
                }
                _ => break,
            }
        }
        match result {
            Ok(_) => {
                info!(
                    role,
                    tid,
                    priority = RTKIT_PRIORITY,
                    "input thread is realtime"
                );
                true
            }
            Err(error) => {
                debug!(role, tid, %error, "rtkit refused realtime scheduling");
                false
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::promote_current_thread_realtime;
    use nix::libc;

    fn policy() -> i32 {
        // rtkit adds SCHED_RESET_ON_FORK; compare the policy bits only.
        unsafe { libc::sched_getscheduler(0) & !libc::SCHED_RESET_ON_FORK }
    }

    /// Manual: needs an rtkit-capable session, so run it as a user service:
    /// `systemd-run --user --pipe --wait <test binary> --ignored --nocapture
    /// runaway_realtime_thread_is_demoted_not_killed`.
    #[test]
    #[ignore = "needs rtkit from a graphical-session user; prints, asserts demotion"]
    fn runaway_realtime_thread_is_demoted_not_killed() {
        std::thread::spawn(|| {
            let granted = promote_current_thread_realtime("test-runaway");
            println!("granted={granted} policy_before={}", policy());
            if !granted {
                return None;
            }
            assert_eq!(policy(), libc::SCHED_RR);
            // Never block: the kernel signals SIGXCPU at 100 ms, which must
            // demote this thread instead of terminating the process.
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(600) {
                std::hint::spin_loop();
            }
            Some(policy())
        })
        .join()
        .map(|after| {
            println!("survived, policy_after={after:?}");
            if let Some(after) = after {
                assert_eq!(after, libc::SCHED_OTHER, "runaway thread stayed realtime");
            }
        })
        .unwrap();
    }

    /// Manual, same launch as above: eight threads ask at once; all must be
    /// granted even though rtkit rate-limits bursts.
    #[test]
    #[ignore = "needs rtkit from a graphical-session user; prints, asserts all granted"]
    fn a_burst_of_promotions_is_not_dropped_by_the_rtkit_rate_limit() {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    let granted = promote_current_thread_realtime("test-burst");
                    // Stay alive but blocked so rtkit still counts the thread.
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    granted
                })
            })
            .collect();
        let granted = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|g| *g)
            .count();
        println!("granted {granted}/8");
        assert_eq!(granted, 8);
    }
}
