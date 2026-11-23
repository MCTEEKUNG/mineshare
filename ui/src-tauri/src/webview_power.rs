//! Put the WebView to sleep while nobody can see it.
//!
//! wry never tells WebView2 that the window was minimized or hidden to the
//! tray, so the page keeps running at full speed: every React poller, re-render
//! and the GPU compositor. Measured on the laptop with the window minimized:
//! the WebView2 processes used 4.8–44 % of a core and ~140 MB private memory.
//!
//! On Windows we mark the controller invisible, lower its memory target and
//! ask it to suspend (timers and script stop; page state is kept). Making it
//! visible again resumes it automatically. Other platforms are left alone:
//! WebKitGTK already follows the toplevel's mapped state.

/// Re-evaluate whether the main window's WebView should be awake.
pub fn sync(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        let minimized = window.is_minimized().unwrap_or(false);
        let shown = window.is_visible().unwrap_or(true);
        imp::apply(window, shown && !minimized);
    }
    #[cfg(not(windows))]
    {
        let _ = window;
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicU8, Ordering};
    use tracing::{debug, info};
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        ICoreWebView2_3, ICoreWebView2_19,
    };
    use webview2_com::TrySuspendCompletedHandler;
    use windows::core::Interface;

    const UNKNOWN: u8 = 0;
    const AWAKE: u8 = 1;
    const ASLEEP: u8 = 2;
    static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);

    pub(super) fn apply(window: &tauri::WebviewWindow, awake: bool) {
        let want = if awake { AWAKE } else { ASLEEP };
        if STATE.swap(want, Ordering::AcqRel) == want {
            return;
        }
        let result = window.with_webview(move |webview| unsafe {
            let controller = webview.controller();
            // Visibility first: TrySuspend is refused while the WebView is
            // visible, and setting it visible again is what resumes it.
            if let Err(error) = controller.SetIsVisible(awake) {
                debug!(%error, "WebView2 SetIsVisible failed");
            }
            let Ok(core) = controller.CoreWebView2() else {
                return;
            };
            if let Ok(core19) = core.cast::<ICoreWebView2_19>() {
                let level = if awake {
                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
                } else {
                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
                };
                let _ = core19.SetMemoryUsageTargetLevel(level);
            }
            if !awake && let Ok(core3) = core.cast::<ICoreWebView2_3>() {
                let handler = TrySuspendCompletedHandler::create(Box::new(|_, suspended| {
                    debug!(suspended, "WebView2 suspend finished");
                    Ok(())
                }));
                let _ = core3.TrySuspend(&handler);
            }
        });
        match result {
            Ok(()) => info!(awake, "main WebView activity updated"),
            Err(error) => {
                STATE.store(UNKNOWN, Ordering::Release);
                debug!(%error, "could not reach the main WebView");
            }
        }
    }
}
