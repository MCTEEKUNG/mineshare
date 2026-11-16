//! Bounded supervision for the per-user Windows login launch only.
use std::ffi::OsStr;
use std::io;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::Duration;

const FLAG: &str = "--mineshare-autostart";
const RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_secs(3);

pub fn run_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !args.iter().any(|arg| arg == OsStr::new(FLAG)) {
        return None;
    }
    let _ = mineshare_daemon::logs::init();
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!(%error, "login launcher cannot locate MineShare");
            return Some(1);
        }
    };
    let child_args: Vec<_> = args
        .into_iter()
        .filter(|arg| arg != OsStr::new(FLAG))
        .collect();
    Some(supervise(
        || {
            Command::new(&executable)
                .args(&child_args)
                .creation_flags(0x08000000)
                .status()
                .map(|status| status.code().unwrap_or(1))
        },
        std::thread::sleep,
    ))
}

fn supervise(mut launch: impl FnMut() -> io::Result<i32>, mut wait: impl FnMut(Duration)) -> i32 {
    for attempt in 0..=RETRIES {
        tracing::info!(attempt = attempt + 1, "login launcher starting MineShare");
        let code = match launch() {
            Ok(0) => return 0, // Normal Quit and a duplicate launch must stay stopped.
            Ok(code) => code,
            Err(error) => {
                tracing::warn!(%error, "login launcher could not start MineShare");
                1
            }
        };
        if attempt == RETRIES {
            tracing::error!(code, "login launcher exhausted recovery attempts");
            return code;
        }
        tracing::warn!(
            code,
            "MineShare exited abnormally; retrying login launch in 3 seconds"
        );
        wait(RETRY_DELAY);
    }
    unreachable!("the bounded launch loop always returns")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_exit_never_restarts() {
        let mut attempts = 0;
        let code = supervise(
            || {
                attempts += 1;
                Ok(0)
            },
            |_| panic!("normal Quit must not be retried"),
        );
        assert_eq!((code, attempts), (0, 1));
    }

    #[test]
    fn persistent_failure_stops_after_three_retries() {
        let mut attempts = 0;
        let mut delays = Vec::new();
        let code = supervise(
            || {
                attempts += 1;
                Ok(101)
            },
            |delay| delays.push(delay),
        );
        assert_eq!((code, attempts), (101, 4));
        assert_eq!(delays, vec![Duration::from_secs(3); 3]);
    }

    #[test]
    fn spawn_failure_can_recover_without_unbounded_retries() {
        let mut attempts = 0;
        let code = supervise(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    Ok(0)
                }
            },
            |_| {},
        );
        assert_eq!((code, attempts), (0, 2));
    }

    #[test]
    fn real_child_exit_101_is_retried_then_zero_stops() {
        let path = std::env::temp_dir().join(format!(
            "mineshare-autostart-test-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script = "$ErrorActionPreference='Stop'; $p=$env:MINESHARE_AUTOSTART_TEST_PATH; if(-not (Test-Path -LiteralPath $p)){Set-Content -LiteralPath $p -Value 1; exit 101}; Set-Content -LiteralPath $p -Value 2; exit 0";
        let mut attempts = 0;
        let started = std::time::Instant::now();
        let code = supervise(
            || {
                attempts += 1;
                Command::new("powershell.exe")
                    .args(["-NoProfile", "-NonInteractive", "-Command", script])
                    .env("MINESHARE_AUTOSTART_TEST_PATH", &path)
                    .creation_flags(0x08000000)
                    .status()
                    .map(|status| status.code().unwrap_or(1))
            },
            std::thread::sleep,
        );
        assert_eq!((code, attempts), (0, 2));
        assert!(started.elapsed() >= RETRY_DELAY);
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "2");
        std::fs::remove_file(path).unwrap();
    }
}
