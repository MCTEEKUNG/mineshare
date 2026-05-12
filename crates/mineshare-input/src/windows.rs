//! Windows input via low-level hooks (capture) and enigo (inject).
//!
//! M2 layer adds **edge-triggered cursor handover**: when the cursor reaches
//! the right edge of the local screen we enter a "remote" mode where
//!  * the local cursor is warped to a centre anchor every event so subsequent
//!    HW motion keeps producing fresh deltas (the WH_MOUSE_LL hook needs
//!    cursor position changes to fire);
//!  * captured deltas, button events, and keystrokes are forwarded to the
//!    peer instead of being processed locally;
//!  * a virtual `(virt_x, virt_y)` cursor position is tracked in remote
//!    space — when `virt_x` falls below zero we hand control back and the
//!    real Windows cursor is restored at the right edge.
//!
//! `SetCursorPos` does not trigger `WH_MOUSE_LL` (only HW interrupts do), so
//! the warp is invisible to the hook.

use std::mem;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread;

use anyhow::{Context, Result};
use enigo::{
    Axis, Button as EButton, Coordinate, Direction, Enigo, Key as EKey, Keyboard, Mouse, Settings,
};
use parking_lot::Mutex;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, CURSORINFO, CURSOR_SHOWING, CallNextHookEx,
    DispatchMessageW, GetClipCursor, GetCursorInfo, GetCursorPos, GetMessageW,
    GetSystemMetrics, HC_ACTION, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    RegisterClassExW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SetCursorPos, SetWindowsHookExW, TranslateMessage,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSEXW,
    WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH, RECT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    PROCESS_NAME_FORMAT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::{Button, InputCapture, InputEvent, InputInject, KeyCode};

const MODE_LOCAL: u8 = 0;
const MODE_REMOTE: u8 = 1;

static EVENT_SINK: OnceLock<
    Mutex<Option<std::sync::Arc<dyn Fn(InputEvent) + Send + Sync + 'static>>>,
> = OnceLock::new();
static LAST_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_Y: AtomicI32 = AtomicI32::new(i32::MIN);
static CURSOR_MODE: AtomicU8 = AtomicU8::new(MODE_LOCAL);
/// Virtual desktop bounding rectangle. On a single-monitor setup
/// this matches `SM_CXSCREEN` / `SM_CYSCREEN`; with multiple
/// monitors it widens to span every connected display.
/// (Stage 9 — was previously primary-monitor only.)
static SCREEN_W: AtomicI32 = AtomicI32::new(1920);
static SCREEN_H: AtomicI32 = AtomicI32::new(1080);
/// Top-left of the virtual screen in Windows-screen coordinates.
/// Non-zero when a secondary monitor sits above or to the left of
/// the primary; required because `WH_MOUSE_LL` and `SetCursorPos`
/// both work in this coordinate space and a hard-coded "0,0" left
/// edge becomes wrong as soon as the user adds a left-side monitor.
static ORIGIN_X: AtomicI32 = AtomicI32::new(0);
static ORIGIN_Y: AtomicI32 = AtomicI32::new(0);
/// Approximate peer screen width — used to clamp `VIRT_X` so that pushing
/// past the peer's right edge stops accumulating instead of letting the
/// virtual cursor race off into infinity (which makes it impossible to
/// drag back to negative virt_x and exit Remote mode).
///
/// 1920 is a sensible default until M2 Slice 2 negotiates the real width
/// over the encrypted control channel.
static PEER_W: AtomicI32 = AtomicI32::new(1920);
static VIRT_X: AtomicI32 = AtomicI32::new(0);
static VIRT_Y: AtomicI32 = AtomicI32::new(0);

/// Sub-pixel residue for the Stage 10 sensitivity multiplier.
/// `f32` bits packed into `AtomicU32` — the WH_MOUSE_LL hook is
/// effectively single-threaded but using atomics keeps the
/// pattern uniform with the rest of this file.
static SENS_RESIDUE_X: AtomicU32 = AtomicU32::new(0);
static SENS_RESIDUE_Y: AtomicU32 = AtomicU32::new(0);

/// Hysteresis buffer in pixels at the left edge of the peer screen. The
/// user has to drag this much further left than virt_x = 0 before we hand
/// control back to the local desktop. Without it, any tiny leftward jitter
/// (or natural left-tracking inside the peer screen) bounces the cursor
/// back out of Remote mode immediately.
const EXIT_BUFFER_PX: i32 = 100;

// Modifier-key tracking (PS/2 set-1 scan codes — left/right both produce
// the same scancode here, so we ignore the LLKHF_EXTENDED bit).
const SCAN_CTRL: u32 = 0x1D;
const SCAN_ALT: u32 = 0x38;
/// Hotkey: Ctrl+Alt+R forces exit_remote regardless of cursor position.
/// Useful when remote-mode gets stuck (e.g. peer disconnected mid-session).
const SCAN_HOTKEY: u32 = 0x13; // R
/// Hotkey: Ctrl+Alt+L toggles game-mode lock — pins all input to
/// this PC so accidental edge crosses during fullscreen gameplay
/// don't yank focus.
const SCAN_HOTKEY_LOCK: u32 = 0x26; // L
/// Hotkey: Ctrl+Alt+K cycles the keyboard target through
/// Auto → ForcePeer → ForceLocal → Auto. Lets the user pin
/// keys to the peer (or back to local) without moving the mouse
/// cursor — useful for "leave mouse here, type over there"
/// workflows.
const SCAN_HOTKEY_KB: u32 = 0x25; // K

static MOD_CTRL: AtomicBool = AtomicBool::new(false);
static MOD_ALT: AtomicBool = AtomicBool::new(false);

/// Set to `true` once `WM_INPUT` raw-mouse registration succeeds in
/// `hook_thread`.  When true, `low_mouse_hook` skips motion forwarding
/// (motion comes via `handle_raw_input` instead — raw pre-acceleration
/// hardware mickeys, identical to what the Linux evdev path forwards,
/// giving 1:1 mouse feel regardless of whether the peer is Linux or
/// Windows).
///
/// If registration fails (no GUI session, sandboxed environment) we fall
/// back to the old WH_MOUSE_LL screen-space delta path.
static USING_RAW_INPUT: AtomicBool = AtomicBool::new(false);

/// Timestamp (`super::now_ms`) of the most recent raw input motion event
/// received via `WM_INPUT`.  The hook fallback uses this as a liveness
/// check — if registration succeeded but no WM_INPUT actually arrives
/// (some Windows builds / message-only-window quirks silently drop the
/// delivery), the hook path takes over so the cursor still moves on the
/// peer.  300ms window picks up >3 ticks of a 125 Hz mouse.
static LAST_RAW_INPUT_MS: AtomicU64 = AtomicU64::new(0);
const RAW_INPUT_STALE_MS: u64 = 300;

// --- Motion coalescing (port of linux.rs 8ms window) ---------------------
//
// Raw input on Windows can fire at the mouse's polling rate (often 1000 Hz
// on gaming mice).  Sending one UDP packet per event saturates the peer's
// receive-and-inject loop — each `Enigo::move_mouse` call holds a mutex
// and serialises through a single tokio task, so 1000 events/sec arrives
// as a stuttery batch on the peer's cursor.
//
// Fix: accumulate dx/dy into `PENDING_*` and dispatch one combined event
// every 8 ms (~125 Hz, indistinguishable from a 125 Hz polling mouse).
// Mirrors `linux.rs::flush_pending` so both sides behave identically.
const FLUSH_INTERVAL_MS: u64 = 8;
static PENDING_DX: AtomicI32 = AtomicI32::new(0);
static PENDING_DY: AtomicI32 = AtomicI32::new(0);
static LAST_FLUSH_MS: AtomicU64 = AtomicU64::new(0);
static FLUSH_WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
static FLUSH_FWD_COUNT: AtomicI32 = AtomicI32::new(0);

/// Atomically drain `PENDING_DX/DY` and forward the combined delta.
/// Skips no-op events when both axes are zero.
fn flush_pending_motion() {
    let dx = PENDING_DX.swap(0, Ordering::AcqRel);
    let dy = PENDING_DY.swap(0, Ordering::AcqRel);
    if dx == 0 && dy == 0 {
        return;
    }
    LAST_FLUSH_MS.store(super::now_ms(), Ordering::Release);
    sink_send(InputEvent::MouseMove { dx, dy });
    let n = FLUSH_FWD_COUNT.fetch_add(1, Ordering::Relaxed);
    if n % 100 == 0 {
        info!(dx, dy, n, "win coalesced motion forward (8ms-window)");
    }
}

/// Spawn the periodic flush thread once.  Wakes every `FLUSH_INTERVAL_MS`
/// and drains pending motion if `CURSOR_MODE == REMOTE`.  Mirrors the
/// Linux watchdog — needed so a "moved 2 px then paused" residual
/// doesn't sit in the accumulator until the next motion event.
fn start_motion_flush_watchdog() {
    if FLUSH_WATCHDOG_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(e) = thread::Builder::new()
        .name("win-motion-flush".into())
        .spawn(|| loop {
            thread::sleep(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));
            if CURSOR_MODE.load(Ordering::Acquire) != MODE_REMOTE {
                continue;
            }
            let now = super::now_ms();
            let last = LAST_FLUSH_MS.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= FLUSH_INTERVAL_MS {
                flush_pending_motion();
            }
        })
    {
        warn!(error = %e, "failed to spawn motion flush watchdog");
        FLUSH_WATCHDOG_STARTED.store(false, Ordering::Release);
    }
}

/// Fallback per-event delta cap used ONLY when `USING_RAW_INPUT = false`.
/// The WH_MOUSE_LL path delivers post-acceleration screen-space coords;
/// the cap prevents a coalesced burst from teleporting the peer cursor.
/// Irrelevant (not reached) when raw input is active.
const MAX_DELTA_PX: i32 = 30;

fn sink_send(ev: InputEvent) {
    if let Some(s) = EVENT_SINK.get()
        && let Some(cb) = s.lock().as_ref()
    {
        cb(ev);
    }
}

pub fn local_screen_geometry() -> (u32, u32) {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        // Idempotent — failures here just mean DPI awareness is already
        // set. Calling before HookCapture::start() lets a `--no-capture`
        // daemon still report DPI-aware physical pixels.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
    // Virtual screen — the smallest rectangle that contains every
    // connected monitor. On a 2-monitor side-by-side setup this is
    // (combined-width, max-height), and the origin can be negative
    // if the secondary monitor is to the left of the primary.
    let ox = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let oy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32 };
    let h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32 };
    ORIGIN_X.store(ox, Ordering::Relaxed);
    ORIGIN_Y.store(oy, Ordering::Relaxed);
    SCREEN_W.store(w as i32, Ordering::Relaxed);
    SCREEN_H.store(h as i32, Ordering::Relaxed);
    (w, h)
}

pub fn set_peer_screen(w: u32, _h: u32) {
    PEER_W.store(w.max(1) as i32, Ordering::Relaxed);
    info!(peer_w = w, "peer screen geometry stored");
}

fn anchor() -> (i32, i32) {
    // Centre of the virtual desktop, not just primary monitor.
    let ox = ORIGIN_X.load(Ordering::Relaxed);
    let oy = ORIGIN_Y.load(Ordering::Relaxed);
    let w = SCREEN_W.load(Ordering::Relaxed);
    let h = SCREEN_H.load(Ordering::Relaxed);
    (ox + w / 2, oy + h / 2)
}

/// Outer boundary edges of the virtual desktop, in
/// SetCursorPos / WH_MOUSE_LL coordinates.
fn bounds() -> (i32, i32, i32, i32) {
    let ox = ORIGIN_X.load(Ordering::Relaxed);
    let oy = ORIGIN_Y.load(Ordering::Relaxed);
    let w = SCREEN_W.load(Ordering::Relaxed);
    let h = SCREEN_H.load(Ordering::Relaxed);
    (ox, oy, ox + w - 1, oy + h - 1) // (left, top, right, bottom)
}

fn enter_remote(entry_y: i32) {
    // Refuse if the peer signalled it's already driving Remote — otherwise
    // both ends forward each other's HW input simultaneously and we end
    // up with cursors fighting on both screens.
    if super::peer_in_remote() {
        debug!("enter_remote refused — peer holds Remote");
        return;
    }
    let (ax, ay) = anchor();
    VIRT_X.store(0, Ordering::Relaxed);
    VIRT_Y.store(entry_y, Ordering::Relaxed);
    unsafe {
        let _ = SetCursorPos(ax, ay);
    }
    LAST_X.store(ax, Ordering::Relaxed);
    LAST_Y.store(ay, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_REMOTE, Ordering::Release);
    info!(entry_y, anchor = ?(ax, ay), "cursor → remote");
    super::fire_remote_event(super::RemoteEvent::Entered);
}

fn exit_remote(restore_y: i32) {
    let (left, top, right, bottom) = bounds();
    // Restore the local cursor to the edge that faces the peer
    // (per the configured layout). User came back across that
    // edge to leave Remote, so dropping the OS cursor there
    // matches their hand position on the desk. For top/bottom
    // we keep their horizontal anchor, just snap Y to the edge.
    let (restore_x, restore_y) = match super::peer_side() {
        super::PeerSide::Right => (right, restore_y.clamp(top, bottom)),
        super::PeerSide::Left => (left, restore_y.clamp(top, bottom)),
        super::PeerSide::Top => (LAST_X.load(Ordering::Relaxed).clamp(left, right), top),
        super::PeerSide::Bottom => (LAST_X.load(Ordering::Relaxed).clamp(left, right), bottom),
    };
    unsafe {
        let _ = SetCursorPos(restore_x, restore_y);
    }
    LAST_X.store(restore_x, Ordering::Relaxed);
    LAST_Y.store(restore_y, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
    info!(restore = ?(restore_x, restore_y), "cursor → local");
    super::fire_remote_event(super::RemoteEvent::Exited);
}

pub fn local_in_remote() -> bool {
    CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE
}

pub fn force_exit_remote() {
    if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
        let (_, _, _, bottom) = bounds();
        let oy = ORIGIN_Y.load(Ordering::Relaxed);
        info!("force_exit_remote — peer asked us to release");
        // Halfway down the virtual desktop is a sensible default
        // restore-Y when the user only knows we want out, not where.
        exit_remote((oy + bottom) / 2);
    }
}

/// Called when the peer signals it has taken Remote control of us.
/// Warps the local cursor to the boundary edge facing the peer (per
/// the configured layout) so the peer's virt_x model matches the
/// real cursor position — without this the peer's exit threshold
/// fires after a tiny motion in the wrong direction even though the
/// cursor is mid-screen.
pub fn on_peer_take_control() {
    let (left, top, right, bottom) = bounds();
    let mid_x = (left + right) / 2;
    let mid_y = (top + bottom) / 2;
    let (x, y) = match super::peer_side() {
        super::PeerSide::Right => (right, mid_y),
        super::PeerSide::Left => (left, mid_y),
        super::PeerSide::Top => (mid_x, top),
        super::PeerSide::Bottom => (mid_x, bottom),
    };
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    // Update the hook's "last seen" so HW-motion auto-release doesn't
    // mis-fire on the first injected motion arriving from the peer.
    LAST_X.store(x, Ordering::Relaxed);
    LAST_Y.store(y, Ordering::Relaxed);
    info!(boundary = ?(x, y), side = ?super::peer_side(), "warped cursor to peer-facing edge");
}

pub struct HookCapture {
    started: bool,
}

impl HookCapture {
    pub fn new() -> Result<Self> {
        Ok(Self { started: false })
    }
}

impl InputCapture for HookCapture {
    fn start(
        &mut self,
        sink: std::sync::Arc<dyn Fn(InputEvent) + Send + Sync + 'static>,
    ) -> Result<()> {
        if self.started {
            return Ok(());
        }
        self.started = true;

        // Make this process per-monitor-DPI-aware **before** querying
        // screen geometry. Without it, GetSystemMetrics returns logical
        // (DPI-virtualised) pixels while WH_MOUSE_LL delivers physical
        // pixels — a 200% scale display then reports cursor coords up to
        // 2× our screen-width assumption, producing dx values of 2000+
        // that warp Ubuntu's cursor straight to the right edge.
        unsafe {
            // Best-effort: a process can only set this once, so failure
            // here usually just means a previous setter already ran.
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        // Probe virtual-screen geometry (Stage 9) — bounding rect
        // around every connected monitor. Hot-plug isn't tracked
        // yet; users with monitors that come and go after launch
        // need to restart the daemon. Static probe at startup is
        // fine for the 99% case.
        let ox = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let oy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        if w > 0 && h > 0 {
            ORIGIN_X.store(ox, Ordering::Relaxed);
            ORIGIN_Y.store(oy, Ordering::Relaxed);
            SCREEN_W.store(w, Ordering::Relaxed);
            SCREEN_H.store(h, Ordering::Relaxed);
            info!(origin = ?(ox, oy), width = w, height = h, "virtual screen geometry");
        } else {
            warn!("GetSystemMetrics returned 0 for virtual screen — falling back to 1920x1080");
        }

        let cell = EVENT_SINK.get_or_init(|| Mutex::new(None));
        *cell.lock() = Some(sink);

        thread::Builder::new()
            .name("win-input-hooks".into())
            .spawn(|| unsafe { hook_thread() })
            .context("spawn hook thread")?;

        // Game auto-detect: a separate poll thread watches the
        // visible-cursor flag + the clip-cursor rect. When a
        // foreground app hides or confines the cursor (the
        // signature of fullscreen FPS / GTA / Minecraft mouse
        // capture), we auto-engage the input lock so an
        // accidental edge cross during gameplay can't yank focus
        // to the peer. We deliberately only set/clear an
        // *auto* flag, separate from the manual lock — the user's
        // explicit Ctrl+Alt+L override always wins.
        thread::Builder::new()
            .name("win-game-detect".into())
            .spawn(|| game_detect_thread())
            .context("spawn game-detect thread")?;
        Ok(())
    }
}

/// Anti-cheat-protected executables. Foreground match → auto-lock
/// the bridge regardless of cursor state, and surface a red
/// banner on the GUI Status tab so the user *knows* why their
/// keyboard isn't crossing. These games' kernel-level anti-cheat
/// (BattlEye / EAC / Vanguard / RICOCHET / Hyperion) routinely
/// bans accounts for SendInput-style injected events — silently
/// dropping the bridge is the safer default than letting an
/// accidental edge cross arrive as suspect input on the peer.
///
/// Match is case-insensitive on the basename. Add new entries
/// freely — false positives just engage a lock the user can
/// override with Ctrl+Alt+R, false negatives are the dangerous
/// direction.
const RISKY_GAMES: &[&str] = &[
    // Riot Vanguard
    "VALORANT.exe",
    "VALORANT-Win64-Shipping.exe",
    "LeagueClient.exe",
    "League of Legends.exe",
    // BattlEye
    "FortniteClient-Win64-Shipping.exe",
    "RainbowSix.exe",
    "RainbowSix_Vulkan.exe",
    "TslGame.exe", // PUBG
    "destiny2.exe",
    "ArmaReforger.exe",
    "DayZ_x64.exe",
    "Tarkov.exe",
    "EscapeFromTarkov.exe",
    // Easy Anti-Cheat
    "r5apex.exe", // Apex Legends
    "r5apex_dx12.exe",
    "FFXIV_dx11.exe",
    "RustClient.exe",
    "ELDENRING.exe",
    // Activision RICOCHET
    "cod.exe",
    "ModernWarfare.exe",
    "BlackOpsColdWar.exe",
    // Roblox Hyperion
    "RobloxPlayerBeta.exe",
    // FACEIT / ESEA
    "csgo.exe",
    "cs2.exe",
];

fn current_foreground_exe_basename() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(proc, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(proc);
        if res.is_err() {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    }
}

/// Polls cursor visibility + clip-rect + foreground-process state
/// every 250 ms. Auto-engages the input lock when:
///   * the cursor is hidden (Minecraft-style fullscreen capture)
///   * OR the cursor clip rect is smaller than the screen (FPS
///     mouse-confine)
///   * OR the foreground process matches the anti-cheat-protected
///     `RISKY_GAMES` list — even if cursor is visible (main menu)
///
/// Manual user-engaged locks survive auto-detect releases (user
/// always wins) so alt-tabbing out of a game momentarily doesn't
/// drop a user-set lock.
fn game_detect_thread() {
    use std::sync::atomic::AtomicBool;
    static AUTO_ENGAGED: AtomicBool = AtomicBool::new(false);
    let poll_ms = std::time::Duration::from_millis(250);
    loop {
        std::thread::sleep(poll_ms);
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        let cursor_hidden = unsafe { GetCursorInfo(&mut ci) }.is_ok()
            && (ci.flags.0 & CURSOR_SHOWING.0) == 0;

        // Cursor confine: a real fullscreen game's clip rect is a
        // small fraction of the screen (the playable window).
        // Plain Tauri / browser focus can knock a few pixels off
        // the OS default clip and would otherwise false-positive,
        // so require the clip to be meaningfully smaller —
        // < 70% on BOTH axes — before treating it as a game.
        let mut clip = RECT::default();
        let cursor_clipped = unsafe { GetClipCursor(&mut clip) }.is_ok() && {
            let w = SCREEN_W.load(Ordering::Relaxed);
            let h = SCREEN_H.load(Ordering::Relaxed);
            let cw = clip.right - clip.left;
            let ch = clip.bottom - clip.top;
            cw < w * 7 / 10 && ch < h * 7 / 10
        };

        // Foreground process match — independent of cursor state.
        let exe = current_foreground_exe_basename();
        let anticheat_match = exe.as_deref().and_then(|e| {
            RISKY_GAMES
                .iter()
                .find(|r| r.eq_ignore_ascii_case(e))
                .map(|_| e.to_string())
        });
        super::set_anticheat_warning(anticheat_match.clone());

        let should_lock = cursor_hidden || cursor_clipped || anticheat_match.is_some();
        let was_engaged = AUTO_ENGAGED.load(Ordering::Acquire);
        if should_lock != was_engaged {
            AUTO_ENGAGED.store(should_lock, Ordering::Release);
            if should_lock {
                if !super::is_input_locked() {
                    info!(
                        cursor_hidden,
                        cursor_clipped,
                        anticheat = ?anticheat_match,
                        "game auto-detect — engaging lock"
                    );
                    super::set_input_locked(true);
                }
            } else if super::is_input_locked() && was_engaged {
                info!("game auto-detect — releasing lock");
                super::set_input_locked(false);
            }
        }
    }
}

unsafe fn hook_thread() {
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_mouse_hook), None, 0) };
    let kb = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_kb_hook), None, 0) };
    let mouse_ok = matches!(&mouse, Ok(h) if !h.0.is_null());
    let kb_ok = matches!(&kb, Ok(h) if !h.0.is_null());
    if !mouse_ok || !kb_ok {
        warn!(?mouse, ?kb, "SetWindowsHookExW failed (need GUI session)");
        return;
    }
    info!("Win hooks installed (WH_MOUSE_LL + WH_KEYBOARD_LL)");

    // 8ms motion coalescer: batches raw input + hook fallback events
    // into one MouseMove every ~8ms so the peer's inject loop isn't
    // overwhelmed by a 1000 Hz mouse.  Same pattern as linux.rs.
    start_motion_flush_watchdog();

    // Create a message-only window so we can receive WM_INPUT via
    // RIDEV_INPUTSINK.  Raw input gives pre-acceleration hardware mickeys
    // — the same unit that Linux evdev forwards — so the peer cursor
    // behaves naturally under whatever acceleration the receiver applies.
    if let Some(hwnd) = create_raw_input_window() {
        let rid = RAWINPUTDEVICE {
            usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
            usUsage: 0x02,     // HID_USAGE_GENERIC_MOUSE
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        match unsafe { RegisterRawInputDevices(&[rid], std::mem::size_of::<RAWINPUTDEVICE>() as u32) } {
            Ok(()) => {
                USING_RAW_INPUT.store(true, Ordering::Release);
                info!("raw mouse input registered (WM_INPUT / RIDEV_INPUTSINK)");
            }
            Err(e) => {
                warn!(?e, "RegisterRawInputDevices failed — using hook-based delta (may feel slow at high speed)");
            }
        }
    } else {
        warn!("create_raw_input_window failed — using hook-based delta");
    }

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        // Raw mouse input: forward pre-acceleration hardware mickeys when
        // in REMOTE mode.  This runs AFTER low_mouse_hook has already
        // updated VIRT_X / done the anchor warp for the same event.
        if msg.message == WM_INPUT {
            unsafe { handle_raw_input(HRAWINPUT(msg.lParam.0 as *mut _)) };
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Minimal window procedure: delegate everything to DefWindowProcW.
unsafe extern "system" fn raw_input_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Create a message-only (HWND_MESSAGE) window that receives WM_INPUT.
/// Returns `None` if window class registration or window creation fails.
fn create_raw_input_window() -> Option<HWND> {
    use windows::core::PCWSTR;
    let class_name: Vec<u16> = "MineShareRI\0".encode_utf16().collect();
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(raw_input_wnd_proc),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..std::mem::zeroed()
        };
        // RegisterClassExW fails harmlessly if the class already exists
        // (e.g. across reconnects in the same process).
        let _ = RegisterClassExW(&wc);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0, 0, 0, 0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        ).ok()
    }
}

/// Process one WM_INPUT message: extract raw mouse deltas and forward
/// them to the peer when in REMOTE mode.
///
/// Because these are pre-acceleration hardware mickeys (same unit as
/// Linux evdev REL_X/REL_Y), the peer OS applies its own pointer
/// acceleration naturally — giving the same 1:1 "natural mouse" feel
/// regardless of whether the peer is Windows or Linux.
unsafe fn handle_raw_input(h: HRAWINPUT) {
    if !USING_RAW_INPUT.load(Ordering::Relaxed) { return; }
    if CURSOR_MODE.load(Ordering::Acquire) != MODE_REMOTE { return; }

    // Two-pass: first get required buffer size, then read data.
    let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let mut size: u32 = 0;
    unsafe {
        GetRawInputData(h, RID_INPUT, None, &mut size, header_size);
    }
    if size == 0 || size > 1024 { return; }

    let mut buf = vec![0u8; size as usize];
    let written = unsafe {
        GetRawInputData(
            h,
            RID_INPUT,
            Some(buf.as_mut_ptr() as *mut _),
            &mut size,
            header_size,
        )
    };
    if written == u32::MAX || written == 0 { return; }

    let raw = unsafe { &*(buf.as_ptr() as *const RAWINPUT) };
    // Only handle mouse events (dwType == 0 == RIM_TYPEMOUSE).
    if raw.header.dwType != RIM_TYPEMOUSE.0 { return; }

    let mouse = unsafe { &raw.data.mouse };
    // Skip absolute-position events (touch digitiser, graphics tablet…).
    if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 { return; }

    let dx = mouse.lLastX;
    let dy = mouse.lLastY;
    if dx == 0 && dy == 0 { return; }

    // Apply user sensitivity multiplier with sub-pixel residue.
    let mut rx = f32::from_bits(SENS_RESIDUE_X.load(Ordering::Relaxed));
    let mut ry = f32::from_bits(SENS_RESIDUE_Y.load(Ordering::Relaxed));
    let sdx = super::scale_delta(dx, &mut rx);
    let sdy = super::scale_delta(dy, &mut ry);
    SENS_RESIDUE_X.store(rx.to_bits(), Ordering::Relaxed);
    SENS_RESIDUE_Y.store(ry.to_bits(), Ordering::Relaxed);

    // Record liveness so the hook fallback knows raw input is delivering.
    LAST_RAW_INPUT_MS.store(super::now_ms(), Ordering::Release);

    static RAW_FWD: AtomicI32 = AtomicI32::new(0);
    let n = RAW_FWD.fetch_add(1, Ordering::Relaxed);
    if n % 500 == 0 {
        info!(raw_dx = dx, raw_dy = dy, sdx, sdy, n, "sample raw motion captured");
    }

    // Coalesce into the 8ms accumulator instead of firing one packet
    // per HW event.  The flush watchdog (or opportunistic flush below)
    // dispatches the combined delta — keeps event rate at ~125 Hz so
    // the peer's inject loop stays responsive even with a 1000 Hz mouse.
    PENDING_DX.fetch_add(sdx, Ordering::AcqRel);
    PENDING_DY.fetch_add(sdy, Ordering::AcqRel);
    // Opportunistic flush: if the 8 ms window has already lapsed since
    // the last dispatch, send now instead of waiting for the watchdog.
    let now = super::now_ms();
    let last = LAST_FLUSH_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= FLUSH_INTERVAL_MS {
        flush_pending_motion();
    }
}

const LLMHF_INJECTED: u32 = 0x00000001;
const LLMHF_LOWER_IL_INJECTED: u32 = 0x00000002;
const LLKHF_INJECTED: u32 = 0x00000010;
const LLKHF_LOWER_IL_INJECTED: u32 = 0x00000002;

unsafe extern "system" fn low_mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    if info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let x = info.pt.x;
    let y = info.pt.y;
    let mode = CURSOR_MODE.load(Ordering::Acquire);
    let last_x = LAST_X.load(Ordering::Relaxed);
    let last_y = LAST_Y.load(Ordering::Relaxed);

    // Click-based focus signal: any local mouse-button DOWN
    // (regardless of mode) bumps `LOCAL_CLICK_AT`. The next
    // activity beacon will report the age, and Smart keyboard
    // routing on the peer side will treat the more recent click
    // as the "user is focused on that machine" signal.
    if matches!(
        wparam.0 as u32,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    ) {
        super::bump_local_click();
    }

    match wparam.0 as u32 {
        WM_MOUSEMOVE => {
            // Stage 11 Smart-keyboard: any HW mouse motion on this
            // machine (the hook never sees `SetCursorPos`-driven
            // events) is a signal that the local user is actively
            // working here. Bump regardless of mode so the peer's
            // Smart-target can see "Win mouse is in use, don't
            // route Linux keys back here".
            super::bump_local_mouse_activity();
            if mode == MODE_LOCAL {
                let (left, top, right, bottom) = bounds();
                // Auto-release peer Remote on real local HW motion (the
                // user is moving Win HW while Ubuntu is driving). Same
                // pattern as Synergy: any local input on the passive
                // side reclaims the cursor.
                if super::peer_in_remote()
                    && last_x != i32::MIN
                    && (x - last_x).abs() + (y - last_y).abs() > 5
                {
                    info!(
                        dx = x - last_x,
                        dy = y - last_y,
                        "local HW motion while peer holds Remote — requesting peer release"
                    );
                    super::fire_remote_event(super::RemoteEvent::RequestPeerExit);
                }
                LAST_X.store(x, Ordering::Relaxed);
                LAST_Y.store(y, Ordering::Relaxed);
                // Edge to watch depends on layout. We treat hitting
                // the OUTER edge of the virtual desktop as the
                // trigger (Stage 9) — internal monitor seams don't
                // count, so a 2-monitor user can drag freely
                // between displays without falling into Remote.
                // Game-mode lock pins input to this PC — skip the
                // edge check entirely so accidental cursor moves
                // during fullscreen play don't yank focus to the
                // peer. Ctrl+Alt+R still works as a manual override.
                let crossed_edge = !super::is_input_locked()
                    && last_x != i32::MIN
                    && match super::peer_side() {
                        super::PeerSide::Right => last_x < right && x >= right,
                        super::PeerSide::Left => last_x > left && x <= left,
                        super::PeerSide::Top => last_y > top && y <= top,
                        super::PeerSide::Bottom => last_y < bottom && y >= bottom,
                    };
                if crossed_edge {
                    enter_remote(y);
                }
                // local: don't forward, OS handles cursor as usual
            } else {
                // REMOTE: compute delta from anchor, clamp to peer screen,
                // and forward. The "depth" axis (how far INTO the peer
                // we've gone) flips with the layout — right means
                // rightward dx, left means -dx, top means -dy, bottom
                // means dy. Exit fires when depth retreats to
                // -EXIT_BUFFER_PX past the entry edge.
                let dx = x - last_x;
                let dy = y - last_y;
                let depth_dx = match super::peer_side() {
                    super::PeerSide::Right => dx,
                    super::PeerSide::Left => -dx,
                    super::PeerSide::Top => -dy,
                    super::PeerSide::Bottom => dy,
                };
                let peer_w = PEER_W.load(Ordering::Relaxed);
                // Clamp accumulated virt_x to [-EXIT_BUFFER_PX, peer_w-1].
                // The lower floor doubles as the exit trigger; the upper
                // bound stops further depth-direction dx from being
                // absorbed into ever-growing virt_x (which would force
                // the user to drag back for thousands of events to escape).
                let raw = VIRT_X.load(Ordering::Relaxed) + depth_dx;
                let new_virt_x = raw.clamp(-EXIT_BUFFER_PX, peer_w - 1);
                VIRT_X.store(new_virt_x, Ordering::Relaxed);
                VIRT_Y.fetch_add(dy, Ordering::Relaxed);

                if new_virt_x <= -EXIT_BUFFER_PX {
                    exit_remote(y);
                } else {
                    // Anchor warp: keeps WH_MOUSE_LL firing so edge-exit
                    // detection (VIRT_X above) continues to work.
                    // Motion forwarding is handled by `handle_raw_input`
                    // (WM_INPUT, pre-acceleration hardware mickeys) when
                    // raw input is *actually delivering events*.  If
                    // registration succeeded but WM_INPUT never arrives
                    // (HWND_MESSAGE quirks on some Win builds, anti-
                    // cheat conflicts, etc.), `LAST_RAW_INPUT_MS` stays
                    // stale and we fall back here so the cursor still
                    // moves on the peer.
                    let (ax, ay) = anchor();
                    let last_raw = LAST_RAW_INPUT_MS.load(Ordering::Acquire);
                    let raw_is_live = last_raw != 0
                        && super::now_ms().saturating_sub(last_raw) < RAW_INPUT_STALE_MS;
                    if !raw_is_live {
                        // Fallback: post-accel screen-space delta with cap.
                        // Same accumulator as raw input path — the flush
                        // watchdog dispatches at ~125 Hz so the peer sees
                        // a steady event rate regardless of source.
                        let mut rx = f32::from_bits(SENS_RESIDUE_X.load(Ordering::Relaxed));
                        let mut ry = f32::from_bits(SENS_RESIDUE_Y.load(Ordering::Relaxed));
                        let scaled_dx = super::scale_delta(dx, &mut rx);
                        let scaled_dy = super::scale_delta(dy, &mut ry);
                        SENS_RESIDUE_X.store(rx.to_bits(), Ordering::Relaxed);
                        SENS_RESIDUE_Y.store(ry.to_bits(), Ordering::Relaxed);
                        let fdx = scaled_dx.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
                        let fdy = scaled_dy.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
                        if fdx != 0 || fdy != 0 {
                            static HOOK_CAPTURED: AtomicI32 = AtomicI32::new(0);
                            let n = HOOK_CAPTURED.fetch_add(1, Ordering::Relaxed);
                            if n % 200 == 0 {
                                info!(
                                    raw_dx = dx, raw_dy = dy, fdx, fdy,
                                    virt_x = new_virt_x, n,
                                    "sample motion captured (hook fallback)"
                                );
                            }
                            PENDING_DX.fetch_add(fdx, Ordering::AcqRel);
                            PENDING_DY.fetch_add(fdy, Ordering::AcqRel);
                            // Opportunistic flush if the 8ms window has lapsed.
                            let now = super::now_ms();
                            let last_flush = LAST_FLUSH_MS.load(Ordering::Relaxed);
                            if now.saturating_sub(last_flush) >= FLUSH_INTERVAL_MS {
                                flush_pending_motion();
                            }
                        }
                    }
                    unsafe { let _ = SetCursorPos(ax, ay); }
                    LAST_X.store(ax, Ordering::Relaxed);
                    LAST_Y.store(ay, Ordering::Relaxed);
                }
                // Consume the event so the OS doesn't process it locally.
                return LRESULT(1);
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
        | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
            // Resolve which button + direction. X1 / X2 share the
            // same WM_* — `info.mouseData` high word disambiguates.
            let (btn, down) = match wparam.0 as u32 {
                WM_LBUTTONDOWN => (Some(Button::Left), true),
                WM_LBUTTONUP => (Some(Button::Left), false),
                WM_RBUTTONDOWN => (Some(Button::Right), true),
                WM_RBUTTONUP => (Some(Button::Right), false),
                WM_MBUTTONDOWN => (Some(Button::Middle), true),
                WM_MBUTTONUP => (Some(Button::Middle), false),
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let down = wparam.0 as u32 == WM_XBUTTONDOWN;
                    let high = (info.mouseData >> 16) as u16;
                    let btn = match high {
                        1 => Some(Button::X1),
                        2 => Some(Button::X2),
                        _ => None,
                    };
                    (btn, down)
                }
                _ => (None, false),
            };
            if let Some(btn) = btn {
                // Held-aware routing: any DOWN previously forwarded
                // is remembered, and its eventual UP forwards too —
                // even if the cursor has crossed back to local in
                // the meantime. Without this the peer ends up with
                // a button stuck-down (drag-select runs wild, links
                // never release, etc.).
                if super::route_mouse_button(btn, down, mode == MODE_REMOTE) {
                    sink_send(InputEvent::MouseButton { btn, down });
                    return LRESULT(1);
                }
            }
        }
        WM_MOUSEWHEEL if mode == MODE_REMOTE => {
            let delta = ((info.mouseData >> 16) as i16) as f32 / 120.0;
            // Stage 10: optional Y-axis inversion. Win has no
            // horizontal-wheel hook here so only Y matters.
            let dy = if super::invert_scroll_y() { -delta } else { delta };
            sink_send(InputEvent::Scroll { dx: 0.0, dy });
        }
        _ => {}
    }
    // In remote mode every mouse event has been forwarded; consume it so
    // the OS doesn't double-process it locally.
    if mode == MODE_REMOTE {
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn low_kb_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.flags.0 & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let scan = info.scanCode;
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);

        // Track modifier state regardless of mode so the hotkey works
        // even after a half-pressed transition.
        if scan == SCAN_CTRL && (down || up) {
            MOD_CTRL.store(down, Ordering::Relaxed);
        }
        if scan == SCAN_ALT && (down || up) {
            MOD_ALT.store(down, Ordering::Relaxed);
        }

        let mode = CURSOR_MODE.load(Ordering::Acquire);

        // Hotkey: Ctrl+Alt+R toggles Local ⇄ Remote, or asks the peer to
        // release if the peer is the one currently driving.
        if down
            && scan == SCAN_HOTKEY
            && MOD_CTRL.load(Ordering::Relaxed)
            && MOD_ALT.load(Ordering::Relaxed)
        {
            if mode == MODE_REMOTE {
                info!("hotkey Ctrl+Alt+R — forcing exit_remote");
                let (_, top, _, bottom) = bounds();
                exit_remote((top + bottom) / 2);
            } else if super::peer_in_remote() {
                info!("hotkey Ctrl+Alt+R — requesting peer to release");
                super::fire_remote_event(super::RemoteEvent::RequestPeerExit);
            } else {
                info!("hotkey Ctrl+Alt+R — entering remote");
                let mut pt = POINT::default();
                let (_, top, _, bottom) = bounds();
                let entry_y = unsafe {
                    if GetCursorPos(&mut pt).is_ok() {
                        pt.y
                    } else {
                        (top + bottom) / 2
                    }
                };
                enter_remote(entry_y);
            }
            return LRESULT(1);
        }

        // Hotkey: Ctrl+Alt+L toggles game-mode lock.
        if down
            && scan == SCAN_HOTKEY_LOCK
            && MOD_CTRL.load(Ordering::Relaxed)
            && MOD_ALT.load(Ordering::Relaxed)
        {
            let next = !super::is_input_locked();
            info!(locked = next, "hotkey Ctrl+Alt+L — game-mode lock");
            super::set_input_locked(next);
            return LRESULT(1);
        }

        // Hotkey: Ctrl+Alt+K cycles keyboard target. ALWAYS handled
        // here regardless of current target — otherwise pinning
        // keys to peer would also trap the un-pin hotkey on the
        // peer side and leave the user stuck.
        if down
            && scan == SCAN_HOTKEY_KB
            && MOD_CTRL.load(Ordering::Relaxed)
            && MOD_ALT.load(Ordering::Relaxed)
        {
            super::cycle_keyboard_target();
            info!(target = ?super::keyboard_target(), "hotkey Ctrl+Alt+K — keyboard target");
            return LRESULT(1);
        }

        // Keystrokes are routed via `route_keystroke`, which combines
        // the user's keyboard-target preference, the cursor's
        // current side, AND a held-key tracker that ensures every
        // key release follows its press to the same destination.
        // The tracker is what fixes the "Shift-stuck-on-peer →
        // every letter forced uppercase" Caps-Lock bug when Smart
        // flips mid-keypress.

        // Convert Windows VK code → Linux evdev code for extended keys
        // (arrow keys, Windows key, nav cluster).  For ordinary keys
        // the PS/2 scan code already equals the Linux evdev number, so
        // we fall back to `scan` when the VK is not in the table.
        let linux_code = EXTENDED_KEY_TABLE
            .iter()
            .find(|&&(_, vk)| vk == info.vkCode as u16)
            .map(|&(evdev, _)| evdev)
            .unwrap_or(scan as u16);

        if (down || up) && super::route_keystroke(scan as u16, down, mode == MODE_REMOTE) {
            sink_send(InputEvent::Key {
                code: KeyCode(linux_code),
                down,
            });
            super::note_key_forwarded_with_code(linux_code, down);
            // Consume so Windows doesn't also act on the keystroke.
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Tag tracking which kind of code is held so `release_all_held`
/// knows whether to call enigo's button() or key() to clear it.
#[derive(Hash, PartialEq, Eq, Clone, Copy)]
enum HeldKey {
    MouseBtn(Button),
    Key(u16),
}

pub struct EnigoInject {
    inner: Mutex<Enigo>,
    /// Codes the peer has injected `down` without a matching `up`.
    /// Drained on `release_all_held` at session teardown so a
    /// disconnected peer doesn't leave keys logically pressed in
    /// the local OS (typical: WASD held during a network drop).
    held: Mutex<std::collections::HashSet<HeldKey>>,
}

impl EnigoInject {
    pub fn new() -> Result<Self> {
        let inner = Enigo::new(&Settings::default()).context("init enigo")?;
        debug!("enigo inject ready");
        Ok(Self {
            inner: Mutex::new(inner),
            held: Mutex::new(std::collections::HashSet::new()),
        })
    }
}

impl InputInject for EnigoInject {
    fn mouse_move_rel(&self, dx: i32, dy: i32) -> Result<()> {
        self.inner
            .lock()
            .move_mouse(dx, dy, Coordinate::Rel)
            .context("enigo move_mouse")?;
        Ok(())
    }

    fn mouse_button(&self, btn: Button, down: bool) -> Result<()> {
        let b = match btn {
            Button::Left => EButton::Left,
            Button::Right => EButton::Right,
            Button::Middle => EButton::Middle,
            Button::X1 => EButton::Back,
            Button::X2 => EButton::Forward,
        };
        let dir = if down {
            Direction::Press
        } else {
            Direction::Release
        };
        self.inner.lock().button(b, dir).context("enigo button")?;
        let mut held = self.held.lock();
        if down {
            held.insert(HeldKey::MouseBtn(btn));
        } else {
            held.remove(&HeldKey::MouseBtn(btn));
        }
        Ok(())
    }

    fn key(&self, code: KeyCode, down: bool) -> Result<()> {
        let vk = scancode_to_vk(code.0);
        let dir = if down {
            Direction::Press
        } else {
            Direction::Release
        };
        self.inner
            .lock()
            .key(EKey::Other(vk as u32), dir)
            .context("enigo key")?;
        super::note_key_injected_with_code(code.0, down);
        let mut held = self.held.lock();
        if down {
            held.insert(HeldKey::Key(code.0));
        } else {
            held.remove(&HeldKey::Key(code.0));
        }
        Ok(())
    }

    fn release_all_held(&self) -> Result<()> {
        let drained: Vec<HeldKey> = self.held.lock().drain().collect();
        if drained.is_empty() {
            return Ok(());
        }
        let count = drained.len();
        let mut enigo = self.inner.lock();
        for h in drained {
            let res: std::result::Result<(), _> = match h {
                HeldKey::MouseBtn(btn) => {
                    let b = match btn {
                        Button::Left => EButton::Left,
                        Button::Right => EButton::Right,
                        Button::Middle => EButton::Middle,
                        Button::X1 => EButton::Back,
                        Button::X2 => EButton::Forward,
                    };
                    enigo.button(b, Direction::Release)
                }
                HeldKey::Key(code) => {
                    let vk = scancode_to_vk(code);
                    enigo.key(EKey::Other(vk as u32), Direction::Release)
                }
            };
            if let Err(e) = res {
                warn!(error = %e, "failed to release held key on session end");
            }
        }
        info!(
            count,
            "released stale held keys at session end (preventing stuck-key after peer disconnect)"
        );
        Ok(())
    }

    fn scroll(&self, _dx: f32, dy: f32) -> Result<()> {
        // Sign convention mismatch:
        //   * Linux/Win HW: REL_WHEEL / WM_MOUSEWHEEL positive = wheel
        //     rotated forward (away from user) → content scrolls UP.
        //   * enigo's `scroll(length, Vertical)`: positive = scroll
        //     DOWN. (See enigo docs: "positive value means scroll
        //     down/right, negative means scroll up/left".)
        // Without negation, every wheel-up on the controller becomes
        // wheel-down on the Win receiver — exactly the inverted
        // "looking" feel the user reported.
        if dy != 0.0 {
            self.inner
                .lock()
                .scroll(-dy.round() as i32, Axis::Vertical)
                .context("enigo scroll v")?;
        }
        Ok(())
    }
}

/// Linux evdev codes for the "extended" key cluster that do NOT share
/// values with their Windows PS/2 scan-code equivalents.  We use this
/// table in two places:
///   1. `scancode_to_vk`  — Linux→Windows injection: evdev code → VK
///   2. `low_kb_hook`     — Windows→Linux capture: VK → evdev code
///
/// Bidirectional: left column is Linux evdev, right is Windows VK.
const EXTENDED_KEY_TABLE: &[(u16, u16)] = &[
    (103, 0x26), // KEY_UP        ↔ VK_UP
    (108, 0x28), // KEY_DOWN      ↔ VK_DOWN
    (105, 0x25), // KEY_LEFT      ↔ VK_LEFT
    (106, 0x27), // KEY_RIGHT     ↔ VK_RIGHT
    (102, 0x24), // KEY_HOME      ↔ VK_HOME
    (107, 0x23), // KEY_END       ↔ VK_END
    (104, 0x21), // KEY_PAGEUP    ↔ VK_PRIOR
    (109, 0x22), // KEY_PAGEDOWN  ↔ VK_NEXT
    (110, 0x2D), // KEY_INSERT    ↔ VK_INSERT
    (111, 0x2E), // KEY_DELETE    ↔ VK_DELETE
    (125, 0x5B), // KEY_LEFTMETA  ↔ VK_LWIN
    (126, 0x5C), // KEY_RIGHTMETA ↔ VK_RWIN
    (99,  0x2C), // KEY_SYSRQ     ↔ VK_SNAPSHOT (PrintScreen)
];

/// Linux evdev KeyCode → Windows Virtual Key.
/// For common keys the PS/2 scan-code number equals the evdev number,
/// so `MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX)` handles them.
/// Extended keys (arrow cluster, Win key, …) need explicit remapping.
fn scancode_to_vk(scan: u16) -> u16 {
    if let Some(&(_, vk)) = EXTENDED_KEY_TABLE.iter().find(|&&(evdev, _)| evdev == scan) {
        return vk;
    }
    use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK_EX, MapVirtualKeyW};
    unsafe {
        let vk = MapVirtualKeyW(scan as u32, MAPVK_VSC_TO_VK_EX);
        if vk == 0 { scan } else { (vk & 0xFFFF) as u16 }
    }
}

#[allow(dead_code)]
fn _force_vk_use(_v: VIRTUAL_KEY) {}

const _: usize = mem::size_of::<MSLLHOOKSTRUCT>();
