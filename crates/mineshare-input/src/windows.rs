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
#[cfg(test)]
use std::sync::atomic::AtomicI64;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use enigo::{Button as EButton, Direction, Enigo, Key as EKey, Keyboard, Mouse, Settings};
use parking_lot::Mutex;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH, RECT};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThread, GetCurrentThreadId, OpenProcess, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, SetThreadPriority,
    THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GA_ROOT, GetAncestor, GetClipCursor, GetCursorInfo, GetCursorPos, GetMessageW,
    GetSystemMetrics, HC_ACTION, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PostThreadMessageW, RI_KEY_BREAK, RI_KEY_E0, RegisterClassExW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetCursorPos, SetForegroundWindow,
    SetWindowsHookExW, ShowCursor, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_INPUT, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    WM_XBUTTONUP, WNDCLASSEXW, WindowFromPoint,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::{Button, InputCapture, InputEvent, InputInject, KeyCode, TouchpadEvent};

mod touchpad;

/// RAII guard that releases the raised system timer resolution
/// (`timeBeginPeriod(1)`) on drop. Held for the lifetime of the
/// forward watchdog thread so the finer scheduler tick is only in
/// effect while the bridge is actively forwarding motion.
struct HighResTimerGuard;

impl Drop for HighResTimerGuard {
    fn drop(&mut self) {
        unsafe { timeEndPeriod(1) };
    }
}

const MODE_LOCAL: u8 = 0;
const MODE_REMOTE: u8 = 1;

/// True when the capture path should forward + pin motion to the peer:
/// either the cursor has crossed into Remote mode, or Game Drive is
/// actively driving the peer (which forwards continuously without ever
/// entering the cursor-crossing state machine).
fn forwarding_active() -> bool {
    CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE || super::is_game_driving()
}

type EventSink = std::sync::Arc<dyn Fn(InputEvent) + Send + Sync + 'static>;
static EVENT_SINK: OnceLock<Mutex<Option<EventSink>>> = OnceLock::new();
/// The isolated keyboard dispatch thread owns the low-level hook. Each remote
/// focus handoff asks that thread to replace its hook because Windows can
/// silently remove a timed-out hook while leaving the old handle looking valid.
static KEYBOARD_HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
const WM_REFRESH_KEYBOARD_HOOK: u32 = WM_APP + 0x32;
static LAST_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_Y: AtomicI32 = AtomicI32::new(i32::MIN);
static CURSOR_MODE: AtomicU8 = AtomicU8::new(MODE_LOCAL);
/// Number of `ShowCursor(false)` calls made while entering Remote mode.
/// Restoring the exact same count keeps Windows' display counter balanced.
static REMOTE_CURSOR_HIDE_CALLS: AtomicI32 = AtomicI32::new(0);
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
static PEER_H: AtomicI32 = AtomicI32::new(1080);
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
/// Hotkey: Ctrl+Alt+G toggles Game Drive — drive the peer's game with
/// pure-relative mouse + keyboard (no cursor-crossing, warp, or game-lock).
const SCAN_G: u32 = 0x22; // G (set-1 make code)

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

// --- Motion coalescing -----------------------------------------------------
//
// Raw input on Windows can fire at the mouse's polling rate (often 1000 Hz on
// gaming mice). Forwarding one UDP packet per event saturates the peer's
// receive-and-inject loop — each `Enigo::move_mouse` holds a mutex and
// serialises through a single tokio task — so we coalesce: accumulate dx/dy
// into `PENDING_*` and dispatch one combined delta per flush window.
//
// The window is a fidelity-vs-load tradeoff. The original 8 ms (~125 Hz) was
// fine for desktop cursor work but downsampled a 1000 Hz gaming mouse into
// choppy lumps that feel "wrong" / uncontrollable inside GAMES on the
// receiver — the game reads motion (via Raw Input) as a stream of coarse
// 125 Hz jumps instead of smooth high-rate deltas. The window is now a
// runtime value (`super::target_flush_us()`, default 2 ms = 500 Hz) driven
// by the user's mouse-rate setting, so it is tunable 60..=1000 Hz from the
// GUI instead of being a compile-time constant. Forwarding only happens
// while CURSOR_MODE == REMOTE, so this rate applies only when actively
// driving the peer — never during idle local desktop use.
static PENDING_DX: AtomicI32 = AtomicI32::new(0);
static PENDING_DY: AtomicI32 = AtomicI32::new(0);
static LAST_FLUSH_MS: AtomicU64 = AtomicU64::new(0);
static LAST_FLUSH_US: AtomicU64 = AtomicU64::new(0);
/// Absolute monotonic deadline for the currently pending motion window.
/// Zero means no motion is armed. Producers only wake the watchdog when
/// they win the 0 → deadline transition, avoiding one scheduler unpark per
/// raw hardware event.
static NEXT_FLUSH_DEADLINE_US: AtomicU64 = AtomicU64::new(0);
static FLUSH_WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
static FLUSH_WATCHDOG_THREAD: OnceLock<thread::Thread> = OnceLock::new();
static FLUSH_FWD_COUNT: AtomicI32 = AtomicI32::new(0);
#[cfg(test)]
static FLUSH_WATCHDOG_WAKEUPS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static FLUSH_WATCHDOG_UNPARKS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static FLUSH_FWD_DISTANCE_X: AtomicI64 = AtomicI64::new(0);

/// Idle window after which a negative `VIRT_X` drift is snapped back to 0
/// by the watchdog safety net.
const REMOTE_IDLE_SNAP_MS: u64 = 1_000;

/// Returns true when accumulated negative `VIRT_X` drift should be snapped
/// back to 0: it has gone negative AND the mouse has been idle (no flush)
/// for at least `REMOTE_IDLE_SNAP_MS`. `last_flush` of 0 means "never
/// flushed" and must NOT trigger a snap.
///
/// Behavioral edge: a user who drags partway back toward the exit threshold
/// and then PAUSES (>= REMOTE_IDLE_SNAP_MS) has their negative progress
/// reset — intended; a pause is treated as "exit not committed", while a
/// continuous backward drag still crosses the threshold and exits correctly.
fn should_snap_virt_x(virt_x: i32, last_flush: u64, now: u64) -> bool {
    virt_x < 0 && last_flush != 0 && now.saturating_sub(last_flush) >= REMOTE_IDLE_SNAP_MS
}

fn monotonic_us() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    // Reserve zero as the "not armed / never flushed" sentinel.
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros()
        .min((u64::MAX - 1) as u128) as u64
        + 1
}

/// Atomically drain `PENDING_DX/DY` and forward the combined delta.
/// Skips no-op events when both axes are zero.
fn flush_pending_motion(now_us: u64) {
    let dx = PENDING_DX.swap(0, Ordering::AcqRel);
    let dy = PENDING_DY.swap(0, Ordering::AcqRel);
    if dx == 0 && dy == 0 {
        return;
    }
    LAST_FLUSH_US.store(now_us, Ordering::Release);
    LAST_FLUSH_MS.store(super::now_ms(), Ordering::Release);
    #[cfg(test)]
    FLUSH_FWD_DISTANCE_X.fetch_add(dx as i64, Ordering::Relaxed);
    sink_send(InputEvent::MouseMove { dx, dy });
    super::bump_fwd_events();
    let n = FLUSH_FWD_COUNT.fetch_add(1, Ordering::Relaxed);
    if n % 100 == 0 {
        debug!(dx, dy, n, "win coalesced motion forward");
    }
}

fn pending_motion_exists() -> bool {
    PENDING_DX.load(Ordering::Acquire) != 0 || PENDING_DY.load(Ordering::Acquire) != 0
}

/// Arm one absolute deadline for the current pending window. Returns true
/// only for the producer that changed the state from idle to armed.
fn arm_pending_motion(now_us: u64) -> bool {
    if !pending_motion_exists() || NEXT_FLUSH_DEADLINE_US.load(Ordering::Acquire) != 0 {
        return false;
    }
    let flush_us = super::target_flush_us();
    let last = LAST_FLUSH_US.load(Ordering::Acquire);
    let deadline = if last == 0 || now_us.saturating_sub(last) >= flush_us {
        now_us
    } else {
        last.saturating_add(flush_us)
    };
    NEXT_FLUSH_DEADLINE_US
        .compare_exchange(0, deadline, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn wake_motion_flush_watchdog() {
    if let Some(thread) = FLUSH_WATCHDOG_THREAD.get() {
        #[cfg(test)]
        FLUSH_WATCHDOG_UNPARKS.fetch_add(1, Ordering::Relaxed);
        thread.unpark();
    }
}

/// Flush when the absolute deadline is due. The CAS ensures the raw-input
/// callback and watchdog can race without double-dispatching. A producer
/// arriving during the drain is re-armed after the swap, preventing a final
/// fragment from being stranded.
fn flush_pending_motion_if_due(now_us: u64) -> bool {
    let deadline = NEXT_FLUSH_DEADLINE_US.load(Ordering::Acquire);
    if deadline == 0 || now_us < deadline {
        return false;
    }
    if NEXT_FLUSH_DEADLINE_US
        .compare_exchange(deadline, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }

    flush_pending_motion(now_us);
    let rearm_now = monotonic_us();
    if arm_pending_motion(rearm_now) {
        wake_motion_flush_watchdog();
    }
    true
}

fn queue_pending_motion(dx: i32, dy: i32) {
    if dx == 0 && dy == 0 {
        return;
    }
    PENDING_DX.fetch_add(dx, Ordering::AcqRel);
    PENDING_DY.fetch_add(dy, Ordering::AcqRel);

    let now_us = monotonic_us();
    let newly_armed = arm_pending_motion(now_us);
    let flushed = flush_pending_motion_if_due(now_us);
    if newly_armed && !flushed {
        wake_motion_flush_watchdog();
    }
}

fn remote_motion_axes(side: super::PeerSide, dx: i32, dy: i32) -> (i32, i32) {
    match side {
        super::PeerSide::Right => (dx, dy),
        super::PeerSide::Left => (-dx, dy),
        super::PeerSide::Top => (-dy, dx),
        super::PeerSide::Bottom => (dy, dx),
    }
}

fn peer_depth_extent(side: super::PeerSide) -> i32 {
    match side {
        super::PeerSide::Right | super::PeerSide::Left => PEER_W.load(Ordering::Relaxed),
        super::PeerSide::Top | super::PeerSide::Bottom => PEER_H.load(Ordering::Relaxed),
    }
    .max(1)
}

/// Advance the source-side virtual cursor by the exact relative delta that is
/// sent to the peer. Keeping edge detection in the same coordinate stream as
/// peer injection prevents DPI, acceleration, and anchor-warp differences
/// from making a small reverse movement look like a complete edge crossing.
fn advance_remote_virtual_cursor(dx: i32, dy: i32) -> i32 {
    let side = super::peer_side();
    let (depth_delta, lateral_delta) = remote_motion_axes(side, dx, dy);
    let peer_extent = peer_depth_extent(side);
    let next_depth = VIRT_X
        .load(Ordering::Relaxed)
        .saturating_add(depth_delta)
        .clamp(-EXIT_BUFFER_PX, peer_extent - 1);
    VIRT_X.store(next_depth, Ordering::Relaxed);
    VIRT_Y.fetch_add(lateral_delta, Ordering::Relaxed);
    next_depth
}

/// Keep the Raw Input route aligned with the cursor displacement visibly
/// applied on the peer. Game Drive forwards motion without edge switching, so
/// it deliberately bypasses this cursor-crossing state machine.
fn track_raw_remote_motion(dx: i32, dy: i32) -> bool {
    CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE
        && advance_remote_virtual_cursor(dx, dy) <= -EXIT_BUFFER_PX
}

/// Spawn the event-driven flush thread once. It parks with zero polling while
/// input stays local, then uses the configured high-resolution flush window
/// only while motion is actively being forwarded. A final short movement still
/// wakes the thread, so residual deltas cannot sit in the accumulator.
fn start_motion_flush_watchdog() {
    if FLUSH_WATCHDOG_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(e) = thread::Builder::new()
        .name("win-motion-flush".into())
        .spawn(|| {
            let _ = FLUSH_WATCHDOG_THREAD.set(thread::current());
            loop {
                while !forwarding_active() {
                    thread::park();
                    #[cfg(test)]
                    FLUSH_WATCHDOG_WAKEUPS.fetch_add(1, Ordering::Relaxed);
                }

                // The default Windows scheduler tick is ~15.6 ms. Raise it
                // only for an active remote-driving interval, then release it
                // as soon as control returns local.
                unsafe { timeBeginPeriod(1) };
                let timer_guard = HighResTimerGuard;
                while forwarding_active() {
                    let now_us = monotonic_us();
                    let deadline = NEXT_FLUSH_DEADLINE_US.load(Ordering::Acquire);
                    let wait = if deadline != 0 {
                        std::time::Duration::from_micros(deadline.saturating_sub(now_us))
                    } else {
                        std::time::Duration::from_millis(REMOTE_IDLE_SNAP_MS)
                    };
                    thread::park_timeout(wait);
                    #[cfg(test)]
                    FLUSH_WATCHDOG_WAKEUPS.fetch_add(1, Ordering::Relaxed);

                    if !forwarding_active() {
                        break;
                    }
                    let now_us = monotonic_us();
                    flush_pending_motion_if_due(now_us);
                    let now = super::now_ms();
                    let last = LAST_FLUSH_MS.load(Ordering::Relaxed);
                    // Safety net: if VIRT_X has drifted negative while the
                    // mouse has been idle, snap it back to zero.
                    if should_snap_virt_x(VIRT_X.load(Ordering::Relaxed), last, now) {
                        VIRT_X.store(0, Ordering::Relaxed);
                    }
                }
                drop(timer_guard);
            }
        })
    {
        warn!(error = %e, "failed to spawn motion flush watchdog");
        FLUSH_WATCHDOG_STARTED.store(false, Ordering::Release);
    }
}

/// Per-event delta cap used in the WH_MOUSE_LL fallback path. The fallback
/// fires whenever raw input goes stale for >`RAW_INPUT_STALE_MS`, which on
/// real hardware happens at **motion onset** after each pause — the user
/// stops aiming for a moment, then flicks. The first event(s) of that
/// flick come through the hook before raw input resumes.
///
/// The cap was originally 30 px to prevent a single coalesced burst from
/// teleporting the peer cursor. That value is fine for desktop cursor work
/// but **brutally clips game flicks**: an observed hook event of -964 px
/// (a fast aim swing) got clamped to -30 px, throwing away 97% of the
/// movement. The user experiences this as a micro-stutter / dead zone at
/// the start of every aim swing — *not* a "low rate" problem (raw input is
/// firing fine during continuous motion), just the first event of each
/// burst getting truncated.
///
/// 1500 px allows realistic fast flicks (~94000 px/s at 60 Hz hook cadence)
/// while still preventing pathologically large teleports from genuine
/// driver glitches. A real mouse can't physically produce a single 60-Hz
/// delta beyond a few thousand pixels.
const MAX_DELTA_PX: i32 = 1500;

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

pub fn set_peer_screen(w: u32, h: u32) {
    PEER_W.store(w.max(1) as i32, Ordering::Relaxed);
    PEER_H.store(h.max(1) as i32, Ordering::Relaxed);
    info!(peer_w = w, peer_h = h, "peer screen geometry stored");
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

fn set_touchpad_gesture_capture(active: bool) {
    touchpad::set_capture_active(active);
}

fn hide_source_cursor() {
    if REMOTE_CURSOR_HIDE_CALLS.load(Ordering::Acquire) != 0 {
        return;
    }
    let mut calls = 0;
    while calls < 32 {
        let display_count = unsafe { ShowCursor(false) };
        calls += 1;
        if display_count < 0 {
            break;
        }
    }
    REMOTE_CURSOR_HIDE_CALLS.store(calls, Ordering::Release);
}

fn restore_source_cursor() {
    let calls = REMOTE_CURSOR_HIDE_CALLS.swap(0, Ordering::AcqRel);
    for _ in 0..calls {
        unsafe {
            ShowCursor(true);
        }
    }
}

pub fn touchpad_capabilities() -> u32 {
    touchpad::capabilities()
}

fn enter_remote(entry_lateral: i32) {
    // Refuse if the peer signalled it's already driving Remote — otherwise
    // both ends forward each other's HW input simultaneously and we end
    // up with cursors fighting on both screens.
    if super::peer_in_remote() {
        debug!("enter_remote refused — peer holds Remote");
        return;
    }
    let (ax, ay) = anchor();
    VIRT_X.store(0, Ordering::Relaxed);
    VIRT_Y.store(entry_lateral, Ordering::Relaxed);
    unsafe {
        let _ = SetCursorPos(ax, ay);
    }
    LAST_X.store(ax, Ordering::Relaxed);
    LAST_Y.store(ay, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_REMOTE, Ordering::Release);
    request_keyboard_hook_refresh();
    touchpad::set_capture_anchor(ax, ay);
    set_touchpad_gesture_capture(true);
    hide_source_cursor();
    wake_motion_flush_watchdog();
    info!(
        entry_lateral,
        anchor = ?(ax, ay),
        source_cursor_hide_calls = REMOTE_CURSOR_HIDE_CALLS.load(Ordering::Acquire),
        "cursor → remote"
    );
    super::fire_remote_event(super::RemoteEvent::Entered);
}

fn exit_remote(restore_lateral: i32) {
    let (left, top, right, bottom) = bounds();
    // Restore the local cursor to the edge that faces the peer
    // (per the configured layout). User came back across that
    // edge to leave Remote, so dropping the OS cursor there
    // matches their hand position on the desk. For top/bottom
    // we keep their horizontal anchor, just snap Y to the edge.
    let (restore_x, restore_y) = match super::peer_side() {
        super::PeerSide::Right => (right, restore_lateral.clamp(top, bottom)),
        super::PeerSide::Left => (left, restore_lateral.clamp(top, bottom)),
        super::PeerSide::Top => (restore_lateral.clamp(left, right), top),
        super::PeerSide::Bottom => (restore_lateral.clamp(left, right), bottom),
    };
    unsafe {
        let _ = SetCursorPos(restore_x, restore_y);
    }
    LAST_X.store(restore_x, Ordering::Relaxed);
    LAST_Y.store(restore_y, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
    set_touchpad_gesture_capture(false);
    restore_source_cursor();
    wake_motion_flush_watchdog();
    info!(restore = ?(restore_x, restore_y), "cursor → local");
    super::fire_remote_event(super::RemoteEvent::Exited);
}

fn crossed_peer_edge(
    side: super::PeerSide,
    bounds: (i32, i32, i32, i32),
    previous: (i32, i32),
    current: (i32, i32),
) -> bool {
    let (left, top, right, bottom) = bounds;
    let (last_x, last_y) = previous;
    let (x, y) = current;
    last_x != i32::MIN
        && match side {
            super::PeerSide::Right => last_x < right && x >= right,
            super::PeerSide::Left => last_x > left && x <= left,
            super::PeerSide::Top => last_y > top && y <= top,
            super::PeerSide::Bottom => last_y < bottom && y >= bottom,
        }
}

/// Handle genuine local motion outside the latency-critical low-level hook.
///
/// Raw Input calls this only after Windows has released the original input
/// event. The hook calls it solely as a fallback when Raw Input registration
/// is unavailable.
fn process_local_mouse_motion(x: i32, y: i32, track_activity: bool) {
    if track_activity {
        super::bump_local_mouse_activity();
    }

    let last_x = LAST_X.load(Ordering::Relaxed);
    let last_y = LAST_Y.load(Ordering::Relaxed);
    if super::peer_in_remote()
        && last_x != i32::MIN
        && (x - last_x).abs() + (y - last_y).abs() > 5
        && super::reclaim_from_peer_hardware()
    {
        super::fire_remote_event(super::RemoteEvent::RequestPeerExit);
    }

    LAST_X.store(x, Ordering::Relaxed);
    LAST_Y.store(y, Ordering::Relaxed);

    if crossed_peer_edge(super::peer_side(), bounds(), (last_x, last_y), (x, y))
        && !super::is_input_locked()
        && super::game_drive() == super::GameDrive::Off
    {
        let entry_lateral = match super::peer_side() {
            super::PeerSide::Right | super::PeerSide::Left => y,
            super::PeerSide::Top | super::PeerSide::Bottom => x,
        };
        enter_remote(entry_lateral);
    }
}

pub fn local_in_remote() -> bool {
    CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE
}

pub(super) fn reset_forwarded_keyboard_latches() {
    touchpad::reset_forwarded_keyboard_latches();
}

pub fn force_exit_remote() {
    if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
        let (left, top, right, bottom) = bounds();
        let restore_lateral = match super::peer_side() {
            super::PeerSide::Right | super::PeerSide::Left => (top + bottom) / 2,
            super::PeerSide::Top | super::PeerSide::Bottom => (left + right) / 2,
        };
        info!("force_exit_remote — peer asked us to release");
        exit_remote(restore_lateral);
    }
}

/// Called when the peer signals it has taken Remote control of us.
/// Warps the local cursor to the boundary edge facing the peer (per
/// the configured layout) so the peer's virt_x model matches the
/// real cursor position — without this the peer's exit threshold
/// fires after a tiny motion in the wrong direction even though the
/// cursor is mid-screen.
/// Set every poll by `game_detect_thread`: true when a fullscreen /
/// cursor-confine / anti-cheat title currently owns the cursor.
static GAME_FOREGROUND: AtomicBool = AtomicBool::new(false);

/// A peer handoff also transfers Windows' foreground window. The transition
/// starts synchronously with TakeControl; mouse motion and the first key are
/// retries only when Windows temporarily rejects that first activation.
struct FocusHandoff {
    pending: AtomicBool,
    key_pending: AtomicBool,
}

impl FocusHandoff {
    const fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            key_pending: AtomicBool::new(false),
        }
    }

    fn arm(&self) {
        self.pending.store(true, Ordering::Release);
        self.key_pending.store(true, Ordering::Release);
    }

    fn try_begin(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }

    fn try_begin_key(&self) -> bool {
        self.key_pending.swap(false, Ordering::AcqRel)
    }

    fn finish(&self, activated: bool) {
        if !activated {
            self.pending.store(true, Ordering::Release);
        }
    }

    fn finish_key(&self, activated: bool) {
        if !activated {
            self.key_pending.store(true, Ordering::Release);
        }
    }
}

static REMOTE_FOCUS_HANDOFF: FocusHandoff = FocusHandoff::new();

fn focus_point_is_safe(point: (i32, i32), screen_bounds: (i32, i32, i32, i32)) -> bool {
    const EDGE_INSET_PX: i32 = 8;
    let (left, top, right, bottom) = screen_bounds;
    point.0 >= left.saturating_add(EDGE_INSET_PX)
        && point.0 <= right.saturating_sub(EDGE_INSET_PX)
        && point.1 >= top.saturating_add(EDGE_INSET_PX)
        && point.1 <= bottom.saturating_sub(EDGE_INSET_PX)
}

/// Activate the top-level window under the cursor without synthesizing a
/// click. Temporarily joining its input queue makes SetForegroundWindow
/// reliable despite Windows' foreground-stealing restrictions.
fn activate_window_under_cursor() -> Result<bool> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.context("GetCursorPos for focus handoff")?;
    if !focus_point_is_safe((point.x, point.y), bounds()) {
        return Ok(false);
    }
    let child = unsafe { WindowFromPoint(point) };
    if child.0.is_null() {
        return Ok(false);
    }
    let target = unsafe { GetAncestor(child, GA_ROOT) };
    if target.0.is_null() || target == unsafe { GetForegroundWindow() } {
        return Ok(!target.0.is_null());
    }

    let target_thread = unsafe { GetWindowThreadProcessId(target, None) };
    let current_thread = unsafe { GetCurrentThreadId() };
    let attached = target_thread != 0
        && target_thread != current_thread
        && unsafe { AttachThreadInput(current_thread, target_thread, true) }.as_bool();
    let activated = unsafe { SetForegroundWindow(target) }.as_bool();
    if attached {
        let _ = unsafe { AttachThreadInput(current_thread, target_thread, false) };
    }
    Ok(activated || target == unsafe { GetForegroundWindow() })
}

fn activate_remote_focus_checkpoint(reason: &'static str) -> bool {
    let started = Instant::now();
    match activate_window_under_cursor() {
        Ok(true) => {
            info!(
                reason,
                elapsed_us = started.elapsed().as_micros() as u64,
                "remote focus activated window under cursor without click"
            );
            true
        }
        Ok(false) => {
            debug!(
                reason,
                elapsed_us = started.elapsed().as_micros() as u64,
                "remote focus handoff found no activatable window"
            );
            false
        }
        Err(error) => {
            debug!(
                %error,
                reason,
                elapsed_us = started.elapsed().as_micros() as u64,
                "remote focus handoff failed"
            );
            false
        }
    }
}

fn attempt_remote_focus(reason: &'static str) {
    if !REMOTE_FOCUS_HANDOFF.try_begin() {
        return;
    }
    let activated = activate_remote_focus_checkpoint(reason);
    REMOTE_FOCUS_HANDOFF.finish(activated);
}

fn attempt_remote_key_focus() {
    if !REMOTE_FOCUS_HANDOFF.try_begin_key() {
        return;
    }
    let activated = activate_remote_focus_checkpoint("first-key-checkpoint");
    REMOTE_FOCUS_HANDOFF.finish_key(activated);
}

pub(crate) fn game_foreground() -> bool {
    GAME_FOREGROUND.load(Ordering::Relaxed)
}

#[derive(Debug, PartialEq, Eq)]
enum TakeControlAction {
    /// Warp the cursor to this screen point (normal desktop edge crossing,
    /// so the peer's `virt_x` model matches the real cursor position).
    Warp(i32, i32),
    /// A cursor-locked game owns the cursor — leave it where it is and only
    /// anchor `LAST_X/Y` here. Warping would start a `SetCursorPos`
    /// tug-of-war with the game's per-frame recenter, flinging a mouse-look
    /// camera (Roblox LockCenter). Injection stays pure-relative.
    Anchor(i32, i32),
}

/// Skip the warp only when a cursor-locked game owns the cursor. The
/// game-drive path no longer relies on this (it never warps), so do NOT
/// anchor for every peer-driven session — desktop crossing needs the warp.
fn should_anchor_cursor(game_foreground: bool, _peer_in_remote: bool) -> bool {
    game_foreground
}

/// Pure decision: where the local cursor should go when the peer takes
/// control. Split out so the warp-vs-anchor choice is unit-testable
/// without OS calls.
fn take_control_action(
    anchor_in_place: bool,
    side: super::PeerSide,
    bounds: (i32, i32, i32, i32),
    cur: (i32, i32),
) -> TakeControlAction {
    if anchor_in_place {
        return TakeControlAction::Anchor(cur.0, cur.1);
    }
    let (left, top, right, bottom) = bounds;
    let mid_x = (left + right) / 2;
    let mid_y = (top + bottom) / 2;
    let (x, y) = match side {
        super::PeerSide::Right => (right, mid_y),
        super::PeerSide::Left => (left, mid_y),
        super::PeerSide::Top => (mid_x, top),
        super::PeerSide::Bottom => (mid_x, bottom),
    };
    TakeControlAction::Warp(x, y)
}

pub fn on_peer_take_control() {
    REMOTE_FOCUS_HANDOFF.arm();
    let mut cur = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cur);
    }
    let anchor = should_anchor_cursor(game_foreground(), super::peer_in_remote());
    match take_control_action(anchor, super::peer_side(), bounds(), (cur.x, cur.y)) {
        TakeControlAction::Warp(x, y) => {
            unsafe {
                let _ = SetCursorPos(x, y);
            }
            // Update the hook's "last seen" so HW-motion auto-release doesn't
            // mis-fire on the first injected motion arriving from the peer.
            LAST_X.store(x, Ordering::Relaxed);
            LAST_Y.store(y, Ordering::Relaxed);
            info!(boundary = ?(x, y), side = ?super::peer_side(), "warped cursor to peer-facing edge");
        }
        TakeControlAction::Anchor(x, y) => {
            // Cursor-locked / anti-cheat game in foreground: skip the edge
            // warp so we don't fight the game's recenter (camera fling).
            LAST_X.store(x, Ordering::Relaxed);
            LAST_Y.store(y, Ordering::Relaxed);
            info!(at = ?(x, y), "peer take-control: cursor-locked game foreground — skipping edge warp");
        }
    }
    // Foreground ownership is part of the control handoff, not a side effect
    // of later mouse travel. This makes shortcuts and typing valid as soon as
    // the peer observes TakeControl.
    attempt_remote_focus("take-control");
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

        launch_isolated_hook_workers(
            || unsafe { mouse_hook_thread() },
            || unsafe { keyboard_hook_thread() },
        )
        .context("spawn isolated Windows hook threads")?;

        if let Err(e) = launch_touchpad_capture_worker(|| {
            if let Err(e) = touchpad::run_capture_loop() {
                warn!(error = %e, "native Precision Touchpad capture unavailable");
            }
        }) {
            warn!(error = %e, "failed to launch native touchpad capture worker");
        }

        // 8ms motion coalescer: batches raw input + hook fallback events
        // into one MouseMove every ~8ms so the peer's inject loop isn't
        // overwhelmed by a 1000 Hz mouse. Same pattern as linux.rs.
        start_motion_flush_watchdog();

        if let Err(e) = thread::Builder::new()
            .name("win-raw-input".into())
            .spawn(|| unsafe { raw_input_thread() })
        {
            warn!(error = %e, "failed to spawn raw-input worker");
        }

        // Capability bits are part of the lockstep session handshake. Wait a
        // bounded interval for the touchpad worker's Win32/WinRT probe so the
        // first peer connection does not race startup and incorrectly
        // advertise basic fallback for an available native touchpad.
        touchpad::wait_for_capture_setup(std::time::Duration::from_millis(500));

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
            .spawn(game_detect_thread)
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
        let res = QueryFullProcessImageNameW(
            proc,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
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

fn should_auto_lock_game(
    _cursor_hidden: bool,
    cursor_clipped: bool,
    anticheat_match: bool,
    game_drive_off: bool,
) -> bool {
    // Windows commonly hides the pointer while the user types. Treating that
    // transient UI state as a game caused the bridge to toggle its global
    // input lock every time typing began or ended. Cursor confinement and the
    // explicit risky-process list are stronger, stable game signals.
    (cursor_clipped || anticheat_match) && game_drive_off
}

/// Polls cursor visibility + clip-rect + foreground-process state
/// every 250 ms. Auto-engages the input lock when:
///   * the cursor clip rect is smaller than the screen (FPS
///     mouse-confine)
///   * OR the foreground process matches the anti-cheat-protected
///     `RISKY_GAMES` list — even if cursor is visible (main menu)
///
/// Cursor visibility is retained for diagnostics, but is deliberately not a
/// lock signal: Windows hides the pointer during ordinary keyboard input.
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
        let cursor_hidden =
            unsafe { GetCursorInfo(&mut ci) }.is_ok() && (ci.flags.0 & CURSOR_SHOWING.0) == 0;

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

        let should_lock = should_auto_lock_game(
            cursor_hidden,
            cursor_clipped,
            anticheat_match.is_some(),
            super::game_drive() == super::GameDrive::Off,
        );
        // Publish for `on_peer_take_control`: when a cursor-locked game owns
        // the cursor we must NOT warp it to the edge (camera fling).
        GAME_FOREGROUND.store(should_lock, Ordering::Relaxed);
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

fn launch_touchpad_capture_worker<F>(worker: F) -> std::io::Result<()>
where
    F: FnOnce() + Send + 'static,
{
    thread::Builder::new()
        .name("win-touchpad-capture".into())
        .spawn(worker)
        .map(|_| ())
}

fn launch_isolated_hook_workers<FM, FK>(
    mouse_worker: FM,
    keyboard_worker: FK,
) -> std::io::Result<()>
where
    FM: FnOnce() + Send + 'static,
    FK: FnOnce() + Send + 'static,
{
    thread::Builder::new()
        .name("win-keyboard-hook".into())
        .spawn(keyboard_worker)?;
    thread::Builder::new()
        .name("win-mouse-hook".into())
        .spawn(mouse_worker)?;
    Ok(())
}

unsafe fn pump_hook_messages() {
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn keyboard_hook_thread() {
    let mut keyboard =
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_kb_hook), None, 0) } {
            Ok(handle) if !handle.0.is_null() => handle,
            result => {
                warn!(
                    ?result,
                    "WH_KEYBOARD_LL installation failed (need GUI session)"
                );
                return;
            }
        };
    KEYBOARD_HOOK_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    info!("Windows keyboard hook installed on isolated dispatch thread");

    let mut msg = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if result.0 == 0 || result.0 == -1 {
            break;
        }
        if msg.message == WM_REFRESH_KEYBOARD_HOOK {
            match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_kb_hook), None, 0) } {
                Ok(replacement) if !replacement.0.is_null() => {
                    let previous = std::mem::replace(&mut keyboard, replacement);
                    let _ = unsafe { UnhookWindowsHookEx(previous) };
                    info!("keyboard shortcut hook refreshed for remote focus handoff");
                }
                result => warn!(?result, "keyboard shortcut hook refresh failed"),
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    KEYBOARD_HOOK_THREAD_ID.store(0, Ordering::Release);
    let _ = unsafe { UnhookWindowsHookEx(keyboard) };
}

fn request_keyboard_hook_refresh() {
    let thread_id = KEYBOARD_HOOK_THREAD_ID.load(Ordering::Acquire);
    if thread_id == 0 {
        warn!("keyboard hook thread unavailable during remote focus handoff");
        return;
    }
    if let Err(error) =
        unsafe { PostThreadMessageW(thread_id, WM_REFRESH_KEYBOARD_HOOK, WPARAM(0), LPARAM(0)) }
    {
        warn!(%error, "could not queue keyboard shortcut hook refresh");
    }
}

unsafe fn mouse_hook_thread() {
    if std::env::var_os("MINESHARE_DISABLE_MOUSE_HOOK").is_some() {
        info!("diagnostic mode: WH_MOUSE_LL disabled");
        return;
    }
    let mouse = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_mouse_hook), None, 0) } {
        Ok(handle) if !handle.0.is_null() => handle,
        result => {
            warn!(
                ?result,
                "WH_MOUSE_LL installation failed (need GUI session)"
            );
            return;
        }
    };
    info!("Windows mouse hook installed on isolated dispatch thread");
    unsafe { pump_hook_messages() };
    let _ = unsafe { UnhookWindowsHookEx(mouse) };
}

unsafe fn raw_input_thread() {
    if let Err(error) = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) } {
        warn!(%error, "could not raise raw-input thread priority");
    }

    // Create a message-only window so we can receive WM_INPUT via
    // RIDEV_INPUTSINK. Mouse reports provide pre-acceleration mickeys, while
    // keyboard reports provide the event-driven fallback for shell-reserved
    // shortcuts whose legacy key-down is not posted to the capture window.
    if let Some(hwnd) = create_raw_input_window() {
        let devices = [
            RAWINPUTDEVICE {
                usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
                usUsage: 0x02,     // HID_USAGE_GENERIC_MOUSE
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
                usUsage: 0x06,     // HID_USAGE_GENERIC_KEYBOARD
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
        ];
        match unsafe {
            RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32)
        } {
            Ok(()) => {
                USING_RAW_INPUT.store(true, Ordering::Release);
                touchpad::set_raw_keyboard_available(true);
                info!("raw mouse and keyboard input registered (WM_INPUT / RIDEV_INPUTSINK)");
            }
            Err(e) => {
                touchpad::set_raw_keyboard_available(false);
                warn!(
                    ?e,
                    "RegisterRawInputDevices failed — using hook/timer input fallbacks"
                );
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
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
        .ok()
    }
}

/// Process one WM_INPUT message: route the latency-critical keyboard chord or
/// extract raw mouse deltas and forward them while in REMOTE mode.
///
/// Because these are pre-acceleration hardware mickeys (same unit as
/// Linux evdev REL_X/REL_Y), the peer OS applies its own pointer
/// acceleration naturally — giving the same 1:1 "natural mouse" feel
/// regardless of whether the peer is Windows or Linux.
unsafe fn handle_raw_input(h: HRAWINPUT) {
    if !USING_RAW_INPUT.load(Ordering::Relaxed) {
        return;
    }

    // Mouse and keyboard RAWINPUT payloads are fixed-size. Read directly into
    // an aligned stack slot: this removes one user32 call plus a heap
    // allocation from every hardware report on the sub-millisecond path.
    let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let mut raw = std::mem::MaybeUninit::<RAWINPUT>::uninit();
    let written = unsafe {
        GetRawInputData(
            h,
            RID_INPUT,
            Some(raw.as_mut_ptr().cast()),
            &mut size,
            header_size,
        )
    };
    if written == u32::MAX || written == 0 {
        return;
    }

    let raw = unsafe { raw.assume_init_ref() };
    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        let keyboard = unsafe { &raw.data.keyboard };
        let down = u32::from(keyboard.Flags) & RI_KEY_BREAK == 0;
        let extended = u32::from(keyboard.Flags) & RI_KEY_E0 != 0;
        touchpad::handle_raw_keyboard(
            u32::from(keyboard.MakeCode),
            u32::from(keyboard.VKey),
            extended,
            down,
        );
        return;
    }

    if raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }

    let mouse = unsafe { &raw.data.mouse };
    // Skip absolute-position events (touch digitiser, graphics tablet…).
    if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 {
        return;
    }

    let dx = mouse.lLastX;
    let dy = mouse.lLastY;
    if dx == 0 && dy == 0 {
        return;
    }

    // Record Raw Input liveness on both local and remote motion. The local
    // samples immediately before an edge crossing let the hook know WM_INPUT
    // owns depth tracking before the first remote callback arrives.
    LAST_RAW_INPUT_MS.store(super::now_ms(), Ordering::Release);

    // Activity bookkeeping reads the wall clock and updates several atomics.
    // Keep it off WH_MOUSE_LL's latency-critical callback: WM_INPUT is
    // delivered after Windows has released the original input event, so this
    // work cannot stall the touchpad/keyboard system hook chain.
    if !forwarding_active() {
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
            process_local_mouse_motion(cursor.x, cursor.y, true);
        }
        return;
    }

    // Apply user sensitivity multiplier with sub-pixel residue.
    let mut rx = f32::from_bits(SENS_RESIDUE_X.load(Ordering::Relaxed));
    let mut ry = f32::from_bits(SENS_RESIDUE_Y.load(Ordering::Relaxed));
    let sdx = super::scale_delta(dx, &mut rx);
    let sdy = super::scale_delta(dy, &mut ry);
    SENS_RESIDUE_X.store(rx.to_bits(), Ordering::Relaxed);
    SENS_RESIDUE_Y.store(ry.to_bits(), Ordering::Relaxed);

    static RAW_FWD: AtomicI32 = AtomicI32::new(0);
    let n = RAW_FWD.fetch_add(1, Ordering::Relaxed);
    if n % 500 == 0 {
        debug!(
            raw_dx = dx,
            raw_dy = dy,
            sdx,
            sdy,
            n,
            "sample raw motion captured"
        );
    }

    // Coalesce into the 8ms accumulator instead of firing one packet
    // per HW event.  The flush watchdog (or opportunistic flush below)
    // dispatches the combined delta — keeps event rate at ~125 Hz so
    // the peer's inject loop stays responsive even with a 1000 Hz mouse.
    if track_raw_remote_motion(sdx, sdy) {
        exit_remote(VIRT_Y.load(Ordering::Relaxed));
        return;
    }
    queue_pending_motion(sdx, sdy);
}

const LLMHF_INJECTED: u32 = 0x00000001;
const LLMHF_LOWER_IL_INJECTED: u32 = 0x00000002;
const MINESHARE_INPUT_TAG: usize = 0x4D53_4852; // "MSHR"
const LLKHF_EXTENDED: u32 = 0x00000001;
const LLKHF_INJECTED: u32 = 0x00000010;
const LLKHF_LOWER_IL_INJECTED: u32 = 0x00000002;

unsafe extern "system" fn low_mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    let message = wparam.0 as u32;
    let injected = info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0;
    let system_touchpad_wheel = injected
        && info.dwExtraInfo != MINESHARE_INPUT_TAG
        && matches!(message, WM_MOUSEWHEEL | WM_MOUSEHWHEEL)
        && forwarding_active();
    if injected && !system_touchpad_wheel {
        // When the peer is driving us (peer_in_remote=true), track where
        // its injected moves have put our cursor.  Without this update,
        // LAST_X/LAST_Y stay frozen at the boundary-entry point set by
        // `on_peer_take_control`, so the first real local HW event
        // (e.g. a laptop touchpad brush while the peer is driving)
        // computes delta vs the stale boundary rather than vs the actual
        // current cursor position — a tiny touchpad nudge appears as
        // hundreds of pixels and fires a spurious `RequestPeerExit` that
        // bounces the peer's cursor back to its own screen.
        if message == WM_MOUSEMOVE && super::peer_in_remote() {
            LAST_X.store(info.pt.x, Ordering::Relaxed);
            LAST_Y.store(info.pt.y, Ordering::Relaxed);
        }
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let mode = CURSOR_MODE.load(Ordering::Acquire);
    // Ordinary local motion is handled from WM_INPUT after Windows releases
    // this global hook chain. Return immediately here so touchpad events never
    // wait on MineShare's activity, ownership or edge-transition logic.
    if wparam.0 as u32 == WM_MOUSEMOVE
        && mode == MODE_LOCAL
        && !forwarding_active()
        && USING_RAW_INPUT.load(Ordering::Acquire)
    {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let x = info.pt.x;
    let y = info.pt.y;
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
            if mode == MODE_LOCAL {
                // Raw Input normally owns local-motion bookkeeping. Reaching
                // this branch means registration failed, so preserve the old
                // hook-based behavior as a compatibility fallback.
                process_local_mouse_motion(x, y, true);
            } else {
                // REMOTE: Raw Input owns both forwarding and virtual-depth
                // tracking while it is live. Counting the hook's accelerated
                // screen-space delta as well would mix coordinate streams and
                // can make a small reverse movement look like an edge exit.
                let dx = x - last_x;
                let dy = y - last_y;
                let (ax, ay) = anchor();
                let last_raw = LAST_RAW_INPUT_MS.load(Ordering::Acquire);
                let raw_is_live =
                    last_raw != 0 && super::now_ms().saturating_sub(last_raw) < RAW_INPUT_STALE_MS;
                let mut exited = false;
                if !raw_is_live {
                    // Raw Input registration can succeed on Windows builds
                    // where WM_INPUT later stops arriving. In that case this
                    // fallback becomes the sole forwarding + depth source.
                    let mut rx = f32::from_bits(SENS_RESIDUE_X.load(Ordering::Relaxed));
                    let mut ry = f32::from_bits(SENS_RESIDUE_Y.load(Ordering::Relaxed));
                    let scaled_dx = super::scale_delta(dx, &mut rx);
                    let scaled_dy = super::scale_delta(dy, &mut ry);
                    SENS_RESIDUE_X.store(rx.to_bits(), Ordering::Relaxed);
                    SENS_RESIDUE_Y.store(ry.to_bits(), Ordering::Relaxed);
                    let fdx = scaled_dx.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
                    let fdy = scaled_dy.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
                    if fdx != 0 || fdy != 0 {
                        let new_virt_x = advance_remote_virtual_cursor(fdx, fdy);
                        if new_virt_x <= -EXIT_BUFFER_PX {
                            exit_remote(VIRT_Y.load(Ordering::Relaxed));
                            exited = true;
                        } else {
                            static HOOK_CAPTURED: AtomicI32 = AtomicI32::new(0);
                            let n = HOOK_CAPTURED.fetch_add(1, Ordering::Relaxed);
                            if n % 200 == 0 {
                                debug!(
                                    raw_dx = dx,
                                    raw_dy = dy,
                                    fdx,
                                    fdy,
                                    virt_x = new_virt_x,
                                    n,
                                    "sample motion captured (hook fallback)"
                                );
                            }
                            queue_pending_motion(fdx, fdy);
                        }
                    }
                }
                if !exited {
                    // Pin the hidden local cursor at the anchor. The virtual
                    // cursor above, not this warp, represents peer position.
                    unsafe {
                        let _ = SetCursorPos(ax, ay);
                    }
                    LAST_X.store(ax, Ordering::Relaxed);
                    LAST_Y.store(ay, Ordering::Relaxed);
                }
                // Consume the event so the OS doesn't process it locally.
                return LRESULT(1);
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
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
                if super::route_mouse_button(btn, down, forwarding_active()) {
                    sink_send(InputEvent::MouseButton { btn, down });
                    return LRESULT(1);
                }
            }
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL if forwarding_active() => {
            let delta =
                (((info.mouseData >> 16) as i16) as f32 / 120.0) * super::touchpad_scroll_speed();
            let horizontal = wparam.0 as u32 == WM_MOUSEHWHEEL;
            sink_send(windows_wheel_event(horizontal, delta));
        }
        _ => {}
    }
    // In remote mode — and while Game Driving — every mouse event has been
    // forwarded to the peer; consume it so the OS doesn't process it locally.
    // For Game Drive this is what pins the controller's cursor: consuming the
    // move event freezes the local cursor in place (raw input still forwards
    // the hardware delta), so it never drifts to a screen edge where motion
    // would clamp and stall the driven camera, and local clicks/scroll don't
    // leak onto this machine's desktop.
    if forwarding_active() {
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
            } else if super::game_drive() == super::GameDrive::Off {
                // While Game Driving we must NOT enter the cursor-crossing
                // REMOTE state — its motion branch runs the anchor warp that
                // re-introduces the in-game camera fling. Ctrl+Alt+R is inert
                // (but still consumed) during a Game Drive session.
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

        // Hotkey: Ctrl+Alt+G toggles Game Drive (drive the peer's game with
        // pure-relative mouse + keyboard, no cursor-crossing).
        if down
            && scan == SCAN_G
            && MOD_CTRL.load(Ordering::Relaxed)
            && MOD_ALT.load(Ordering::Relaxed)
        {
            info!("hotkey Ctrl+Alt+G — toggling Game Drive");
            super::toggle_game_drive();
            return LRESULT(1);
        }

        // Keystrokes are routed via `route_keystroke`, which combines
        // the user's keyboard-target preference, the cursor's
        // current side, AND a held-key tracker that ensures every
        // key release follows its press to the same destination.
        // The tracker is what fixes the "Shift-stuck-on-peer →
        // every letter forced uppercase" Caps-Lock bug when Smart
        // flips mid-keypress.

        if down || up {
            let extended = info.flags.0 & LLKHF_EXTENDED != 0;
            let pass_local_release =
                up && touchpad::take_legacy_passthrough_release(scan, extended);
            let routed = route_physical_key(scan, info.vkCode, extended, down, mode == MODE_REMOTE);
            if matches!(scan, 0x2a | 0x36) {
                info!(
                    scan,
                    extended,
                    down,
                    up,
                    cursor_in_remote = mode == MODE_REMOTE,
                    pass_local_release,
                    routed,
                    "source Shift low-level hook edge"
                );
            }
            if matches!(scan, 0x0f | 0x38) {
                info!(
                    scan,
                    extended,
                    down,
                    up,
                    cursor_in_remote = mode == MODE_REMOTE,
                    pass_local_release,
                    routed,
                    "source Alt+Tab low-level hook edge"
                );
            }
            // Normally a routed edge is consumed. The one exception is an up
            // whose down already passed through the foreground fallback
            // window: route it to the peer but also let it continue locally,
            // otherwise Windows retains a stuck physical modifier here.
            if routed && !pass_local_release {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn route_physical_key(
    scan: u32,
    vk: u32,
    extended: bool,
    down: bool,
    cursor_in_remote: bool,
) -> bool {
    // Keep extended scan-code identity intact before normalising to Linux
    // evdev numbers. In particular, keypad Enter and divide use the same base
    // set-1 scan codes as main Enter and slash; E0 is the only discriminator.
    let linux_code = captured_keycode(scan as u16, vk as u16, extended);
    if !super::route_keystroke(linux_code, down, cursor_in_remote) {
        return false;
    }
    sink_send(InputEvent::Key {
        code: KeyCode(linux_code),
        down,
    });
    if should_sync_forwarded_key_latch(down, cursor_in_remote) {
        touchpad::note_forwarded_key(linux_code, down);
    }
    super::note_key_forwarded_with_code(linux_code, down);
    true
}

fn should_sync_forwarded_key_latch(down: bool, cursor_in_remote: bool) -> bool {
    cursor_in_remote || !down
}

/// Keyboard fallback for the foreground Precision Touchpad capture window.
/// A successful low-level hook consumes the event before Windows posts the
/// legacy message, so this path is naturally de-duplicated. It runs only when
/// that capture surface actually receives a key the hook did not consume.
pub(super) fn route_touchpad_capture_key(scan: u32, vk: u32, extended: bool, down: bool) -> bool {
    route_physical_key(scan, vk, extended, down, true)
}

/// Tag tracking which kind of code is held so `release_all_held`
/// knows whether to call enigo's button() or key() to clear it.
#[derive(Hash, PartialEq, Eq, Clone, Copy)]
enum HeldKey {
    MouseBtn(Button),
    Key(u16),
}

fn should_inject_owned_edge(
    held: &std::collections::HashSet<HeldKey>,
    edge: HeldKey,
    down: bool,
) -> bool {
    down || held.contains(&edge)
}

fn record_owned_release_result(
    held: &mut std::collections::HashSet<HeldKey>,
    edge: HeldKey,
    succeeded: bool,
) {
    if !succeeded {
        held.insert(edge);
    }
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

    fn inject_key(&self, code: KeyCode, down: bool) -> Result<()> {
        if let Some(spec) = key_injection_spec(code) {
            return send_scancode_key_input(spec, down);
        }

        // Keep Enigo as the fallback for uncommon Linux evdev values that
        // do not have a one-byte Windows set-1 scan-code equivalent.
        let vk = scancode_to_vk(code.0);
        let dir = if down {
            Direction::Press
        } else {
            Direction::Release
        };
        self.inner
            .lock()
            .key(EKey::Other(vk as u32), dir)
            .context("enigo key")
    }
}

fn send_relative_mouse_input(dx: i32, dy: i32) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: MINESHARE_INPUT_TAG,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    anyhow::ensure!(sent == 1, "SendInput relative mouse move was rejected");
    Ok(())
}

fn move_desktop_cursor_rel(dx: i32, dy: i32) -> Result<()> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.context("get desktop cursor position")?;
    let x = cursor.x.saturating_add(dx);
    let y = cursor.y.saturating_add(dy);
    unsafe { SetCursorPos(x, y) }.context("set desktop cursor position")?;
    Ok(())
}

fn send_desktop_mouse_move(dx: i32, dy: i32) -> Result<()> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.context("get desktop cursor position")?;

    let origin_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let origin_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    let x = cursor
        .x
        .saturating_add(dx)
        .clamp(origin_x, origin_x.saturating_add(width - 1));
    let y = cursor
        .y
        .saturating_add(dy)
        .clamp(origin_y, origin_y.saturating_add(height - 1));

    let normalize = |value: i32, origin: i32, extent: i32| -> i32 {
        if extent <= 1 {
            0
        } else {
            ((i64::from(value - origin) * 65_535) / i64::from(extent - 1)) as i32
        }
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: normalize(x, origin_x, width),
                dy: normalize(y, origin_y, height),
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: MINESHARE_INPUT_TAG,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    anyhow::ensure!(sent == 1, "SendInput desktop mouse move was rejected");
    Ok(())
}

fn should_use_true_relative_injection(game_foreground: bool, game_drive: super::GameDrive) -> bool {
    game_foreground || matches!(game_drive, super::GameDrive::Receiving)
}

fn should_emit_mouse_motion_packet(
    game_foreground: bool,
    game_drive: super::GameDrive,
    mouse_button_held: bool,
) -> bool {
    should_use_true_relative_injection(game_foreground, game_drive) || mouse_button_held
}

impl InputInject for EnigoInject {
    fn mouse_move_rel(&self, dx: i32, dy: i32) -> Result<()> {
        let game_foreground = game_foreground();
        let game_drive = super::game_drive();
        let mouse_button_held = self
            .held
            .lock()
            .iter()
            .any(|held| matches!(held, HeldKey::MouseBtn(_)));
        if should_use_true_relative_injection(game_foreground, game_drive) {
            // Pointer-lock games continually recenter the cursor and require
            // true relative mickeys.
            send_relative_mouse_input(dx, dy)?;
        } else if should_emit_mouse_motion_packet(game_foreground, game_drive, mouse_button_held) {
            // Drag-sensitive Windows surfaces (notably the Win+Shift+S
            // snipping overlay) require real mouse-move packets while a
            // button is held. SetCursorPos alone changes the cursor location
            // but does not extend their selection gesture.
            send_desktop_mouse_move(dx, dy)?;
        } else {
            // Desktop sharing deliberately avoids SendInput. Every SendInput
            // call traverses the machine-wide low-level-hook chain; on the
            // affected Laptop that path stalled for 50–180 ms and delayed
            // genuine touchpad events in the same Windows input queue.
            // Direct cursor positioning measured below one millisecond p99
            // and preserves the existing desktop-relative semantics.
            move_desktop_cursor_rel(dx, dy)?;
        }
        if dx != 0 || dy != 0 {
            attempt_remote_focus("first-motion-retry");
        }
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
        let edge = HeldKey::MouseBtn(btn);
        let mut held = self.held.lock();
        if !should_inject_owned_edge(&held, edge, down) {
            debug!(?btn, "ignored unowned injected mouse-button release");
            return Ok(());
        }
        self.inner.lock().button(b, dir).context("enigo button")?;
        if down {
            held.insert(edge);
        } else {
            held.remove(&edge);
        }
        Ok(())
    }

    fn key(&self, code: KeyCode, down: bool) -> Result<()> {
        let trace_alt_tab = matches!(code.0, 15 | 56 | 100);
        let started = trace_alt_tab.then(Instant::now);
        if down {
            attempt_remote_key_focus();
        }
        let edge = HeldKey::Key(code.0);
        let mut held = self.held.lock();
        if !should_inject_owned_edge(&held, edge, down) {
            debug!(scancode = code.0, "ignored unowned injected key release");
            return Ok(());
        }
        self.inject_key(code, down)?;
        if let Some(started) = started {
            info!(
                code = code.0,
                down,
                elapsed_us = started.elapsed().as_micros() as u64,
                "target Alt+Tab injection edge"
            );
        }
        super::note_key_injected_with_code(code.0, down);
        if down {
            held.insert(edge);
        } else {
            held.remove(&edge);
        }
        Ok(())
    }

    fn release_all_held(&self) -> Result<()> {
        // Serialize cleanup with in-flight SendInput calls so a successful
        // down cannot land between draining the ownership set and releasing
        // it, leaving a modifier stuck after handback.
        let mut held = self.held.lock();
        let drained: Vec<HeldKey> = held.drain().collect();
        let mut released = 0usize;
        for h in drained {
            let res = match h {
                HeldKey::MouseBtn(btn) => {
                    let b = match btn {
                        Button::Left => EButton::Left,
                        Button::Right => EButton::Right,
                        Button::Middle => EButton::Middle,
                        Button::X1 => EButton::Back,
                        Button::X2 => EButton::Forward,
                    };
                    self.inner
                        .lock()
                        .button(b, Direction::Release)
                        .context("enigo button release")
                }
                HeldKey::Key(code) => self.inject_key(KeyCode(code), false),
            };
            match res {
                Ok(()) => released += 1,
                Err(e) => {
                    record_owned_release_result(&mut held, h, false);
                    warn!(error = %e, "failed to release held key; ownership retained for retry");
                }
            }
        }
        if released > 0 {
            info!(
                count = released,
                "released stale held keys at session end (preventing stuck-key after peer disconnect)"
            );
        }
        drop(held);
        touchpad::release_all()
    }

    fn scroll(&self, dx: f32, dy: f32) -> Result<()> {
        // Precision Touchpads emit partial deltas smaller than one 120-unit
        // wheel notch. Enigo accepts whole notches, so rounding dx/dy here
        // discarded most two-finger frames. Restore the original Windows
        // wheel units and inject without quantising.
        if dy != 0.0 {
            send_wheel_input(dy, false).context("SendInput scroll v")?;
        }
        if dx != 0.0 {
            send_wheel_input(dx, true).context("SendInput scroll h")?;
        }
        Ok(())
    }

    fn touchpad(&self, event: TouchpadEvent) -> Result<()> {
        touchpad::inject(event)
    }
}

fn windows_wheel_event(horizontal: bool, delta: f32) -> InputEvent {
    if horizontal {
        let dx = if super::invert_scroll_x() {
            -delta
        } else {
            delta
        };
        InputEvent::Scroll { dx, dy: 0.0 }
    } else {
        let dy = if super::invert_scroll_y() {
            -delta
        } else {
            delta
        };
        InputEvent::Scroll { dx: 0.0, dy }
    }
}

fn wheel_units(delta: f32) -> i32 {
    if delta.is_finite() {
        (delta * 120.0).round() as i32
    } else {
        0
    }
}

fn send_wheel_input(delta: f32, horizontal: bool) -> Result<()> {
    let units = wheel_units(delta);
    if units == 0 {
        return Ok(());
    }
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: units as u32,
                dwFlags: if horizontal {
                    MOUSEEVENTF_HWHEEL
                } else {
                    MOUSEEVENTF_WHEEL
                },
                time: 0,
                dwExtraInfo: MINESHARE_INPUT_TAG,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    anyhow::ensure!(sent == 1, "SendInput wheel event was rejected");
    Ok(())
}

/// Linux evdev codes for the extended key cluster that do not share values
/// with their Windows PS/2 scan-code equivalents. The table provides the
/// VK fallback for uncommon key injection and normalises capture where the
/// base scan code alone is not sufficient.
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
    (99, 0x2C),  // KEY_SYSRQ     ↔ VK_SNAPSHOT (PrintScreen)
];

/// Normalise a Windows hook event to its Linux evdev key code while retaining
/// E0-prefixed keypad and modifier keys. `vkCode` alone cannot distinguish
/// keypad Enter from main Enter, or keypad slash from the main slash key.
fn captured_keycode(scan: u16, vk: u16, extended: bool) -> u16 {
    if extended {
        let code = match scan {
            0x1C => Some(96),  // KEY_KPENTER
            0x1D => Some(97),  // KEY_RIGHTCTRL
            0x35 => Some(98),  // KEY_KPSLASH
            0x37 => Some(99),  // KEY_SYSRQ / PrintScreen
            0x38 => Some(100), // KEY_RIGHTALT
            0x45 => Some(69),  // KEY_NUMLOCK
            0x5B => Some(125), // KEY_LEFTMETA
            0x5C => Some(126), // KEY_RIGHTMETA
            0x5D => Some(127), // KEY_COMPOSE / application menu
            _ => None,
        };
        if let Some(code) = code {
            return code;
        }
    }

    EXTENDED_KEY_TABLE
        .iter()
        .find(|&&(_, key_vk)| key_vk == vk)
        .map(|&(evdev, _)| evdev)
        .unwrap_or(scan)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyInjectionSpec {
    scan: u16,
    extended: bool,
}

/// Return a raw Windows scan-code injection for keys whose Linux evdev code
/// either represents an E0-prefixed key or a keypad key. Sending these as a
/// virtual key through Enigo loses the exact physical key identity on some
/// Windows layouts, notably the logo keys and the keypad cluster.
fn key_injection_spec(KeyCode(code): KeyCode) -> Option<KeyInjectionSpec> {
    let (scan, extended) = match code {
        // Linux evdev intentionally retains the original PC/AT set-1 values
        // for the standard keyboard block. Keep those as physical scan-code
        // events instead of converting them to virtual keys. In particular,
        // Windows Shell only recognises Win+Shift+S reliably when all three
        // members of the chord arrive through one physical-key path.
        1..=68 | 70..=83 | 86..=88 => (code, false),
        69 => (0x45, true),  // KEY_NUMLOCK
        96 => (0x1C, true),  // KEY_KPENTER
        97 => (0x1D, true),  // KEY_RIGHTCTRL
        98 => (0x35, true),  // KEY_KPSLASH
        99 => (0x37, true),  // KEY_SYSRQ / PrintScreen
        100 => (0x38, true), // KEY_RIGHTALT
        102 => (0x47, true), // KEY_HOME
        103 => (0x48, true), // KEY_UP
        104 => (0x49, true), // KEY_PAGEUP
        105 => (0x4B, true), // KEY_LEFT
        106 => (0x4D, true), // KEY_RIGHT
        107 => (0x4F, true), // KEY_END
        108 => (0x50, true), // KEY_DOWN
        109 => (0x51, true), // KEY_PAGEDOWN
        110 => (0x52, true), // KEY_INSERT
        111 => (0x53, true), // KEY_DELETE
        125 => (0x5B, true), // KEY_LEFTMETA
        126 => (0x5C, true), // KEY_RIGHTMETA
        127 => (0x5D, true), // KEY_COMPOSE / application menu
        _ => return None,
    };
    Some(KeyInjectionSpec { scan, extended })
}

fn send_scancode_key_input(spec: KeyInjectionSpec, down: bool) -> Result<()> {
    let mut flags = KEYEVENTF_SCANCODE;
    if spec.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: spec.scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MINESHARE_INPUT_TAG,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    anyhow::ensure!(sent == 1, "SendInput keyboard event was rejected");
    Ok(())
}

/// Linux evdev KeyCode → Windows Virtual Key.
/// For common keys the PS/2 scan-code number equals the evdev number,
/// so `MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX)` handles them. Used as a
/// fallback only for uncommon evdev values without a direct scan-code spec.
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

const _: usize = mem::size_of::<MSLLHOOKSTRUCT>();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touchpad_capture_worker_is_isolated_from_hook_dispatch() {
        let hook_thread_id = thread::current().id();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);

        launch_touchpad_capture_worker(move || {
            tx.send((
                thread::current().id(),
                thread::current().name().map(str::to_owned),
            ))
            .expect("report touchpad worker identity");
        })
        .expect("launch touchpad capture worker");

        let (capture_thread_id, capture_thread_name) = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("touchpad worker should start");
        assert_ne!(
            capture_thread_id, hook_thread_id,
            "touchpad window traffic must not share the low-level keyboard hook thread"
        );
        assert_eq!(capture_thread_name.as_deref(), Some("win-touchpad-capture"));
    }

    #[test]
    fn keyboard_and_mouse_hooks_use_isolated_dispatch_threads() {
        let test_thread_id = thread::current().id();
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        let mouse_tx = tx.clone();

        launch_isolated_hook_workers(
            move || {
                mouse_tx
                    .send((
                        "mouse",
                        thread::current().id(),
                        thread::current().name().map(str::to_owned),
                    ))
                    .expect("report mouse hook worker identity");
            },
            move || {
                tx.send((
                    "keyboard",
                    thread::current().id(),
                    thread::current().name().map(str::to_owned),
                ))
                .expect("report keyboard hook worker identity");
            },
        )
        .expect("launch isolated hook workers");

        let first = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first hook worker should start");
        let second = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second hook worker should start");
        assert_ne!(first.1, test_thread_id);
        assert_ne!(second.1, test_thread_id);
        assert_ne!(
            first.1, second.1,
            "mouse traffic must not share the low-level keyboard hook thread"
        );

        let workers = [first, second]
            .into_iter()
            .map(|(role, _, name)| (role, name))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            workers["mouse"].as_deref(),
            Some("win-mouse-hook"),
            "mouse hook worker should be identifiable in diagnostics"
        );
        assert_eq!(
            workers["keyboard"].as_deref(),
            Some("win-keyboard-hook"),
            "keyboard hook worker should be identifiable in diagnostics"
        );
    }

    #[test]
    fn focus_handoff_starts_immediately_and_retries_only_after_failure() {
        let handoff = FocusHandoff::new();
        handoff.arm();
        assert!(handoff.try_begin());
        handoff.finish(false);
        assert!(handoff.try_begin());
        handoff.finish(true);
        assert!(!handoff.try_begin());
    }

    #[test]
    fn focus_handoff_revalidates_at_first_key_after_motion_activation() {
        let handoff = FocusHandoff::new();
        handoff.arm();

        assert!(handoff.try_begin(), "motion may activate the edge window");
        handoff.finish(true);
        assert!(
            handoff.try_begin_key(),
            "the first key needs an independent focus checkpoint at the cursor's final position"
        );
    }

    #[test]
    fn focus_handoff_waits_until_cursor_leaves_warped_edge() {
        let screen = (0, 0, 1919, 1079);
        assert!(!focus_point_is_safe((0, 539), screen));
        assert!(!focus_point_is_safe((7, 539), screen));
        assert!(focus_point_is_safe((8, 539), screen));
        assert!(!focus_point_is_safe((1919, 539), screen));
        assert!(focus_point_is_safe((1911, 539), screen));
        assert!(!focus_point_is_safe((960, 0), screen));
        assert!(focus_point_is_safe((960, 8), screen));
    }

    #[test]
    fn raw_input_edge_detection_preserves_every_layout_direction() {
        let desktop = (0, 0, 99, 99);
        assert!(crossed_peer_edge(
            super::super::PeerSide::Right,
            desktop,
            (98, 50),
            (99, 50),
        ));
        assert!(crossed_peer_edge(
            super::super::PeerSide::Left,
            desktop,
            (1, 50),
            (0, 50),
        ));
        assert!(crossed_peer_edge(
            super::super::PeerSide::Top,
            desktop,
            (50, 1),
            (50, 0),
        ));
        assert!(crossed_peer_edge(
            super::super::PeerSide::Bottom,
            desktop,
            (50, 98),
            (50, 99),
        ));
        assert!(!crossed_peer_edge(
            super::super::PeerSide::Right,
            desktop,
            (50, 50),
            (51, 50),
        ));
    }

    #[test]
    fn direct_key_specs_preserve_meta_and_numpad_identity() {
        assert_eq!(
            key_injection_spec(KeyCode(125)),
            Some(KeyInjectionSpec {
                scan: 0x5B,
                extended: true,
            }),
            "left Win must be injected as an extended scan code"
        );
        assert_eq!(
            key_injection_spec(KeyCode(96)),
            Some(KeyInjectionSpec {
                scan: 0x1C,
                extended: true,
            }),
            "keypad Enter must not collapse into the main Enter key"
        );
        assert_eq!(
            key_injection_spec(KeyCode(79)),
            Some(KeyInjectionSpec {
                scan: 0x4F,
                extended: false,
            }),
            "keypad 1 must remain the non-extended keypad key"
        );
        assert_eq!(
            captured_keycode(0x1C, 0x0D, true),
            96,
            "capturing keypad Enter must retain its E0 identity"
        );
    }

    #[test]
    fn snipping_shortcut_preserves_win_shift_s_identity() {
        assert_eq!(captured_keycode(0x5B, 0x5B, true), 125);
        assert_eq!(captured_keycode(0x2A, 0xA0, false), 42);
        assert_eq!(captured_keycode(0x1F, 0x53, false), 31);
        assert_eq!(
            key_injection_spec(KeyCode(125)),
            Some(KeyInjectionSpec {
                scan: 0x5B,
                extended: true,
            })
        );
        assert_eq!(
            key_injection_spec(KeyCode(42)),
            Some(KeyInjectionSpec {
                scan: 0x2A,
                extended: false,
            })
        );
        assert_eq!(
            key_injection_spec(KeyCode(31)),
            Some(KeyInjectionSpec {
                scan: 0x1F,
                extended: false,
            })
        );
    }

    #[test]
    fn forwarded_modifier_release_clears_latch_after_cursor_returns_local() {
        assert!(should_sync_forwarded_key_latch(true, true));
        assert!(
            should_sync_forwarded_key_latch(false, false),
            "a routed Shift-up must clear the remote-down latch even after cursor handback"
        );
    }

    #[test]
    fn injected_shift_release_requires_a_mineshare_owned_down() {
        let mut held = std::collections::HashSet::new();
        for code in [42, 54] {
            let shift = HeldKey::Key(code);
            assert!(should_inject_owned_edge(&held, shift, true));
            assert!(
                !should_inject_owned_edge(&held, shift, false),
                "unowned Shift-up code {code} could disturb the physical keyboard"
            );
            held.insert(shift);
            assert!(should_inject_owned_edge(&held, shift, false));
            held.remove(&shift);
        }
    }

    #[test]
    fn failed_shift_cleanup_retains_ownership_for_retry() {
        let shift = HeldKey::Key(42);
        let mut held = std::collections::HashSet::new();

        record_owned_release_result(&mut held, shift, false);
        assert!(
            held.contains(&shift),
            "a failed cleanup must let the next heartbeat retry Shift-up"
        );

        held.clear();
        record_owned_release_result(&mut held, shift, true);
        assert!(
            held.is_empty(),
            "successful cleanup must not restore ownership"
        );
    }

    #[test]
    fn anchor_cursor_only_when_game_foreground() {
        // Skip the edge warp only when a cursor-locked game owns the cursor.
        // The game-drive path no longer relies on this, so a peer-driven
        // session that is NOT a foreground game still warps (desktop crossing
        // needs the warp for its exit hysteresis).
        assert!(should_anchor_cursor(true, false)); // game foreground
        assert!(!should_anchor_cursor(false, true)); // peer driving, not game → warp
        assert!(should_anchor_cursor(true, true));
        assert!(!should_anchor_cursor(false, false)); // desktop crossing → warp
    }

    #[test]
    fn take_control_skips_warp_when_game_foreground() {
        let bounds = (0, 0, 2880, 1800);
        // Normal desktop crossing from the right → warp to the right edge,
        // mid-height, so the peer's virt_x model matches reality.
        assert_eq!(
            take_control_action(false, crate::PeerSide::Right, bounds, (700, 700)),
            TakeControlAction::Warp(2880, 900),
        );
        // A cursor-locked / anti-cheat game owns the cursor (Roblox LockCenter):
        // warping to the edge starts a tug-of-war with the game's per-frame
        // recenter that flings the camera. Leave the cursor where it is.
        assert_eq!(
            take_control_action(true, crate::PeerSide::Right, bounds, (700, 700)),
            TakeControlAction::Anchor(700, 700),
        );
    }

    #[test]
    fn hidden_cursor_alone_does_not_trigger_game_lock() {
        assert!(
            !should_auto_lock_game(true, false, false, true),
            "Windows hides the pointer while typing; that alone is not evidence of a game"
        );
        assert!(should_auto_lock_game(false, true, false, true));
        assert!(should_auto_lock_game(false, false, true, true));
        assert!(!should_auto_lock_game(false, true, false, false));
    }

    #[test]
    fn true_relative_injection_is_scoped_to_game_receiving() {
        assert!(should_use_true_relative_injection(
            true,
            super::super::GameDrive::Off
        ));
        assert!(should_use_true_relative_injection(
            false,
            super::super::GameDrive::Receiving
        ));
        assert!(!should_use_true_relative_injection(
            false,
            super::super::GameDrive::Driving
        ));
        assert!(!should_use_true_relative_injection(
            false,
            super::super::GameDrive::Off
        ));
    }

    #[test]
    fn desktop_drag_emits_mouse_motion_packets() {
        assert!(should_emit_mouse_motion_packet(
            false,
            super::super::GameDrive::Off,
            true
        ));
        assert!(!should_emit_mouse_motion_packet(
            false,
            super::super::GameDrive::Off,
            false
        ));
    }

    #[test]
    fn wheel_capture_preserves_both_touchpad_scroll_axes() {
        super::super::set_invert_scroll(false, false);
        super::super::set_touchpad_scroll_speed(1.0);
        assert_eq!(
            windows_wheel_event(false, 1.25),
            InputEvent::Scroll { dx: 0.0, dy: 1.25 }
        );
        assert_eq!(
            windows_wheel_event(true, -0.5),
            InputEvent::Scroll { dx: -0.5, dy: 0.0 }
        );
        assert_eq!(wheel_units(1.0), 120);
        assert_eq!(wheel_units(1.0 / 120.0), 1);
        assert_eq!(wheel_units(-0.25), -30);
    }

    #[test]
    fn raw_forwarded_motion_tracks_visible_peer_depth() {
        super::super::set_peer_side(super::super::PeerSide::Right);
        PEER_W.store(1_920, Ordering::Relaxed);
        VIRT_X.store(0, Ordering::Relaxed);
        VIRT_Y.store(500, Ordering::Relaxed);
        CURSOR_MODE.store(MODE_REMOTE, Ordering::Release);

        // The peer applies this exact desktop delta. If the source keeps
        // VIRT_X at the entry edge, a small reverse movement can incorrectly
        // satisfy the exit threshold even though the visible peer cursor is
        // still hundreds of pixels away from its edge.
        assert!(!track_raw_remote_motion(600, 20));

        assert_eq!(
            VIRT_X.load(Ordering::Relaxed),
            600,
            "source virtual depth must follow the delta visibly applied on the peer"
        );
        assert_eq!(VIRT_Y.load(Ordering::Relaxed), 520);

        assert!(!track_raw_remote_motion(-50, 0));
        assert_eq!(
            VIRT_X.load(Ordering::Relaxed),
            550,
            "a small reverse movement must not jump back to the laptop"
        );

        // Reaching the visible edge (550 px back) still keeps the 100 px
        // hysteresis buffer. Only the final 100 px requests the handoff.
        assert!(!track_raw_remote_motion(-600, 0));
        assert_eq!(VIRT_X.load(Ordering::Relaxed), -50);
        assert!(track_raw_remote_motion(-50, 0));
        assert_eq!(VIRT_X.load(Ordering::Relaxed), -EXIT_BUFFER_PX);
        CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
    }

    #[test]
    fn remote_depth_axis_matches_every_layout_side() {
        assert_eq!(
            remote_motion_axes(super::super::PeerSide::Right, 7, 11),
            (7, 11)
        );
        assert_eq!(
            remote_motion_axes(super::super::PeerSide::Left, -7, 11),
            (7, 11)
        );
        assert_eq!(
            remote_motion_axes(super::super::PeerSide::Top, 11, -7),
            (7, 11)
        );
        assert_eq!(
            remote_motion_axes(super::super::PeerSide::Bottom, 11, 7),
            (7, 11)
        );
    }

    #[test]
    fn snap_never_when_non_negative() {
        // virt_x >= 0 never snaps, even after a long idle period.
        assert!(!should_snap_virt_x(0, 1, 10_000));
        assert!(!should_snap_virt_x(5, 1, 10_000));
    }

    #[test]
    fn snap_never_when_never_flushed() {
        // last_flush == 0 means "never flushed" and must not trigger.
        assert!(!should_snap_virt_x(-1, 0, 10_000));
    }

    #[test]
    fn snap_not_when_recently_flushed() {
        // Negative drift but flushed < REMOTE_IDLE_SNAP_MS ago → no snap.
        let now = 5_000;
        let last_flush = now - (REMOTE_IDLE_SNAP_MS - 1);
        assert!(!should_snap_virt_x(-50, last_flush, now));
    }

    #[test]
    fn snap_when_negative_and_idle_long() {
        // Negative drift and idle >= REMOTE_IDLE_SNAP_MS → snap.
        let now = 5_000;
        let last_flush = now - REMOTE_IDLE_SNAP_MS;
        assert!(should_snap_virt_x(-50, last_flush, now));
    }

    #[test]
    #[ignore = "manual CPU regression gate; starts the process-lifetime watchdog"]
    fn idle_motion_watchdog_is_event_driven() {
        super::super::set_game_drive(super::super::GameDrive::Off);
        CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
        super::super::set_mouse_rate_hz(1_000);
        start_motion_flush_watchdog();

        thread::sleep(std::time::Duration::from_millis(20));
        let before = FLUSH_WATCHDOG_WAKEUPS.load(Ordering::Relaxed);
        thread::sleep(std::time::Duration::from_millis(40));
        let wakeups = FLUSH_WATCHDOG_WAKEUPS
            .load(Ordering::Relaxed)
            .saturating_sub(before);

        assert!(
            wakeups <= 1,
            "idle watchdog polled {wakeups} times in 40 ms instead of parking"
        );
    }

    #[test]
    #[ignore = "manual active-latency regression gate; starts the process-lifetime watchdog"]
    fn active_motion_burst_does_not_unpark_per_hardware_event() {
        super::super::set_game_drive(super::super::GameDrive::Off);
        CURSOR_MODE.store(MODE_REMOTE, Ordering::Release);
        super::super::set_mouse_rate_hz(1_000);
        start_motion_flush_watchdog();
        thread::sleep(std::time::Duration::from_millis(10));

        let before = FLUSH_WATCHDOG_UNPARKS.load(Ordering::Relaxed);
        let distance_before = FLUSH_FWD_DISTANCE_X.load(Ordering::Relaxed);
        for _ in 0..1_000 {
            queue_pending_motion(1, 0);
        }
        thread::sleep(std::time::Duration::from_millis(10));
        let unparks = FLUSH_WATCHDOG_UNPARKS
            .load(Ordering::Relaxed)
            .saturating_sub(before);
        let distance = FLUSH_FWD_DISTANCE_X
            .load(Ordering::Relaxed)
            .saturating_sub(distance_before);
        CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
        wake_motion_flush_watchdog();

        assert!(
            unparks <= 4,
            "1,000-event burst caused {unparks} scheduler unparks"
        );
        assert_eq!(
            distance, 1_000,
            "scheduler lost {distance}/1,000 motion units"
        );
    }

    #[test]
    #[ignore = "manual live latency gate; briefly moves the real Windows cursor"]
    fn desktop_inject_path_has_no_user_visible_stalls() {
        super::super::set_game_drive(super::super::GameDrive::Off);
        let inject = EnigoInject::new().expect("create Windows input injector");
        let mut samples = Vec::with_capacity(1_000);

        for i in 0usize..1_000 {
            let dx = if i.is_multiple_of(2) { 1 } else { -1 };
            // Match the fastest supported production cadence. A tight
            // unpaced loop measures Windows queue saturation rather than the
            // MineShare dispatcher, which now uses the same 1,000 Hz ceiling.
            thread::sleep(std::time::Duration::from_millis(1));
            let started = std::time::Instant::now();
            inject
                .mouse_move_rel(dx, 0)
                .expect("inject desktop-relative mouse move");
            samples.push(started.elapsed());
        }

        samples.sort_unstable();
        let p99 = samples[samples.len() * 99 / 100];
        let max = *samples.last().expect("latency sample");
        eprintln!("desktop inject latency: p99={p99:?} max={max:?}");

        assert!(
            p99 <= std::time::Duration::from_millis(8),
            "desktop inject p99 {p99:?} exceeds one 120 Hz display frame"
        );
        assert!(
            max <= std::time::Duration::from_millis(50),
            "desktop inject stalled for {max:?}, which is user-visible"
        );
    }
}
