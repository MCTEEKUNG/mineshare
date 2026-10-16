//! Keyboard hooks live outside the foreground WebView host process. The GUI
//! keeps the sole routing/held-key state; the child asks it whether to consume
//! each physical edge over private inherited pipes (no network or disk IPC).
use super::*;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Pipes::PeekNamedPipe;
use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;

const ARG: &str = "--mineshare-keyboard-worker";
const MAGIC: [u8; 4] = *b"MSK1";
static ENABLED: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);

pub fn enable_gui_keyboard_worker() {
    ENABLED.store(true, Ordering::Release);
}

pub(super) fn capture_or_fallback() {
    if ENABLED.load(Ordering::Acquire) {
        supervise(std::iter::repeat(()), capture, recover_capture, || {
            thread::sleep(std::time::Duration::from_millis(250))
        });
        return;
    }
    unsafe { keyboard_hook_thread() };
}

fn supervise(
    attempts: impl Iterator<Item = ()>,
    mut start: impl FnMut() -> Result<()>,
    mut recover: impl FnMut(),
    mut pause: impl FnMut(),
) {
    for () in attempts {
        let started = Instant::now();
        let result = start();
        warn!(error=?result.err(), uptime_ms=started.elapsed().as_millis() as u64,
            "isolated keyboard capture interrupted; restoring worker, not reverting to WebView host hook");
        recover();
        pause();
    }
}

fn recover_capture() {
    force_exit_remote();
    // An interrupted pipe may lose the final hardware up. Release only keys
    // whose down we actually forwarded; leave mouse ownership alone.
    for code in 0..1024u16 {
        if super::super::route_keystroke(code, false, false) {
            sink_send(InputEvent::Key {
                code: KeyCode(code),
                down: false,
            });
        }
    }
    reset_forwarded_keyboard_latches();
    for modifier in [
        &MOD_CTRL,
        &MOD_ALT,
        &MOD_SHIFT_LEFT,
        &MOD_SHIFT_RIGHT,
        &LOCK_HOTKEY_HELD,
    ] {
        modifier.store(false, Ordering::Release);
    }
}

fn capture() -> Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg(ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()?;
    let result = (|| -> Result<()> {
        let mut input = child.stdout.take().context("keyboard worker stdout")?;
        let mut output = child.stdin.take().context("keyboard worker stdin")?;
        let mut header = [0u8; 8];
        input.read_exact(&mut header)?;
        anyhow::ensure!(header[..4] == MAGIC, "invalid keyboard worker handshake");
        KEYBOARD_HOOK_THREAD_ID.store(
            u32::from_le_bytes(header[4..].try_into()?),
            Ordering::Release,
        );
        info!(
            pid = child.id(),
            "Windows keyboard hook ready in WebView-independent worker"
        );
        loop {
            let mut bytes = [0u8; 16];
            input.read_exact(&mut bytes)?;
            let [message, scan, vk, flags] = decode(bytes);
            let event = KBDLLHOOKSTRUCT {
                vkCode: vk,
                scanCode: scan,
                flags: windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT_FLAGS(flags),
                ..Default::default()
            };
            let routed = unsafe {
                process_keyboard_hook(
                    HC_ACTION as i32,
                    WPARAM(message as usize),
                    LPARAM((&event as *const KBDLLHOOKSTRUCT) as isize),
                )
            }
            .0 != 0;
            output.write_all(&[u8::from(routed)])?;
            output.flush()?;
        }
    })();
    KEYBOARD_HOOK_THREAD_ID.store(0, Ordering::Release);
    let exited = child.try_wait().ok().flatten();
    if exited.is_none() {
        let _ = child.kill();
    }
    let status = child.wait().ok();
    let mut diagnostic = String::new();
    if let Some(stderr) = child.stderr.take() {
        let _ = stderr.take(4096).read_to_string(&mut diagnostic);
    }
    result.with_context(|| format!("keyboard worker exit={status:?}; {}", diagnostic.trim()))
}

fn encode(fields: [u32; 4]) -> [u8; 16] {
    let mut bytes = [0; 16];
    for (field, slot) in fields.into_iter().zip(bytes.chunks_exact_mut(4)) {
        slot.copy_from_slice(&field.to_le_bytes());
    }
    bytes
}

fn decode(bytes: [u8; 16]) -> [u32; 4] {
    std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
}

/// Executed before Tauri/single-instance/logging initialization in the child.
/// Returning None means normal GUI startup, not worker mode.
pub fn run_keyboard_worker_if_requested() -> Option<i32> {
    if std::env::args_os().skip(1).collect::<Vec<_>>() != [std::ffi::OsString::from(ARG)] {
        return None;
    }
    Some(match worker_main() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("keyboard worker stopped: {error:#}");
            1
        }
    })
}

fn worker_main() -> Result<()> {
    let mut hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(worker_hook), None, 0) }?;
    let thread_id = unsafe { GetCurrentThreadId() };
    // Exit even when idle if the owning GUI closes its pipe.
    thread::spawn(move || {
        loop {
            thread::sleep(std::time::Duration::from_millis(250));
            if !pipe_available() {
                let _ = unsafe {
                    PostThreadMessageW(
                        thread_id,
                        windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
                break;
            }
        }
    });
    let mut output = std::io::stdout();
    output.write_all(&MAGIC)?;
    output.write_all(&thread_id.to_le_bytes())?;
    output.flush()?;
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_REFRESH_KEYBOARD_HOOK {
            if let Ok(replacement) =
                unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(worker_hook), None, 0) }
            {
                let old = std::mem::replace(&mut hook, replacement);
                let _ = unsafe { UnhookWindowsHookEx(old) };
            }
        } else {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    let _ = unsafe { UnhookWindowsHookEx(hook) };
    anyhow::ensure!(
        !FAILED.load(Ordering::Acquire),
        "keyboard worker IPC failed"
    );
    Ok(())
}

fn pipe_available() -> bool {
    unsafe {
        PeekNamedPipe(
            HANDLE(std::io::stdin().as_raw_handle()),
            None,
            0,
            None,
            None,
            None,
        )
    }
    .is_ok()
}

fn exchange(bytes: [u8; 16]) -> std::io::Result<bool> {
    let mut output = std::io::stdout().lock();
    output.write_all(&bytes)?;
    output.flush()?;
    let started = Instant::now();
    loop {
        let mut available = 0;
        unsafe {
            PeekNamedPipe(
                HANDLE(std::io::stdin().as_raw_handle()),
                None,
                0,
                None,
                Some(&mut available),
                None,
            )
        }
        .map_err(std::io::Error::other)?;
        if available > 0 {
            let mut response = [0];
            std::io::stdin().read_exact(&mut response)?;
            return Ok(response[0] == 1);
        }
        if started.elapsed() >= std::time::Duration::from_millis(100) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "keyboard routing timeout",
            ));
        }
        thread::yield_now();
    }
}

unsafe extern "system" fn worker_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && !FAILED.load(Ordering::Acquire) {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if key.flags.0 & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) == 0 {
            match exchange(encode([
                wparam.0 as u32,
                key.scanCode,
                key.vkCode,
                key.flags.0,
            ])) {
                Ok(true) => return LRESULT(1),
                Ok(false) => {}
                Err(error) => {
                    FAILED.store(true, Ordering::Release);
                    eprintln!(
                        "keyboard worker IPC error kind={:?}: {}",
                        error.kind(),
                        error
                    );
                    unsafe { PostQuitMessage(1) };
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_worker_failure_is_recovered_instead_of_retiring_to_webview_hook() {
        use std::cell::Cell;
        let attempts = Cell::new(0);
        let recoveries = Cell::new(0);
        supervise(
            std::iter::repeat_n((), 4),
            || {
                attempts.set(attempts.get() + 1);
                anyhow::bail!("fixture IPC disconnect")
            },
            || recoveries.set(recoveries.get() + 1),
            || {},
        );
        assert_eq!(
            attempts.get(),
            4,
            "a transient pipe failure must not permanently disable isolated capture"
        );
        assert_eq!(recoveries.get(), 4);
    }
    #[test]
    fn keyboard_pipe_preserves_system_key_identity_and_edges() {
        for vk in [0x5b, 0x5c, 0x2e, 0x70, 0x71, 0x7b, 0x12, 0x09] {
            for message in [WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP] {
                let fields = [message, 0x53, vk, 0x81];
                assert_eq!(decode(encode(fields)), fields);
            }
        }
    }
}
