//! Native Windows 11 Precision Touchpad capture and injection.
//!
//! The parent Windows input module exposes only four operations from this
//! private module: setup capture, enable/disable routing, report capability
//! bits, and inject/release a `TouchpadEvent`. Win32/WinRT availability,
//! contact lifecycle reconstruction, foreground ownership, and synthetic
//! device cleanup stay local to this implementation.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU8, AtomicU32, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tracing::{debug, info, warn};
use windows::Foundation::TypedEventHandler;
use windows::UI::Core::PointerEventArgs;
use windows::UI::Input::{
    PointerPoint, TouchpadGesturesController, TouchpadGlobalAction, TouchpadGlobalActionEventArgs,
    TouchpadGlobalGestureKinds,
};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize};
use windows::Win32::UI::Controls::{
    HSYNTHETICPOINTERDEVICE, POINTER_FEEDBACK_MODE, POINTER_FEEDBACK_NONE, POINTER_TYPE_INFO,
    POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardState, SetActiveWindow, SetFocus, SetKeyboardState, VK_LSHIFT,
    VK_MENU, VK_RSHIFT, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::Input::Pointer::{
    GetPointerDeviceRects, GetPointerInfo, InjectSyntheticPointerInput, POINTER_FLAG_CONFIDENCE,
    POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_UP,
    POINTER_FLAG_UPDATE, POINTER_FLAGS, POINTER_INFO, POINTER_TOUCH_INFO, SkipPointerFrameMessages,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow,
    GetMessageW, GetWindowThreadProcessId, KillTimer, LWA_ALPHA, MSG, PT_TOUCHPAD, PostMessageW,
    RegisterClassExW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SendMessageTimeoutW, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer,
    SetWindowPos, ShowWindow, SwitchToThisWindow, TranslateMessage, WM_APP, WM_KEYDOWN, WM_KEYUP,
    WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, PCSTR, PCWSTR, w};

use crate::{
    InputEvent, MAX_TOUCHPAD_CONTACTS, TOUCHPAD_CAP_ACTION_CAPTURE, TOUCHPAD_CAP_ACTION_INJECT,
    TOUCHPAD_CAP_FRAME_CAPTURE, TOUCHPAD_CAP_FRAME_INJECT, TouchpadAction, TouchpadContact,
    TouchpadEvent,
};

const DEVICE_WIDTH: u32 = 10_000;
const DEVICE_HEIGHT: u32 = 6_000;
const TERMINAL_REPEAT_COUNT: usize = 3;
const WM_TOUCHPAD_CAPTURE_ROUTE: u32 = WM_APP + 0x341;
const WM_TOUCHPAD_KEYBOARD_SUSPEND: u32 = WM_APP + 0x342;
const WM_TOUCHPAD_KEYBOARD_RESUME: u32 = WM_APP + 0x343;
const CAPTURE_FOCUS_TIMER_ID: usize = 0x4d53_5450;
// SetTimer clamps intervals below USER_TIMER_MINIMUM to 10 ms. Polling at
// that documented floor keeps shell-reserved shortcuts inside one display
// frame without pretending an 8 ms request would actually be honoured.
const CAPTURE_FOCUS_RETRY_MS: u32 = 10;
const CAPTURE_WINDOW_SIZE: i32 = 5;

static CAPABILITIES: AtomicU32 = AtomicU32::new(0);
static CAPTURE_REQUESTED: AtomicBool = AtomicBool::new(false);
static CAPTURE_ENABLED: AtomicBool = AtomicBool::new(false);
static KEYBOARD_CAPTURE_SUSPENDED: AtomicBool = AtomicBool::new(false);
static CAPTURE_FOCUS_PENDING_LOGGED: AtomicBool = AtomicBool::new(false);
static CAPTURE_SETUP_COMPLETE: AtomicBool = AtomicBool::new(false);
static LEGACY_ALT_HELD: AtomicBool = AtomicBool::new(false);
static ASYNC_TAB_FORWARDED: AtomicBool = AtomicBool::new(false);
static RAW_ALT_HELD: AtomicBool = AtomicBool::new(false);
static LEGACY_SHIFT_HELD: AtomicU8 = AtomicU8::new(0);
static LEGACY_META_HELD: AtomicU8 = AtomicU8::new(0);
static ASYNC_META_FORWARDED: AtomicU8 = AtomicU8::new(0);
static ASYNC_S_FORWARDED: AtomicBool = AtomicBool::new(false);
static RAW_KEYBOARD_AVAILABLE: AtomicBool = AtomicBool::new(false);
static CAPTURE_HWND: AtomicIsize = AtomicIsize::new(0);
static LEGACY_PASSTHROUGH_KEYS: OnceLock<Mutex<LegacyPassthroughTracker>> = OnceLock::new();
static CAPTURE_ANCHOR_X: AtomicI32 = AtomicI32::new(0);
static CAPTURE_ANCHOR_Y: AtomicI32 = AtomicI32::new(0);
static PREVIOUS_FOREGROUND_HWND: AtomicIsize = AtomicIsize::new(0);
static CONTROLLER: OnceLock<TouchpadGesturesController> = OnceLock::new();
static CAPTURE_STATE: Mutex<CaptureState> = parking_lot::const_mutex(CaptureState::new());
static INJECTION_STATE: Mutex<InjectionState> = parking_lot::const_mutex(InjectionState::new());

type RegisterTouchpadCapableWindowFn = unsafe extern "system" fn(HWND, BOOL) -> BOOL;
type GetPointerFrameTouchpadInfoHistoryFn =
    unsafe extern "system" fn(u32, *mut u32, *mut u32, *mut POINTER_TOUCH_INFO) -> BOOL;

#[derive(Clone, Copy)]
struct CaptureApi {
    register_window: RegisterTouchpadCapableWindowFn,
    frame_history: GetPointerFrameTouchpadInfoHistoryFn,
}

static CAPTURE_API: OnceLock<std::result::Result<CaptureApi, String>> = OnceLock::new();

#[derive(Default)]
struct LegacyPassthroughTracker {
    held: HashSet<(u32, bool)>,
}

impl LegacyPassthroughTracker {
    fn identity(scan: u32, extended: bool) -> (u32, bool) {
        // The scan code already distinguishes L/R Shift. On the affected
        // Windows hardware WM_KEYDOWN reports Right Shift as non-extended,
        // while WH_KEYBOARD_LL reports the matching up as extended.
        (
            scan,
            if matches!(scan, 0x2a | 0x36) {
                false
            } else {
                extended
            },
        )
    }

    fn note_legacy_edge(&mut self, scan: u32, extended: bool, down: bool) {
        let identity = Self::identity(scan, extended);
        if down {
            self.held.insert(identity);
        } else {
            self.held.remove(&identity);
        }
    }

    fn take_hook_release(&mut self, scan: u32, extended: bool) -> bool {
        self.held.remove(&Self::identity(scan, extended))
    }
}

fn legacy_passthrough_keys() -> &'static Mutex<LegacyPassthroughTracker> {
    LEGACY_PASSTHROUGH_KEYS.get_or_init(|| Mutex::new(LegacyPassthroughTracker::default()))
}

fn note_legacy_passthrough_edge(scan: u32, extended: bool, down: bool) {
    legacy_passthrough_keys()
        .lock()
        .note_legacy_edge(scan, extended, down);
}

fn should_neutralize_routed_legacy_shift(scan: u32, down: bool, routed: bool) -> bool {
    routed && down && matches!(scan, 0x2a | 0x36)
}

fn neutralize_capture_thread_shift_state() -> Result<()> {
    let mut state = [0u8; 256];
    unsafe { GetKeyboardState(&mut state) }.context("GetKeyboardState capture thread")?;
    for key in [VK_SHIFT, VK_LSHIFT, VK_RSHIFT] {
        state[key.0 as usize] = 0;
    }
    unsafe { SetKeyboardState(&state) }.context("SetKeyboardState capture thread")
}

pub(super) fn take_legacy_passthrough_release(scan: u32, extended: bool) -> bool {
    legacy_passthrough_keys()
        .lock()
        .take_hook_release(scan, extended)
}

#[repr(C)]
struct SyntheticDeviceCreationParams {
    pointer_type: windows::Win32::UI::WindowsAndMessaging::POINTER_INPUT_TYPE,
    max_count: u32,
    feedback_mode: POINTER_FEEDBACK_MODE,
    monitor: HMONITOR,
    device_width: u32,
    device_height: u32,
    options: u32,
}

type CreateSyntheticPointerDevice2Fn =
    unsafe extern "system" fn(*const SyntheticDeviceCreationParams) -> HSYNTHETICPOINTERDEVICE;
type InjectTouchpadActionFn = unsafe extern "system" fn(HSYNTHETICPOINTERDEVICE, i32) -> BOOL;

struct InjectionApi {
    device: usize,
    inject_action: InjectTouchpadActionFn,
}

static INJECTION_API: OnceLock<std::result::Result<InjectionApi, String>> = OnceLock::new();

unsafe fn user32_export(name: PCSTR, ordinal: u16) -> std::result::Result<*const c_void, String> {
    let user32 = unsafe { GetModuleHandleW(w!("user32.dll")) }.map_err(|e| e.to_string())?;
    let by_name = unsafe { GetProcAddress(user32, name) };
    let proc = by_name
        .or_else(|| unsafe { GetProcAddress(user32, PCSTR(usize::from(ordinal) as *const u8)) });
    proc.map(|f| f as *const () as *const c_void)
        .ok_or_else(|| format!("user32 export unavailable (ordinal {ordinal})"))
}

fn capture_api() -> Result<CaptureApi> {
    CAPTURE_API
        .get_or_init(|| unsafe {
            let register = user32_export(
                PCSTR(c"RegisterTouchpadCapableWindow".as_ptr().cast()),
                2689,
            )?;
            let history = user32_export(
                PCSTR(c"GetPointerFrameTouchpadInfoHistory".as_ptr().cast()),
                2694,
            )?;
            Ok(CaptureApi {
                register_window: std::mem::transmute::<
                    *const c_void,
                    RegisterTouchpadCapableWindowFn,
                >(register),
                frame_history: std::mem::transmute::<
                    *const c_void,
                    GetPointerFrameTouchpadInfoHistoryFn,
                >(history),
            })
        })
        .clone()
        .map_err(anyhow::Error::msg)
}

fn injection_api() -> Result<&'static InjectionApi> {
    INJECTION_API
        .get_or_init(|| unsafe {
            let create = user32_export(
                PCSTR(c"CreateSyntheticPointerDevice2".as_ptr().cast()),
                2690,
            )?;
            let inject_action =
                user32_export(PCSTR(c"InjectTouchpadAction".as_ptr().cast()), 2803)?;
            let create: CreateSyntheticPointerDevice2Fn = std::mem::transmute(create);
            let inject_action: InjectTouchpadActionFn = std::mem::transmute(inject_action);
            let params = SyntheticDeviceCreationParams {
                pointer_type: PT_TOUCHPAD,
                max_count: MAX_TOUCHPAD_CONTACTS as u32,
                feedback_mode: POINTER_FEEDBACK_NONE,
                monitor: HMONITOR::default(),
                device_width: DEVICE_WIDTH,
                device_height: DEVICE_HEIGHT,
                // SDCO_PHYSICAL_SIZE | SDCO_TOUCHPAD_GESTURE_ONLY.
                options: 0x1 | 0x2,
            };
            let device = create(&params);
            if device.0.is_null() {
                return Err("CreateSyntheticPointerDevice2 failed".to_string());
            }
            Ok(InjectionApi {
                device: device.0 as usize,
                inject_action,
            })
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))
}

pub(super) fn capabilities() -> u32 {
    let mut caps = CAPABILITIES.load(Ordering::Acquire);
    if injection_api().is_ok() {
        caps |= TOUCHPAD_CAP_FRAME_INJECT | TOUCHPAD_CAP_ACTION_INJECT;
    }
    caps
}

pub(super) fn setup_capture() -> Result<()> {
    let mut ready = false;

    match create_capture_window() {
        Ok(hwnd) => {
            CAPTURE_HWND.store(hwnd.0 as isize, Ordering::Release);
            CAPABILITIES.fetch_or(TOUCHPAD_CAP_FRAME_CAPTURE, Ordering::AcqRel);
            ready = true;
            info!("Windows Precision Touchpad two-finger frame capture ready");
        }
        Err(e) => warn!(error = %e, "Precision Touchpad two-finger capture unavailable"),
    }

    match setup_global_controller() {
        Ok(controller) => {
            let _ = CONTROLLER.set(controller);
            CAPABILITIES.fetch_or(TOUCHPAD_CAP_ACTION_CAPTURE, Ordering::AcqRel);
            ready = true;
            info!("Windows Precision Touchpad 3/4-finger capture ready");
        }
        Err(e) => warn!(error = %e, "Precision Touchpad 3/4-finger capture unavailable"),
    }

    CAPTURE_SETUP_COMPLETE.store(true, Ordering::Release);
    anyhow::ensure!(
        ready,
        "no native Precision Touchpad capture path is available"
    );
    Ok(())
}

pub(super) fn run_capture_loop() -> Result<()> {
    // WinRT touchpad controllers and their capture window are apartment-bound.
    // Keep their message traffic on this dedicated STA so high-rate pointer
    // frames can never delay the WH_KEYBOARD_LL dispatch thread.
    let _ = unsafe { RoInitialize(RO_INIT_SINGLETHREADED) };
    setup_capture()?;

    let mut msg = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if result.0 == 0 {
            break;
        }
        anyhow::ensure!(result.0 != -1, "touchpad capture message loop failed");
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

pub(super) fn wait_for_capture_setup(timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while !CAPTURE_SETUP_COMPLETE.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

pub(super) fn set_capture_anchor(x: i32, y: i32) {
    CAPTURE_ANCHOR_X.store(x, Ordering::Release);
    CAPTURE_ANCHOR_Y.store(y, Ordering::Release);
}

pub(super) fn set_capture_active(requested: bool) {
    CAPTURE_REQUESTED.store(requested, Ordering::Release);
    if !requested {
        KEYBOARD_CAPTURE_SUSPENDED.store(false, Ordering::Release);
    }
    let raw = CAPTURE_HWND.load(Ordering::Acquire);
    if raw == 0 {
        // The frame-capture window is also our cross-thread control window.
        // Without it, invoking the WinRT controller from a foreign apartment
        // is unsafe; keep the basic pointer/scroll fallback active instead.
        CAPTURE_ENABLED.store(false, Ordering::Release);
        if let Some(controller) = CONTROLLER.get() {
            let _ = controller.SetEnabled(false);
        }
        if requested {
            warn!("native touchpad capture control window unavailable; using basic fallback");
        }
        return;
    }

    let capture = HWND(raw as *mut c_void);
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            capture,
            WM_TOUCHPAD_CAPTURE_ROUTE,
            WPARAM(usize::from(requested)),
            LPARAM(0),
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            50,
            Some(&mut result),
        )
    };
    if sent.0 == 0 {
        // Never leave the state change stranded if the hook thread was
        // momentarily busy. Its message pump will apply this immediately
        // after the current low-level-hook callback returns.
        if let Err(e) = unsafe {
            PostMessageW(
                Some(capture),
                WM_TOUCHPAD_CAPTURE_ROUTE,
                WPARAM(usize::from(requested)),
                LPARAM(0),
            )
        } {
            warn!(error = %e, requested, "failed to queue touchpad capture route change");
        }
    }
}

pub(super) fn reset_forwarded_keyboard_latches() {
    LEGACY_ALT_HELD.store(false, Ordering::Release);
    RAW_ALT_HELD.store(false, Ordering::Release);
    ASYNC_TAB_FORWARDED.store(false, Ordering::Release);
    LEGACY_SHIFT_HELD.store(0, Ordering::Release);
    LEGACY_META_HELD.store(0, Ordering::Release);
    ASYNC_META_FORWARDED.store(0, Ordering::Release);
    ASYNC_S_FORWARDED.store(false, Ordering::Release);
    if let Some(keys) = LEGACY_PASSTHROUGH_KEYS.get() {
        keys.lock().held.clear();
    }
    let was_suspended = KEYBOARD_CAPTURE_SUSPENDED.swap(false, Ordering::AcqRel);
    if was_suspended && CAPTURE_REQUESTED.load(Ordering::Acquire) {
        post_capture_control(WM_TOUCHPAD_KEYBOARD_RESUME);
    }
}

fn apply_capture_active(requested: bool) {
    let local = capabilities();
    let peer = crate::peer_touchpad_capabilities();
    let frames = local & TOUCHPAD_CAP_FRAME_CAPTURE != 0 && peer & TOUCHPAD_CAP_FRAME_INJECT != 0;
    let actions =
        local & TOUCHPAD_CAP_ACTION_CAPTURE != 0 && peer & TOUCHPAD_CAP_ACTION_INJECT != 0;
    let route_available = frames || actions;
    let was_active = CAPTURE_ENABLED.load(Ordering::Acquire);
    let raw = CAPTURE_HWND.load(Ordering::Acquire);
    let capture = (raw != 0).then_some(HWND(raw as *mut c_void));

    if !requested || !route_available {
        CAPTURE_ENABLED.store(false, Ordering::Release);
        if let Some(controller) = CONTROLLER.get()
            && let Err(e) = controller.SetEnabled(false)
        {
            warn!(error = %e, "failed to disable touchpad gesture capture");
        }
        if let Some(capture) = capture {
            let _ = unsafe { KillTimer(Some(capture), CAPTURE_FOCUS_TIMER_ID) };
        }
        if was_active {
            publish_cancel();
        }
        restore_foreground();
        if let Some(capture) = capture {
            let _ = unsafe { ShowWindow(capture, SW_HIDE) };
        }
        CAPTURE_FOCUS_PENDING_LOGGED.store(false, Ordering::Release);
        return;
    }

    let focused = if let Some(capture) = capture {
        let previous = unsafe { GetForegroundWindow() };
        if previous != capture && !previous.0.is_null() {
            let _ = PREVIOUS_FOREGROUND_HWND.compare_exchange(
                0,
                previous.0 as isize,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        claim_foreground(capture)
    } else {
        false
    };
    let active = should_enable_native_capture(requested, frames, actions, focused);

    if was_active && !active {
        CAPTURE_ENABLED.store(false, Ordering::Release);
        publish_cancel();
    } else if !was_active && active {
        CAPTURE_STATE.lock().reset_without_event();
    }

    if let Some(controller) = CONTROLLER.get()
        && let Err(e) = controller.SetEnabled(active && actions)
    {
        warn!(error = %e, active, "failed to change touchpad gesture capture");
        CAPTURE_ENABLED.store(false, Ordering::Release);
        return;
    }
    CAPTURE_ENABLED.store(active, Ordering::Release);

    if let Some(capture) = capture {
        // Keep ownership while the cursor is on the peer. Windows can briefly
        // transfer foreground to a shell surface or notification even though
        // the user never returned locally; a one-frame retry prevents the
        // next physical gesture from being interpreted on this laptop.
        let timer = unsafe {
            SetTimer(
                Some(capture),
                CAPTURE_FOCUS_TIMER_ID,
                CAPTURE_FOCUS_RETRY_MS,
                None,
            )
        };
        if timer == 0 {
            warn!("failed to start native touchpad foreground watchdog");
        }
    }

    if focused {
        CAPTURE_FOCUS_PENDING_LOGGED.store(false, Ordering::Release);
        info!(
            focused,
            active, frames, actions, "native touchpad routed to peer focus"
        );
    } else if !CAPTURE_FOCUS_PENDING_LOGGED.swap(true, Ordering::AcqRel) {
        warn!(
            focused,
            active,
            frames,
            actions,
            "native touchpad peer focus pending; foreground watchdog will retry"
        );
    }
}

fn post_capture_control(message: u32) {
    let raw = CAPTURE_HWND.load(Ordering::Acquire);
    if raw == 0 {
        return;
    }
    if let Err(error) = unsafe {
        PostMessageW(
            Some(HWND(raw as *mut c_void)),
            message,
            WPARAM(0),
            LPARAM(0),
        )
    } {
        warn!(%error, message, "failed to queue touchpad keyboard capture control");
    }
}

fn suspend_capture_for_keyboard() {
    if !KEYBOARD_CAPTURE_SUSPENDED.swap(true, Ordering::AcqRel) {
        post_capture_control(WM_TOUCHPAD_KEYBOARD_SUSPEND);
    }
}

fn resume_capture_after_keyboard() {
    if KEYBOARD_CAPTURE_SUSPENDED.swap(false, Ordering::AcqRel) {
        post_capture_control(WM_TOUCHPAD_KEYBOARD_RESUME);
    }
}

fn should_enable_native_capture(
    requested: bool,
    frames: bool,
    actions: bool,
    foreground_owned: bool,
) -> bool {
    requested && foreground_owned && (frames || actions)
}

/// Claim foreground from the thread that owns the capture window.
///
/// Windows normally rejects foreground stealing. Joining the current input
/// queue to the real foreground thread makes this a focus handoff within one
/// attached queue, which is the same mechanism used by the receiver when a
/// peer takes keyboard focus. The capture window is a nearly transparent 5×5
/// hit target under the anchored source cursor, so Windows routes two-finger
/// WM_POINTER frames to it without flashing MineShare UI.
fn claim_foreground(capture: HWND) -> bool {
    let started = Instant::now();
    position_capture_window(capture);
    if unsafe { GetForegroundWindow() } == capture {
        return true;
    }

    let current_thread = unsafe { GetCurrentThreadId() };
    let previous = unsafe { GetForegroundWindow() };
    let foreground_thread = if previous.0.is_null() {
        0
    } else {
        unsafe { GetWindowThreadProcessId(previous, None) }
    };
    let attached = foreground_thread != 0
        && foreground_thread != current_thread
        && unsafe { AttachThreadInput(current_thread, foreground_thread, true) }.as_bool();

    unsafe {
        let _ = ShowWindow(capture, SW_SHOW);
        let _ = SetActiveWindow(capture);
        let _ = SetFocus(Some(capture));
        let _ = BringWindowToTop(capture);
        let _ = SetForegroundWindow(capture);
    }
    let mut owned = unsafe { GetForegroundWindow() } == capture;
    if !owned {
        // SetForegroundWindow is advisory and may be refused under the
        // foreground-lock timeout even while the input queues are attached.
        // SwitchToThisWindow follows the same activation path Windows uses
        // for an Alt+Tab selection and avoids synthesizing a visible Alt key.
        unsafe { SwitchToThisWindow(capture, true) };
        owned = unsafe { GetForegroundWindow() } == capture;
    }

    if attached {
        let _ = unsafe { AttachThreadInput(current_thread, foreground_thread, false) };
    }
    info!(
        owned,
        elapsed_us = started.elapsed().as_micros() as u64,
        "touchpad capture foreground claim"
    );
    owned
}

fn capture_window_rect(anchor_x: i32, anchor_y: i32) -> (i32, i32, i32, i32) {
    (
        anchor_x - CAPTURE_WINDOW_SIZE / 2,
        anchor_y - CAPTURE_WINDOW_SIZE / 2,
        CAPTURE_WINDOW_SIZE,
        CAPTURE_WINDOW_SIZE,
    )
}

fn position_capture_window(capture: HWND) {
    let (x, y, width, height) = capture_window_rect(
        CAPTURE_ANCHOR_X.load(Ordering::Acquire),
        CAPTURE_ANCHOR_Y.load(Ordering::Acquire),
    );
    if let Err(error) = unsafe {
        SetWindowPos(
            capture,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
        )
    } {
        warn!(
            %error,
            x,
            y,
            width,
            height,
            "failed to position native touchpad capture surface"
        );
    }
}

fn restore_foreground() {
    let started = Instant::now();
    let previous_raw = PREVIOUS_FOREGROUND_HWND.swap(0, Ordering::AcqRel);
    if previous_raw == 0 {
        return;
    }
    let previous = HWND(previous_raw as *mut c_void);
    let current_thread = unsafe { GetCurrentThreadId() };
    let target_thread = unsafe { GetWindowThreadProcessId(previous, None) };
    let attached = target_thread != 0
        && target_thread != current_thread
        && unsafe { AttachThreadInput(current_thread, target_thread, true) }.as_bool();
    let restored = unsafe { SetForegroundWindow(previous) }.as_bool()
        || unsafe { GetForegroundWindow() } == previous;
    if attached {
        let _ = unsafe { AttachThreadInput(current_thread, target_thread, false) };
    }
    info!(
        restored,
        elapsed_us = started.elapsed().as_micros() as u64,
        "restored local foreground after peer touchpad focus"
    );
}

fn create_capture_window() -> Result<HWND> {
    let api = capture_api()?;
    let class_name: Vec<u16> = "MineShareTouchpadCapture\0".encode_utf16().collect();
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(touchpad_wnd_proc),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..std::mem::zeroed()
        };
        let _ = RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
        .context("create touchpad capture window")?;
        // Alpha 0 layered windows are excluded from pointer hit-testing.
        // Alpha 1 is visually imperceptible but keeps the 5×5 capture surface
        // eligible for the WM_POINTER routing demonstrated by Microsoft's
        // visible-window Precision Touchpad sample.
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA)
            .context("make touchpad capture window nearly transparent")?;
        anyhow::ensure!(
            (api.register_window)(hwnd, true.into()).as_bool(),
            "RegisterTouchpadCapableWindow failed"
        );
        Ok(hwnd)
    }
}

unsafe extern "system" fn touchpad_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TOUCHPAD_CAPTURE_ROUTE {
        let requested = wparam.0 != 0
            && CAPTURE_REQUESTED.load(Ordering::Acquire)
            && !KEYBOARD_CAPTURE_SUSPENDED.load(Ordering::Acquire);
        apply_capture_active(requested);
        return LRESULT(1);
    }
    if msg == WM_TOUCHPAD_KEYBOARD_SUSPEND {
        apply_capture_active(false);
        return LRESULT(1);
    }
    if msg == WM_TOUCHPAD_KEYBOARD_RESUME {
        apply_capture_active(CAPTURE_REQUESTED.load(Ordering::Acquire));
        return LRESULT(1);
    }
    if msg == WM_TIMER && wparam.0 == CAPTURE_FOCUS_TIMER_ID {
        if CAPTURE_REQUESTED.load(Ordering::Acquire)
            && !KEYBOARD_CAPTURE_SUSPENDED.load(Ordering::Acquire)
        {
            if unsafe { GetForegroundWindow() } != hwnd || !CAPTURE_ENABLED.load(Ordering::Acquire)
            {
                apply_capture_active(true);
            }
        } else {
            apply_capture_active(false);
        }
        poll_reserved_alt_tab();
        return LRESULT(0);
    }
    if CAPTURE_ENABLED.load(Ordering::Acquire)
        && let Some((scan, vk, extended, down)) = legacy_key_from_message(msg, wparam, lparam)
    {
        if scan == 0x38 {
            LEGACY_ALT_HELD.store(down, Ordering::Release);
        }
        if raw_keyboard_owns_legacy_tab(scan) {
            // WM_INPUT owns this physical chord. Ignoring its corresponding
            // legacy message prevents a second Tab edge from advancing the
            // app switcher twice.
            return LRESULT(0);
        }
        if !legacy_capture_may_forward(scan) {
            // The Shell sees Win/Meta before this foreground fallback window.
            // Forwarding it here would open Start locally and remotely. The
            // consumable low-level hook owns standalone Win; Raw Input only
            // tracks it for reserved-chord recovery.
            return LRESULT(0);
        }
        let mut routed = super::route_touchpad_capture_key(scan, vk, extended, down);
        if should_neutralize_routed_legacy_shift(scan, down, routed)
            && let Err(error) = neutralize_capture_thread_shift_state()
        {
            warn!(
                ?error,
                scan, "failed to neutralize routed fallback Shift-down"
            );
        }
        if matches!(scan, 0x2a | 0x36) {
            info!(
                scan,
                extended, down, routed, "source Shift foreground fallback edge"
            );
        }
        if matches!(scan, 0x0f | 0x38) {
            info!(
                scan,
                extended, down, routed, "source Alt+Tab foreground fallback edge"
            );
        }
        if routed {
            // This edge already entered Windows' local key-state machine
            // before reaching the foreground fallback window. Remember its
            // down edge so a matching up that is later recovered by the
            // low-level hook is forwarded to both the peer and local Windows.
            note_legacy_passthrough_edge(scan, extended, down);
        }
        if should_recover_reserved_alt_tab(
            scan,
            down,
            routed,
            LEGACY_ALT_HELD.load(Ordering::Acquire),
        ) {
            // If Windows silently removes a low-level hook, WM_SYSKEYUP still
            // arrives after Shell consumed Tab's key-down. Reconstruct the
            // missing edge while Alt is held, producing the canonical peer
            // sequence: Alt down → Tab down/up → Alt up.
            let recovered_down = super::route_touchpad_capture_key(scan, vk, extended, true);
            let recovered_up = super::route_touchpad_capture_key(scan, vk, extended, false);
            routed = recovered_down && recovered_up;
            if routed {
                info!("recovered shell-reserved Alt+Tab key-down for peer");
            }
        }
        if routed {
            return LRESULT(0);
        }
    }
    if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) {
        let pointer_id = (wparam.0 & 0xffff) as u32;
        let mut pointer = POINTER_INFO::default();
        if unsafe { GetPointerInfo(pointer_id, &mut pointer) }.is_ok()
            && pointer.pointerType == PT_TOUCHPAD
        {
            if CAPTURE_ENABLED.load(Ordering::Acquire)
                && let Err(e) = process_touchpad_history(pointer_id)
            {
                debug!(error = %e, pointer_id, "touchpad frame history read failed");
            }
            let _ = unsafe { SkipPointerFrameMessages(pointer_id) };
            return LRESULT(0);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn should_recover_reserved_alt_tab(scan: u32, down: bool, routed: bool, alt_held: bool) -> bool {
    scan == 0x0f && !down && !routed && alt_held
}

fn legacy_capture_may_forward(scan: u32) -> bool {
    !matches!(scan, 0x5b | 0x5c)
}

pub(super) fn note_forwarded_key(code: u16, down: bool) {
    match code {
        15 => ASYNC_TAB_FORWARDED.store(down, Ordering::Release),
        31 => ASYNC_S_FORWARDED.store(down, Ordering::Release),
        42 => set_modifier_bit(&LEGACY_SHIFT_HELD, 0x01, down),
        54 => set_modifier_bit(&LEGACY_SHIFT_HELD, 0x02, down),
        56 | 100 => LEGACY_ALT_HELD.store(down, Ordering::Release),
        125 => {
            set_modifier_bit(&LEGACY_META_HELD, 0x01, down);
            set_modifier_bit(&ASYNC_META_FORWARDED, 0x01, down);
        }
        126 => {
            set_modifier_bit(&LEGACY_META_HELD, 0x02, down);
            set_modifier_bit(&ASYNC_META_FORWARDED, 0x02, down);
        }
        _ => {}
    }

    match keyboard_capture_action(code, down, no_keyboard_capture_modifiers_held()) {
        KeyboardCaptureAction::Suspend => {
            // A foreground Precision Touchpad window changes how Windows
            // dispatches Shell shortcuts. Temporarily restore the user's real
            // foreground as soon as the first modifier arrives; this makes the
            // cursor-crossing path identical to Smart routing, where
            // Win+Shift+S already works reliably.
            suspend_capture_for_keyboard();
        }
        KeyboardCaptureAction::Resume => resume_capture_after_keyboard(),
        KeyboardCaptureAction::None => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyboardCaptureAction {
    None,
    Suspend,
    Resume,
}

fn keyboard_capture_action(
    code: u16,
    down: bool,
    no_modifiers_held: bool,
) -> KeyboardCaptureAction {
    if !matches!(code, 42 | 54 | 56 | 100 | 125 | 126) {
        return KeyboardCaptureAction::None;
    }
    if down {
        KeyboardCaptureAction::Suspend
    } else if no_modifiers_held {
        KeyboardCaptureAction::Resume
    } else {
        KeyboardCaptureAction::None
    }
}

fn no_keyboard_capture_modifiers_held() -> bool {
    LEGACY_SHIFT_HELD.load(Ordering::Acquire) == 0
        && LEGACY_META_HELD.load(Ordering::Acquire) == 0
        && !LEGACY_ALT_HELD.load(Ordering::Acquire)
}

fn set_modifier_bit(state: &AtomicU8, bit: u8, down: bool) {
    if down {
        state.fetch_or(bit, Ordering::AcqRel);
    } else {
        state.fetch_and(!bit, Ordering::AcqRel);
    }
}

pub(super) fn set_raw_keyboard_available(available: bool) {
    RAW_KEYBOARD_AVAILABLE.store(available, Ordering::Release);
}

fn raw_alt_tab_edge(
    capture_active: bool,
    alt_down: bool,
    scan: u32,
    down: bool,
    tab_forwarded: bool,
) -> Option<bool> {
    if !capture_active || scan != 0x0f || (!alt_down && !tab_forwarded) {
        return None;
    }
    (down != tab_forwarded).then_some(down)
}

fn raw_snipping_edge(
    capture_requested: bool,
    shift_down: bool,
    meta_down: bool,
    scan: u32,
    down: bool,
    s_forwarded: bool,
) -> Option<bool> {
    let _ = (capture_requested, shift_down, meta_down);
    if scan != 0x1f || down {
        return None;
    }
    s_forwarded.then_some(false)
}

fn raw_meta_edge(_capture_requested: bool, down: bool, forwarded: bool) -> Option<bool> {
    (!down && forwarded).then_some(false)
}

pub(super) fn handle_raw_keyboard(scan: u32, vk: u32, extended: bool, down: bool) -> bool {
    match scan {
        0x2a => {
            let forwarded = LEGACY_SHIFT_HELD.load(Ordering::Acquire) & 0x01 != 0;
            if !down && forwarded {
                return super::route_touchpad_capture_key(scan, VK_LSHIFT.0 as u32, false, false);
            }
            return false;
        }
        0x36 => {
            let forwarded = LEGACY_SHIFT_HELD.load(Ordering::Acquire) & 0x02 != 0;
            if !down && forwarded {
                return super::route_touchpad_capture_key(scan, VK_RSHIFT.0 as u32, false, false);
            }
            return false;
        }
        0x5b | 0x5c => {
            let bit = if scan == 0x5b { 0x01 } else { 0x02 };
            let forwarded = ASYNC_META_FORWARDED.load(Ordering::Acquire) & bit != 0;
            let Some(edge) =
                raw_meta_edge(CAPTURE_REQUESTED.load(Ordering::Acquire), down, forwarded)
            else {
                return false;
            };
            let routed = super::route_touchpad_capture_key(scan, vk, extended, edge);
            if routed || !edge {
                set_modifier_bit(&ASYNC_META_FORWARDED, bit, edge && routed);
            }
            return routed;
        }
        _ => {}
    }

    if scan == 0x1f {
        let shift_down = LEGACY_SHIFT_HELD.load(Ordering::Acquire) != 0;
        let meta_down = LEGACY_META_HELD.load(Ordering::Acquire) != 0;
        let s_forwarded = ASYNC_S_FORWARDED.load(Ordering::Acquire);
        let Some(edge) = raw_snipping_edge(
            CAPTURE_REQUESTED.load(Ordering::Acquire),
            shift_down,
            meta_down,
            scan,
            down,
            s_forwarded,
        ) else {
            return false;
        };
        let routed = super::route_touchpad_capture_key(scan, vk, extended, edge);
        if routed || !edge {
            ASYNC_S_FORWARDED.store(edge && routed, Ordering::Release);
        }
        if routed && !edge {
            info!("recovered shell-reserved S release for peer");
        }
        return routed;
    }

    if scan == 0x38 {
        RAW_ALT_HELD.store(down, Ordering::Release);
        info!(down, "source Alt raw-input edge");
        return false;
    }
    if scan != 0x0f {
        return false;
    }

    let tab_forwarded = ASYNC_TAB_FORWARDED.load(Ordering::Acquire);
    let alt_down = RAW_ALT_HELD.load(Ordering::Acquire)
        || LEGACY_ALT_HELD.load(Ordering::Acquire)
        || unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } < 0;
    let edge = raw_alt_tab_edge(
        CAPTURE_REQUESTED.load(Ordering::Acquire),
        alt_down,
        scan,
        down,
        tab_forwarded,
    );
    let Some(edge) = edge else {
        info!(
            down,
            alt_down, tab_forwarded, "source Tab raw-input edge ignored"
        );
        return false;
    };
    let routed = super::route_touchpad_capture_key(scan, vk, extended, edge);
    info!(
        down,
        edge, routed, alt_down, tab_forwarded, "source Alt+Tab raw-input edge"
    );
    if routed || !edge {
        ASYNC_TAB_FORWARDED.store(edge && routed, Ordering::Release);
    }
    routed
}

fn raw_keyboard_owns_legacy_tab(scan: u32) -> bool {
    scan == 0x0f
        && RAW_KEYBOARD_AVAILABLE.load(Ordering::Acquire)
        && (RAW_ALT_HELD.load(Ordering::Acquire)
            || LEGACY_ALT_HELD.load(Ordering::Acquire)
            || ASYNC_TAB_FORWARDED.load(Ordering::Acquire))
}

fn reserved_alt_tab_poll_edge(
    capture_active: bool,
    alt_down: bool,
    tab_down: bool,
    tab_forwarded: bool,
) -> Option<bool> {
    if !tab_forwarded {
        return (capture_active && alt_down && tab_down).then_some(true);
    }
    (!capture_active || !tab_down).then_some(false)
}

fn poll_reserved_alt_tab() {
    let capture_active = CAPTURE_ENABLED.load(Ordering::Acquire);
    let alt_down = LEGACY_ALT_HELD.load(Ordering::Acquire)
        || unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } < 0;
    let tab_down = unsafe { GetAsyncKeyState(VK_TAB.0 as i32) } < 0;
    let tab_forwarded = ASYNC_TAB_FORWARDED.load(Ordering::Acquire);
    let Some(down) = reserved_alt_tab_poll_edge(capture_active, alt_down, tab_down, tab_forwarded)
    else {
        return;
    };

    let routed = super::route_touchpad_capture_key(0x0f, VK_TAB.0 as u32, false, down);
    info!(down, routed, "source Alt+Tab polling fallback edge");
    if routed || !down {
        // `route_touchpad_capture_key` synchronises successful edges through
        // `note_forwarded_key`. A failed release means another capture path
        // already released it, so clear the polling latch as well.
        ASYNC_TAB_FORWARDED.store(down && routed, Ordering::Release);
    }
}

fn legacy_key_from_message(
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<(u32, u32, bool, bool)> {
    let down = match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return None,
    };
    let bits = lparam.0 as u64;
    let scan = ((bits >> 16) & 0xff) as u32;
    let extended = bits & (1 << 24) != 0;
    Some((scan, wparam.0 as u32, extended, down))
}

fn process_touchpad_history(pointer_id: u32) -> Result<()> {
    let api = capture_api()?;
    let mut frame_count = 0u32;
    let mut pointer_count = 0u32;
    anyhow::ensure!(
        unsafe {
            (api.frame_history)(
                pointer_id,
                &mut frame_count,
                &mut pointer_count,
                std::ptr::null_mut(),
            )
        }
        .as_bool(),
        "query touchpad frame history size failed"
    );
    anyhow::ensure!(
        frame_count > 0
            && frame_count <= 128
            && pointer_count > 0
            && pointer_count <= MAX_TOUCHPAD_CONTACTS as u32,
        "malformed touchpad frame history dimensions"
    );

    let total = usize::try_from(frame_count.saturating_mul(pointer_count))
        .context("touchpad history length overflow")?;
    let mut infos = vec![POINTER_TOUCH_INFO::default(); total];
    anyhow::ensure!(
        unsafe {
            (api.frame_history)(
                pointer_id,
                &mut frame_count,
                &mut pointer_count,
                infos.as_mut_ptr(),
            )
        }
        .as_bool(),
        "read touchpad frame history failed"
    );
    let pointer_count = pointer_count as usize;
    let mut frames = infos
        .chunks_exact(pointer_count)
        .map(|frame| {
            let stamp = frame
                .iter()
                .map(|info| info.pointerInfo.PerformanceCount)
                .min()
                .unwrap_or(0);
            let contacts = frame
                .iter()
                .filter_map(normalize_win32_contact)
                .collect::<Vec<_>>();
            (stamp, contacts)
        })
        .collect::<Vec<_>>();
    // History APIs return newest-first on current Windows builds. Sorting by
    // the monotonic hardware timestamp makes this resilient to that detail.
    frames.sort_by_key(|(stamp, _)| *stamp);
    for (_, contacts) in frames {
        publish_snapshot(contacts);
    }
    Ok(())
}

fn normalize_win32_contact(info: &POINTER_TOUCH_INFO) -> Option<TouchpadContact> {
    let pointer = &info.pointerInfo;
    let active = pointer.pointerFlags.0 & POINTER_FLAG_INCONTACT.0 != 0
        && pointer.pointerFlags.0 & POINTER_FLAG_UP.0 == 0;
    if !active {
        return None;
    }
    let mut device = RECT::default();
    let mut display = RECT::default();
    unsafe { GetPointerDeviceRects(pointer.sourceDevice, &mut device, &mut display) }.ok()?;
    Some(TouchpadContact {
        id: pointer.pointerId,
        x: normalize_i32(pointer.ptHimetricLocationRaw.x, device.left, device.right),
        y: normalize_i32(pointer.ptHimetricLocationRaw.y, device.top, device.bottom),
    })
}

pub(super) fn normalize_i32(value: i32, start: i32, end: i32) -> u16 {
    let span = i64::from(end) - i64::from(start);
    if span <= 0 {
        return 0;
    }
    let offset = (i64::from(value) - i64::from(start)).clamp(0, span);
    ((offset * i64::from(u16::MAX)) / span) as u16
}

fn normalize_f32(value: f32, start: f32, extent: f32) -> u16 {
    if !value.is_finite() || !start.is_finite() || !extent.is_finite() || extent <= 0.0 {
        return 0;
    }
    (((value - start) / extent).clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

fn physical_contact(point: &PointerPoint) -> Option<TouchpadContact> {
    let id = point.PointerId().ok()?;
    if point.IsPhysicalPositionSupported().ok() == Some(true) {
        let position = point.PhysicalPosition().ok()?;
        let rect = point.PointerDevice().ok()?.PhysicalDeviceRect().ok()?;
        return Some(TouchpadContact {
            id,
            x: normalize_f32(position.X, rect.X, rect.Width),
            y: normalize_f32(position.Y, rect.Y, rect.Height),
        });
    }
    // Build 26200 also permits querying the corresponding Win32 pointer
    // during a controller callback. Keep this path for devices/drivers that
    // expose controller events but not IPointerPointPhysicalPosition.
    let mut pointer = POINTER_INFO::default();
    unsafe { GetPointerInfo(id, &mut pointer) }.ok()?;
    let mut device = RECT::default();
    let mut display = RECT::default();
    unsafe { GetPointerDeviceRects(pointer.sourceDevice, &mut device, &mut display) }.ok()?;
    Some(TouchpadContact {
        id,
        x: normalize_i32(pointer.ptHimetricLocationRaw.x, device.left, device.right),
        y: normalize_i32(pointer.ptHimetricLocationRaw.y, device.top, device.bottom),
    })
}

fn setup_global_controller() -> Result<TouchpadGesturesController> {
    anyhow::ensure!(
        TouchpadGesturesController::IsSupported()?,
        "TouchpadGesturesController is not supported by this Windows build"
    );
    let controller = TouchpadGesturesController::CreateForProcess()?;
    controller.SetSupportedGestures(
        TouchpadGlobalGestureKinds::ThreeFingerActions
            | TouchpadGlobalGestureKinds::FourFingerActions
            | TouchpadGlobalGestureKinds::ThreeFingerManipulations
            | TouchpadGlobalGestureKinds::FourFingerManipulations,
    )?;
    controller.SetEnabled(false)?;

    controller.GlobalActionPerformed(&TypedEventHandler::<
        TouchpadGesturesController,
        TouchpadGlobalActionEventArgs,
    >::new(|_, args| {
        if CAPTURE_ENABLED.load(Ordering::Acquire)
            && let Some(event) = action_event(args.ok()?.Action()?)
        {
            super::sink_send(InputEvent::Touchpad(event));
        }
        Ok(())
    }))?;

    controller.PointerPressed(&TypedEventHandler::<
        TouchpadGesturesController,
        PointerEventArgs,
    >::new(|_, args| {
        if CAPTURE_ENABLED.load(Ordering::Acquire) {
            let point = args.ok()?.CurrentPoint()?;
            if let Some(contact) = physical_contact(&point) {
                publish_contact_update(ContactUpdate::Press(contact));
            }
        }
        Ok(())
    }))?;

    controller.PointerMoved(&TypedEventHandler::<
        TouchpadGesturesController,
        PointerEventArgs,
    >::new(|_, args| {
        if !CAPTURE_ENABLED.load(Ordering::Acquire) {
            return Ok(());
        }
        let points = args.ok()?.GetIntermediatePoints()?;
        let size = points.Size()?;
        // GetIntermediatePoints is newest-first; replay oldest-first so the
        // peer sees every coalesced motion sample in temporal order.
        for index in (0..size).rev() {
            let point = points.GetAt(index)?;
            if let Some(contact) = physical_contact(&point) {
                publish_contact_update(ContactUpdate::Move(contact));
            }
        }
        Ok(())
    }))?;

    controller.PointerReleased(&TypedEventHandler::<
        TouchpadGesturesController,
        PointerEventArgs,
    >::new(|_, args| {
        if CAPTURE_ENABLED.load(Ordering::Acquire) {
            let point = args.ok()?.CurrentPoint()?;
            publish_contact_update(ContactUpdate::Release(point.PointerId()?));
        }
        Ok(())
    }))?;

    Ok(controller)
}

fn action_event(action: TouchpadGlobalAction) -> Option<TouchpadEvent> {
    let (fingers, action) = if action == TouchpadGlobalAction::ThreeFingerTap {
        (3, TouchpadAction::Tap)
    } else if action == TouchpadGlobalAction::FourFingerTap {
        (4, TouchpadAction::Tap)
    } else if action == TouchpadGlobalAction::ThreeFingerPressDown {
        (3, TouchpadAction::Press)
    } else if action == TouchpadGlobalAction::FourFingerPressDown {
        (4, TouchpadAction::Press)
    } else if action == TouchpadGlobalAction::ThreeFingerPressUp {
        (3, TouchpadAction::Release)
    } else if action == TouchpadGlobalAction::FourFingerPressUp {
        (4, TouchpadAction::Release)
    } else {
        return None;
    };
    Some(TouchpadEvent::Action { fingers, action })
}

enum ContactUpdate {
    Press(TouchpadContact),
    Move(TouchpadContact),
    Release(u32),
}

fn publish_contact_update(update: ContactUpdate) {
    let (contacts, should_publish) = {
        let mut state = CAPTURE_STATE.lock();
        match update {
            ContactUpdate::Press(contact) | ContactUpdate::Move(contact) => {
                state.contacts.insert(contact.id, contact);
            }
            ContactUpdate::Release(id) => {
                state.contacts.remove(&id);
            }
        }
        let contacts = state.contacts.values().copied().collect::<Vec<_>>();
        // TouchpadGesturesController reports the contacts one callback at a
        // time. Do not begin a synthetic gesture with the first/second
        // callback: Windows could classify that early frame as a one-finger
        // pointer gesture before the third contact arrives. Once a 3/4-finger
        // stream has started, smaller trailing snapshots are required to
        // release each contact correctly.
        let should_publish = state.stream_id.is_some() || contacts.len() >= 3;
        (contacts, should_publish)
    };
    if should_publish {
        publish_snapshot(contacts);
    }
}

fn publish_snapshot(mut contacts: Vec<TouchpadContact>) {
    if !CAPTURE_ENABLED.load(Ordering::Acquire) {
        return;
    }
    contacts.sort_by_key(|contact| contact.id);
    contacts.dedup_by_key(|contact| contact.id);
    if contacts.len() > MAX_TOUCHPAD_CONTACTS {
        warn!(
            count = contacts.len(),
            "discarding malformed touchpad contact frame"
        );
        return;
    }

    let events = CAPTURE_STATE.lock().snapshot(&contacts);
    for event in events {
        super::sink_send(InputEvent::Touchpad(event));
    }
}

fn publish_cancel() {
    let events = CAPTURE_STATE.lock().cancel();
    for event in events {
        super::sink_send(InputEvent::Touchpad(event));
    }
}

struct CaptureState {
    next_stream_id: u32,
    stream_id: Option<u32>,
    contacts: BTreeMap<u32, TouchpadContact>,
    scroll_transform: TwoFingerScrollTransform,
}

#[derive(Clone, Copy)]
struct TwoFingerScrollTransform {
    contact_ids: Option<[u32; 2]>,
    source_centroid: [f32; 2],
    output_centroid: [f32; 2],
}

impl TwoFingerScrollTransform {
    const fn new() -> Self {
        Self {
            contact_ids: None,
            source_centroid: [0.0; 2],
            output_centroid: [0.0; 2],
        }
    }

    fn reset(&mut self) {
        self.contact_ids = None;
    }

    /// Scale only the translation shared by both contacts. Their offsets from
    /// the current centroid are copied unchanged, which keeps pinch distance
    /// and rotation geometry native while making two-finger pan/scroll faster
    /// or slower.
    fn apply(&mut self, contacts: &[TouchpadContact], speed: f32) -> Vec<TouchpadContact> {
        if contacts.len() != 2 {
            self.reset();
            return contacts.to_vec();
        }

        let ids = [contacts[0].id, contacts[1].id];
        let centroid = [
            (f32::from(contacts[0].x) + f32::from(contacts[1].x)) * 0.5,
            (f32::from(contacts[0].y) + f32::from(contacts[1].y)) * 0.5,
        ];
        if self.contact_ids != Some(ids) {
            self.contact_ids = Some(ids);
            self.source_centroid = centroid;
            self.output_centroid = centroid;
            return contacts.to_vec();
        }

        let speed = if speed.is_finite() {
            speed.clamp(0.25, 4.0)
        } else {
            1.0
        };
        self.output_centroid[0] += (centroid[0] - self.source_centroid[0]) * speed;
        self.output_centroid[1] += (centroid[1] - self.source_centroid[1]) * speed;
        self.source_centroid = centroid;

        // Clamp the transformed centroid as a unit so neither contact clips
        // independently at the device edge and changes the pinch distance.
        let mut min_x = 0.0f32;
        let mut max_x = f32::from(u16::MAX);
        let mut min_y = 0.0f32;
        let mut max_y = f32::from(u16::MAX);
        for contact in contacts {
            let relative_x = f32::from(contact.x) - centroid[0];
            let relative_y = f32::from(contact.y) - centroid[1];
            min_x = min_x.max(-relative_x);
            max_x = max_x.min(f32::from(u16::MAX) - relative_x);
            min_y = min_y.max(-relative_y);
            max_y = max_y.min(f32::from(u16::MAX) - relative_y);
        }
        self.output_centroid[0] = self.output_centroid[0].clamp(min_x, max_x);
        self.output_centroid[1] = self.output_centroid[1].clamp(min_y, max_y);

        contacts
            .iter()
            .map(|contact| TouchpadContact {
                id: contact.id,
                x: (self.output_centroid[0] + f32::from(contact.x) - centroid[0])
                    .round()
                    .clamp(0.0, f32::from(u16::MAX)) as u16,
                y: (self.output_centroid[1] + f32::from(contact.y) - centroid[1])
                    .round()
                    .clamp(0.0, f32::from(u16::MAX)) as u16,
            })
            .collect()
    }
}

impl CaptureState {
    const fn new() -> Self {
        Self {
            next_stream_id: 1,
            stream_id: None,
            contacts: BTreeMap::new(),
            scroll_transform: TwoFingerScrollTransform::new(),
        }
    }

    fn reset_without_event(&mut self) {
        self.stream_id = None;
        self.contacts.clear();
        self.scroll_transform.reset();
    }

    fn snapshot(&mut self, contacts: &[TouchpadContact]) -> Vec<TouchpadEvent> {
        self.contacts = contacts
            .iter()
            .map(|contact| (contact.id, *contact))
            .collect();
        if contacts.is_empty() {
            return self.cancel();
        }
        // One-finger pointer, tap, physical click and drag deliberately stay
        // on the low-latency mouse hook path. Starting a synthetic touchpad
        // stream for that same contact makes Windows emit a second click on
        // the peer when the source driver also produces WM_LBUTTON*. Once a
        // real multi-contact stream has started, however, a trailing single
        // contact must still be published so the peer can release the other
        // finger without leaving the gesture stuck.
        if self.stream_id.is_none() && contacts.len() < 2 {
            self.scroll_transform.reset();
            return Vec::new();
        }
        let stream_id = *self.stream_id.get_or_insert_with(|| {
            let id = self.next_stream_id;
            self.next_stream_id = self.next_stream_id.wrapping_add(1).max(1);
            id
        });
        let contacts = self
            .scroll_transform
            .apply(contacts, crate::touchpad_scroll_speed());
        TouchpadEvent::frame(stream_id, &contacts, false)
            .into_iter()
            .collect()
    }

    fn cancel(&mut self) -> Vec<TouchpadEvent> {
        self.contacts.clear();
        self.scroll_transform.reset();
        let Some(stream_id) = self.stream_id.take() else {
            return Vec::new();
        };
        vec![TouchpadEvent::terminal(stream_id); TERMINAL_REPEAT_COUNT]
    }
}

pub(super) fn inject(event: TouchpadEvent) -> Result<()> {
    match event {
        TouchpadEvent::Action { fingers, action } => inject_action(fingers, action),
        TouchpadEvent::Frame {
            stream_id,
            count,
            contacts,
            terminal,
        } => {
            let count = usize::from(count);
            anyhow::ensure!(
                count <= MAX_TOUCHPAD_CONTACTS
                    && ((terminal && count == 0) || (!terminal && count > 0)),
                "malformed touchpad contact count"
            );
            let contacts = &contacts[..count];
            let unique = contacts
                .iter()
                .map(|contact| contact.id)
                .collect::<HashSet<_>>();
            anyhow::ensure!(
                unique.len() == contacts.len(),
                "duplicate touchpad contact id"
            );
            INJECTION_STATE
                .lock()
                .apply_frame(stream_id, contacts, terminal)
        }
    }
}

pub(super) fn release_all() -> Result<()> {
    INJECTION_STATE.lock().release_all()
}

fn inject_action(fingers: u8, action: TouchpadAction) -> Result<()> {
    let action_code = match (fingers, action) {
        (3, TouchpadAction::Tap) => 0,
        (3, TouchpadAction::Press) => 1,
        (3, TouchpadAction::Release) => 2,
        (4, TouchpadAction::Tap) => 3,
        (4, TouchpadAction::Press) => 4,
        (4, TouchpadAction::Release) => 5,
        _ => anyhow::bail!("unsupported direct touchpad action"),
    };
    let api = injection_api()?;
    let device = HSYNTHETICPOINTERDEVICE(api.device as *mut c_void);
    anyhow::ensure!(
        unsafe { (api.inject_action)(device, action_code) }.as_bool(),
        "InjectTouchpadAction failed"
    );
    let mut state = INJECTION_STATE.lock();
    match action {
        TouchpadAction::Press => {
            state.held_actions.insert(fingers);
        }
        TouchpadAction::Release => {
            state.held_actions.remove(&fingers);
        }
        TouchpadAction::Tap => {}
    }
    Ok(())
}

struct InjectionState {
    stream_id: Option<u32>,
    contacts: BTreeMap<u32, TouchpadContact>,
    held_actions: BTreeSet<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContactPhase {
    Down,
    Update,
    Up,
}

#[derive(Debug)]
struct LifecyclePlan {
    release_before: Vec<TouchpadContact>,
    frame: Vec<(TouchpadContact, ContactPhase)>,
    next_stream_id: Option<u32>,
    next_contacts: BTreeMap<u32, TouchpadContact>,
    ignored: bool,
}

fn plan_lifecycle(
    current_stream_id: Option<u32>,
    current_contacts: &BTreeMap<u32, TouchpadContact>,
    stream_id: u32,
    contacts: &[TouchpadContact],
    terminal: bool,
) -> LifecyclePlan {
    if terminal && current_stream_id != Some(stream_id) {
        return LifecyclePlan {
            release_before: Vec::new(),
            frame: Vec::new(),
            next_stream_id: current_stream_id,
            next_contacts: current_contacts.clone(),
            ignored: true,
        };
    }

    let stream_changed = current_stream_id.is_some_and(|current| current != stream_id);
    let release_before = if stream_changed {
        current_contacts.values().copied().collect()
    } else {
        Vec::new()
    };
    let baseline = if stream_changed {
        BTreeMap::new()
    } else {
        current_contacts.clone()
    };
    let next_contacts = contacts
        .iter()
        .map(|contact| (contact.id, *contact))
        .collect::<BTreeMap<_, _>>();
    let mut frame = Vec::with_capacity(baseline.len() + next_contacts.len());
    for contact in next_contacts.values() {
        let phase = if baseline.contains_key(&contact.id) {
            ContactPhase::Update
        } else {
            ContactPhase::Down
        };
        frame.push((*contact, phase));
    }
    for contact in baseline.values() {
        if !next_contacts.contains_key(&contact.id) {
            frame.push((*contact, ContactPhase::Up));
        }
    }
    let next_stream_id = if terminal || next_contacts.is_empty() {
        None
    } else {
        Some(stream_id)
    };
    LifecyclePlan {
        release_before,
        frame,
        next_stream_id,
        next_contacts,
        ignored: false,
    }
}

impl InjectionState {
    const fn new() -> Self {
        Self {
            stream_id: None,
            contacts: BTreeMap::new(),
            held_actions: BTreeSet::new(),
        }
    }

    fn apply_frame(
        &mut self,
        stream_id: u32,
        contacts: &[TouchpadContact],
        terminal: bool,
    ) -> Result<()> {
        let plan = plan_lifecycle(
            self.stream_id,
            &self.contacts,
            stream_id,
            contacts,
            terminal,
        );
        if plan.ignored {
            return Ok(());
        }
        if !plan.release_before.is_empty() {
            let releases = plan
                .release_before
                .iter()
                .map(|contact| {
                    pointer_type_info(*contact, POINTER_FLAG_CONFIDENCE | POINTER_FLAG_UP)
                })
                .collect::<Vec<_>>();
            if let Err(error) = inject_pointer_frame(&releases) {
                self.contacts.clear();
                self.stream_id = None;
                return Err(error);
            }
        }
        let frame = plan
            .frame
            .iter()
            .map(|(contact, phase)| {
                let flags = match phase {
                    ContactPhase::Down => {
                        POINTER_FLAG_INRANGE
                            | POINTER_FLAG_INCONTACT
                            | POINTER_FLAG_CONFIDENCE
                            | POINTER_FLAG_DOWN
                    }
                    ContactPhase::Update => {
                        POINTER_FLAG_INRANGE
                            | POINTER_FLAG_INCONTACT
                            | POINTER_FLAG_CONFIDENCE
                            | POINTER_FLAG_UPDATE
                    }
                    ContactPhase::Up => POINTER_FLAG_CONFIDENCE | POINTER_FLAG_UP,
                };
                pointer_type_info(*contact, flags)
            })
            .collect::<Vec<_>>();
        if !frame.is_empty()
            && let Err(error) = inject_pointer_frame(&frame)
        {
            self.contacts.clear();
            self.stream_id = None;
            return Err(error);
        }
        self.stream_id = plan.next_stream_id;
        self.contacts = plan.next_contacts;
        Ok(())
    }

    fn release_contacts(&mut self) -> Result<()> {
        let release = self
            .contacts
            .values()
            .map(|contact| pointer_type_info(*contact, POINTER_FLAG_CONFIDENCE | POINTER_FLAG_UP))
            .collect::<Vec<_>>();
        self.contacts.clear();
        self.stream_id = None;
        if release.is_empty() {
            return Ok(());
        }
        inject_pointer_frame(&release)
    }

    fn release_all(&mut self) -> Result<()> {
        let contact_result = self.release_contacts();
        let actions = self.held_actions.iter().copied().collect::<Vec<_>>();
        self.held_actions.clear();
        let mut action_error = None;
        for fingers in actions {
            if let Err(e) = inject_action_without_tracking(fingers, TouchpadAction::Release) {
                action_error = Some(e);
            }
        }
        contact_result?;
        if let Some(error) = action_error {
            return Err(error);
        }
        Ok(())
    }
}

fn inject_action_without_tracking(fingers: u8, action: TouchpadAction) -> Result<()> {
    let action_code = match (fingers, action) {
        (3, TouchpadAction::Release) => 2,
        (4, TouchpadAction::Release) => 5,
        _ => anyhow::bail!("unsupported touchpad cleanup action"),
    };
    let api = injection_api()?;
    let device = HSYNTHETICPOINTERDEVICE(api.device as *mut c_void);
    anyhow::ensure!(
        unsafe { (api.inject_action)(device, action_code) }.as_bool(),
        "InjectTouchpadAction cleanup failed"
    );
    Ok(())
}

fn inject_pointer_frame(frame: &[POINTER_TYPE_INFO]) -> Result<()> {
    let api = injection_api()?;
    let device = HSYNTHETICPOINTERDEVICE(api.device as *mut c_void);
    unsafe { InjectSyntheticPointerInput(device, frame) }.context("InjectSyntheticPointerInput")
}

fn pointer_type_info(contact: TouchpadContact, flags: POINTER_FLAGS) -> POINTER_TYPE_INFO {
    let position = POINT {
        x: (u32::from(contact.x) * DEVICE_WIDTH / u32::from(u16::MAX)) as i32,
        y: (u32::from(contact.y) * DEVICE_HEIGHT / u32::from(u16::MAX)) as i32,
    };
    let pointer = POINTER_INFO {
        pointerType: PT_TOUCHPAD,
        pointerId: contact.id,
        pointerFlags: flags,
        ptHimetricLocation: position,
        ptHimetricLocationRaw: position,
        ..Default::default()
    };
    POINTER_TYPE_INFO {
        r#type: PT_TOUCHPAD,
        Anonymous: POINTER_TYPE_INFO_0 {
            touchInfo: POINTER_TOUCH_INFO {
                pointerInfo: pointer,
                ..Default::default()
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_clamps_and_scales() {
        assert_eq!(normalize_i32(-1, 0, 100), 0);
        assert_eq!(normalize_i32(0, 0, 100), 0);
        assert_eq!(normalize_i32(50, 0, 100), 32_767);
        assert_eq!(normalize_i32(100, 0, 100), u16::MAX);
        assert_eq!(normalize_i32(101, 0, 100), u16::MAX);
        assert_eq!(normalize_i32(50, 100, 100), 0);
    }

    #[test]
    fn two_finger_scroll_speed_scales_centroid_translation_only() {
        let mut transform = TwoFingerScrollTransform::new();
        let initial = [
            TouchpadContact {
                id: 1,
                x: 10_000,
                y: 20_000,
            },
            TouchpadContact {
                id: 2,
                x: 30_000,
                y: 40_000,
            },
        ];
        assert_eq!(transform.apply(&initial, 2.0), initial);

        let moved = [
            TouchpadContact {
                x: 11_000,
                y: 19_500,
                ..initial[0]
            },
            TouchpadContact {
                x: 31_000,
                y: 39_500,
                ..initial[1]
            },
        ];
        let scaled = transform.apply(&moved, 2.0);
        assert_eq!(
            scaled,
            vec![
                TouchpadContact {
                    x: 12_000,
                    y: 19_000,
                    ..initial[0]
                },
                TouchpadContact {
                    x: 32_000,
                    y: 39_000,
                    ..initial[1]
                },
            ]
        );
        assert_eq!(
            i32::from(scaled[1].x) - i32::from(scaled[0].x),
            i32::from(moved[1].x) - i32::from(moved[0].x)
        );
        assert_eq!(
            i32::from(scaled[1].y) - i32::from(scaled[0].y),
            i32::from(moved[1].y) - i32::from(moved[0].y)
        );
    }

    #[test]
    fn two_finger_scroll_speed_preserves_symmetric_pinch() {
        let mut transform = TwoFingerScrollTransform::new();
        let initial = [
            TouchpadContact {
                id: 10,
                x: 10_000,
                y: 30_000,
            },
            TouchpadContact {
                id: 11,
                x: 30_000,
                y: 30_000,
            },
        ];
        transform.apply(&initial, 3.0);
        let pinched = [
            TouchpadContact {
                x: 8_000,
                ..initial[0]
            },
            TouchpadContact {
                x: 32_000,
                ..initial[1]
            },
        ];
        assert_eq!(transform.apply(&pinched, 3.0), pinched);
    }

    #[test]
    fn scroll_transform_bypasses_three_finger_frames() {
        let mut transform = TwoFingerScrollTransform::new();
        let pair = [
            TouchpadContact { id: 1, x: 1, y: 2 },
            TouchpadContact { id: 2, x: 3, y: 4 },
        ];
        transform.apply(&pair, 2.0);
        let three = [pair[0], pair[1], TouchpadContact { id: 3, x: 5, y: 6 }];
        assert_eq!(transform.apply(&three, 2.0), three);
        assert_eq!(transform.contact_ids, None);
    }

    #[test]
    fn native_capture_does_not_start_from_a_single_contact() {
        let mut state = CaptureState::new();
        let first = TouchpadContact {
            id: 7,
            x: 10,
            y: 20,
        };
        let second = TouchpadContact {
            id: 8,
            x: 30,
            y: 40,
        };

        assert!(
            state.snapshot(&[first]).is_empty(),
            "one-finger pointer/click must remain on the low-latency mouse path"
        );
        assert!(
            state.snapshot(&[]).is_empty(),
            "an ignored one-finger contact must not create a terminal stream"
        );

        let begin = state.snapshot(&[first, second]);
        assert_eq!(begin.len(), 1, "two contacts start native gesture capture");
        assert_eq!(
            state.snapshot(&[first]).len(),
            1,
            "after a two-finger stream starts, trailing releases remain observable"
        );
        assert_eq!(state.snapshot(&[]).len(), TERMINAL_REPEAT_COUNT);
    }

    #[test]
    fn capture_lifecycle_repeats_terminal() {
        let mut state = CaptureState::new();
        let first = TouchpadContact {
            id: 7,
            x: 10,
            y: 20,
        };
        let second = TouchpadContact {
            id: 8,
            x: 30,
            y: 40,
        };
        let begin = state.snapshot(&[first, second]);
        let stream = begin[0].coalescible_stream().unwrap();
        assert_eq!(begin.len(), 1);
        assert_eq!(state.snapshot(&[first, second]).len(), 1);
        let terminal = state.snapshot(&[]);
        assert_eq!(
            terminal,
            vec![TouchpadEvent::terminal(stream); TERMINAL_REPEAT_COUNT]
        );
        assert!(state.cancel().is_empty());
    }

    #[test]
    fn new_capture_stream_gets_new_id_after_cancel() {
        let mut state = CaptureState::new();
        let contacts = [
            TouchpadContact { id: 1, x: 2, y: 3 },
            TouchpadContact { id: 2, x: 4, y: 5 },
        ];
        let first = state.snapshot(&contacts)[0].coalescible_stream().unwrap();
        state.cancel();
        let second = state.snapshot(&contacts)[0].coalescible_stream().unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn action_mapping_preserves_peer_semantics() {
        assert_eq!(
            action_event(TouchpadGlobalAction::ThreeFingerTap),
            Some(TouchpadEvent::Action {
                fingers: 3,
                action: TouchpadAction::Tap,
            })
        );
        assert_eq!(
            action_event(TouchpadGlobalAction::FourFingerPressUp),
            Some(TouchpadEvent::Action {
                fingers: 4,
                action: TouchpadAction::Release,
            })
        );
    }

    #[test]
    fn native_capture_requires_real_foreground_ownership() {
        assert!(
            !should_enable_native_capture(true, true, true, false),
            "advertised APIs are insufficient when Windows kept foreground on the laptop app"
        );
        assert!(should_enable_native_capture(true, true, true, true));
        assert!(!should_enable_native_capture(false, true, true, true));
        assert!(!should_enable_native_capture(true, false, false, true));
    }

    #[test]
    fn capture_surface_is_centered_under_anchor_for_pointer_hit_testing() {
        assert_eq!(capture_window_rect(1_440, 900), (1_438, 898, 5, 5));
        assert_eq!(capture_window_rect(-100, -50), (-102, -52, 5, 5));
    }

    #[test]
    fn foreground_capture_window_recovers_keyboard_messages() {
        let a_down =
            legacy_key_from_message(WM_KEYDOWN, WPARAM(0x41), LPARAM((0x1e_i64 << 16) as isize));
        assert_eq!(a_down, Some((0x1e, 0x41, false, true)));

        let right_up = legacy_key_from_message(
            WM_KEYUP,
            WPARAM(0x27),
            LPARAM(((0x4d_i64 << 16) | (1_i64 << 24) | (1_i64 << 31)) as isize),
        );
        assert_eq!(right_up, Some((0x4d, 0x27, true, false)));
        assert_eq!(
            legacy_key_from_message(0x0102, WPARAM(0x61), LPARAM(0)),
            None,
            "WM_CHAR must not create a duplicate key event"
        );
    }

    #[test]
    fn reserved_alt_tab_release_recovers_only_a_missing_down_edge() {
        assert!(should_recover_reserved_alt_tab(0x0f, false, false, true));
        assert!(!should_recover_reserved_alt_tab(0x0f, true, false, true));
        assert!(!should_recover_reserved_alt_tab(0x0f, false, true, true));
        assert!(!should_recover_reserved_alt_tab(0x0f, false, false, false));
        assert!(!should_recover_reserved_alt_tab(0x1c, false, false, true));
    }

    #[test]
    fn shell_reserved_alt_tab_replay_meets_interactive_latency_budget() {
        // Minimal event-driven replay: Raw Input delivers the physical make
        // code without waiting for Shell's missing legacy Tab-down or the
        // 10 ms safety poll. The measured source routing budget is 1 ms.
        let raw_events_us = [
            (0_u64, 0x38_u32, true),
            (640, 0x0f, true),
            (71_000, 0x0f, false),
        ];
        let mut alt_down = false;
        let mut tab_forwarded = false;
        let first_tab_dispatch_us = raw_events_us
            .into_iter()
            .find_map(|(at_us, scan, down)| {
                if scan == 0x38 {
                    alt_down = down;
                    return None;
                }
                let edge = raw_alt_tab_edge(true, alt_down, scan, down, tab_forwarded);
                if let Some(down) = edge {
                    tab_forwarded = down;
                }
                (edge == Some(true)).then_some(at_us)
            })
            .expect("Raw Input dispatches Tab-down");

        assert!(
            first_tab_dispatch_us <= 1_000,
            "Tab was not dispatched until {first_tab_dispatch_us} µs; source capture budget is 1 ms"
        );
    }

    #[test]
    fn reserved_alt_tab_poller_emits_one_down_and_one_release() {
        let mut forwarded = false;
        let mut edges = Vec::new();
        for tab_down in [false, true, true, true, false, false] {
            if let Some(edge) = reserved_alt_tab_poll_edge(true, true, tab_down, forwarded) {
                edges.push(edge);
                forwarded = edge;
            }
        }
        assert_eq!(edges, [true, false]);
        assert!(!forwarded);
    }

    #[test]
    fn raw_alt_tab_router_deduplicates_each_physical_edge() {
        assert_eq!(raw_alt_tab_edge(true, true, 0x0f, true, false), Some(true));
        assert_eq!(raw_alt_tab_edge(true, true, 0x0f, true, true), None);
        assert_eq!(
            raw_alt_tab_edge(true, false, 0x0f, false, true),
            Some(false)
        );
        assert_eq!(raw_alt_tab_edge(true, false, 0x0f, false, false), None);
        assert_eq!(raw_alt_tab_edge(true, false, 0x0f, true, false), None);
        assert_eq!(raw_alt_tab_edge(false, true, 0x0f, true, false), None);
        assert_eq!(raw_alt_tab_edge(true, true, 0x1c, true, false), None);
    }

    #[test]
    fn raw_snipping_router_recovers_release_only() {
        assert_eq!(raw_snipping_edge(true, true, true, 0x1f, true, false), None);
        assert_eq!(raw_snipping_edge(true, true, true, 0x1f, true, true), None);
        assert_eq!(
            raw_snipping_edge(true, false, false, 0x1f, false, true),
            Some(false)
        );
        assert_eq!(
            raw_snipping_edge(false, true, true, 0x1f, true, false),
            None
        );
        assert_eq!(
            raw_snipping_edge(true, true, false, 0x1f, true, false),
            None
        );
    }

    #[test]
    fn delayed_raw_snipping_down_cannot_replay_completed_hook_chord() {
        assert_eq!(
            raw_snipping_edge(true, true, true, 0x1f, true, false),
            None,
            "Raw Input is observational and may arrive after hook S-up cleared the current-state latch"
        );
    }

    #[test]
    fn raw_meta_router_recovers_only_missing_edges() {
        assert_eq!(raw_meta_edge(true, true, false), None);
        assert_eq!(raw_meta_edge(true, true, true), None);
        assert_eq!(raw_meta_edge(true, false, true), Some(false));
        assert_eq!(raw_meta_edge(false, false, true), Some(false));
        assert_eq!(raw_meta_edge(false, true, false), None);
    }

    #[test]
    fn raw_recovery_never_forwards_a_bare_win_down() {
        assert_eq!(
            raw_meta_edge(true, true, false),
            None,
            "Raw Input cannot consume the source Shell action, so forwarding bare Win here opens Start on both PCs"
        );
    }

    #[test]
    fn foreground_fallback_never_forwards_bare_shell_meta() {
        assert!(!legacy_capture_may_forward(0x5b));
        assert!(!legacy_capture_may_forward(0x5c));
        assert!(legacy_capture_may_forward(0x1e));
    }

    #[test]
    fn session_reset_clears_forwarded_modifier_latches() {
        LEGACY_SHIFT_HELD.store(0x03, Ordering::Release);
        LEGACY_META_HELD.store(0x03, Ordering::Release);
        ASYNC_META_FORWARDED.store(0x03, Ordering::Release);
        ASYNC_S_FORWARDED.store(true, Ordering::Release);
        LEGACY_ALT_HELD.store(true, Ordering::Release);
        ASYNC_TAB_FORWARDED.store(true, Ordering::Release);
        KEYBOARD_CAPTURE_SUSPENDED.store(true, Ordering::Release);

        reset_forwarded_keyboard_latches();

        assert_eq!(LEGACY_SHIFT_HELD.load(Ordering::Acquire), 0);
        assert_eq!(LEGACY_META_HELD.load(Ordering::Acquire), 0);
        assert_eq!(ASYNC_META_FORWARDED.load(Ordering::Acquire), 0);
        assert!(!ASYNC_S_FORWARDED.load(Ordering::Acquire));
        assert!(!LEGACY_ALT_HELD.load(Ordering::Acquire));
        assert!(!ASYNC_TAB_FORWARDED.load(Ordering::Acquire));
        assert!(!KEYBOARD_CAPTURE_SUSPENDED.load(Ordering::Acquire));
    }

    #[test]
    fn capture_disable_preserves_owned_release_latches() {
        LEGACY_SHIFT_HELD.store(0x01, Ordering::Release);
        LEGACY_META_HELD.store(0x01, Ordering::Release);
        ASYNC_META_FORWARDED.store(0x01, Ordering::Release);
        ASYNC_S_FORWARDED.store(true, Ordering::Release);
        KEYBOARD_CAPTURE_SUSPENDED.store(true, Ordering::Release);

        set_capture_active(false);

        assert_eq!(LEGACY_SHIFT_HELD.load(Ordering::Acquire), 0x01);
        assert_eq!(LEGACY_META_HELD.load(Ordering::Acquire), 0x01);
        assert_eq!(ASYNC_META_FORWARDED.load(Ordering::Acquire), 0x01);
        assert!(ASYNC_S_FORWARDED.load(Ordering::Acquire));
        assert!(!KEYBOARD_CAPTURE_SUSPENDED.load(Ordering::Acquire));
        reset_forwarded_keyboard_latches();
    }

    #[test]
    fn modifier_chord_suspends_foreground_capture_until_last_release() {
        assert_eq!(
            keyboard_capture_action(42, true, false),
            KeyboardCaptureAction::Suspend
        );
        assert_eq!(
            keyboard_capture_action(125, true, false),
            KeyboardCaptureAction::Suspend
        );
        assert_eq!(
            keyboard_capture_action(125, false, false),
            KeyboardCaptureAction::None
        );
        assert_eq!(
            keyboard_capture_action(42, false, true),
            KeyboardCaptureAction::Resume
        );
        assert_eq!(
            keyboard_capture_action(31, true, false),
            KeyboardCaptureAction::None
        );
    }

    #[test]
    fn routed_legacy_shift_down_is_neutralized_in_the_capture_thread() {
        for scan in [0x2a, 0x36] {
            assert!(should_neutralize_routed_legacy_shift(scan, true, true));
            assert!(!should_neutralize_routed_legacy_shift(scan, false, true));
            assert!(!should_neutralize_routed_legacy_shift(scan, true, false));
        }
        assert!(!should_neutralize_routed_legacy_shift(0x1e, true, true));
    }

    #[test]
    fn legacy_down_requires_matching_hook_release_to_pass_locally() {
        let mut tracker = LegacyPassthroughTracker::default();

        // The down edge already reached Windows through the foreground
        // fallback window. If the low-level hook later captures the up edge,
        // it must still let that up continue locally or Windows keeps the
        // physical modifier logically pressed on the source machine.
        tracker.note_legacy_edge(0x2a, false, true);
        assert!(tracker.take_hook_release(0x2a, false));
        assert!(
            !tracker.take_hook_release(0x2a, false),
            "one escaped down edge permits exactly one local release"
        );

        // Windows reports Right Shift's legacy WM_KEYDOWN without the
        // extended bit, while this hardware's low-level hook reports the
        // matching WM_KEYUP with LLKHF_EXTENDED. Scan 0x36 already identifies
        // Right Shift uniquely, so the identity must tolerate that mismatch.
        tracker.note_legacy_edge(0x36, false, true);
        assert!(
            tracker.take_hook_release(0x36, true),
            "Right Shift release must match across legacy/hook extended-bit disagreement"
        );
    }

    #[test]
    fn lifecycle_recovers_when_begin_or_update_packet_is_lost() {
        let first_seen = TouchpadContact {
            id: 10,
            x: 1_000,
            y: 2_000,
        };
        let begin_lost = plan_lifecycle(None, &BTreeMap::new(), 44, &[first_seen], false);
        assert_eq!(
            begin_lost.frame,
            vec![(first_seen, ContactPhase::Down)],
            "the first surviving snapshot must synthesize DOWN"
        );

        let current = begin_lost.next_contacts;
        let moved = TouchpadContact {
            x: 1_500,
            ..first_seen
        };
        let update_lost = plan_lifecycle(Some(44), &current, 44, &[moved], false);
        assert_eq!(
            update_lost.frame,
            vec![(moved, ContactPhase::Update)],
            "the latest absolute snapshot remains sufficient after update loss"
        );
    }

    #[test]
    fn terminal_and_new_stream_release_every_old_contact() {
        let old_a = TouchpadContact {
            id: 1,
            x: 100,
            y: 200,
        };
        let old_b = TouchpadContact {
            id: 2,
            x: 300,
            y: 400,
        };
        let current = [(old_a.id, old_a), (old_b.id, old_b)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let terminal = plan_lifecycle(Some(8), &current, 8, &[], true);
        assert_eq!(
            terminal
                .frame
                .iter()
                .map(|(_, phase)| *phase)
                .collect::<Vec<_>>(),
            vec![ContactPhase::Up, ContactPhase::Up]
        );
        assert!(terminal.next_contacts.is_empty());
        assert_eq!(terminal.next_stream_id, None);

        let new_contact = TouchpadContact {
            id: 9,
            x: 500,
            y: 600,
        };
        let replacement = plan_lifecycle(Some(8), &current, 9, &[new_contact], false);
        assert_eq!(replacement.release_before, vec![old_a, old_b]);
        assert_eq!(replacement.frame, vec![(new_contact, ContactPhase::Down)]);
        assert_eq!(replacement.next_stream_id, Some(9));

        let duplicate_terminal = plan_lifecycle(None, &BTreeMap::new(), 8, &[], true);
        assert!(duplicate_terminal.ignored);
        assert!(duplicate_terminal.frame.is_empty());
    }

    #[test]
    #[ignore = "requires the current Windows 11 Precision Touchpad injection API"]
    fn native_touchpad_injection_device_is_available() {
        injection_api().expect("create gesture-only synthetic touchpad");
    }

    #[test]
    #[ignore = "temporarily transfers Windows foreground to an off-screen capture window"]
    fn native_capture_window_can_take_foreground() {
        let _ = unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_SINGLETHREADED,
            )
        };
        let foreground_before = unsafe { GetForegroundWindow() };
        if foreground_before.0.is_null() {
            eprintln!("skipped: test process is not attached to the interactive input desktop");
            return;
        }
        setup_capture().expect("initialize native touchpad capture");
        crate::set_peer_touchpad_capabilities(
            TOUCHPAD_CAP_FRAME_INJECT | TOUCHPAD_CAP_ACTION_INJECT,
        );

        set_capture_active(true);
        let raw = CAPTURE_HWND.load(Ordering::Acquire);
        eprintln!(
            "foreground before={foreground_before:?} after={:?} capture={:?}",
            unsafe { GetForegroundWindow() },
            HWND(raw as *mut c_void)
        );
        assert_ne!(raw, 0, "capture window was not created");
        assert_eq!(
            unsafe { GetForegroundWindow() },
            HWND(raw as *mut c_void),
            "capture window did not acquire foreground"
        );
        assert!(
            CAPTURE_ENABLED.load(Ordering::Acquire),
            "native capture was not enabled after foreground handoff"
        );

        set_capture_active(false);
        crate::set_peer_touchpad_capabilities(0);
    }
}
