//! Short, click-through desktop feedback. Never activates or focuses a window.
use crate::{LockAnimation, LockEffect};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{COLORREF, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

pub(crate) fn show(locked: bool) {
    show_configured(locked, *crate::LOCK_EFFECT.lock());
}

pub(crate) fn show_configured(locked: bool, effect: LockEffect) {
    tracing::info!(locked, ?effect, "lock feedback requested");
    static TX: OnceLock<mpsc::Sender<(bool, LockEffect)>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        if let Err(e) = std::thread::Builder::new()
            .name("lock-feedback".into())
            .spawn(move || {
                if let Err(e) = unsafe { run(rx) } {
                    #[cfg(test)]
                    eprintln!("lock feedback error: {e}");
                    tracing::warn!(error = %e, "lock feedback unavailable");
                }
            })
        {
            tracing::warn!(error = %e, "lock feedback worker unavailable");
        }
        tx
    });
    if tx.send((locked, effect.clamped())).is_err() {
        tracing::warn!("lock feedback channel closed");
    }
}

fn envelope(locked: bool, progress: f32) -> (u8, i32, i32) {
    let t = progress.clamp(0.0, 1.0);
    let alpha = if locked {
        (t / 0.12).min(1.0) * ((1.0 - t) / 0.35).min(1.0)
    } else {
        1.0 - t
    };
    let inset = if locked {
        12.0 * (1.0 - t / 0.2).max(0.0)
    } else {
        t * 18.0
    };
    (
        (alpha * 220.0) as u8,
        inset as i32,
        if locked {
            6
        } else {
            (6.0 * (1.0 - t)).max(1.0) as i32
        },
    )
}

fn styled_envelope(locked: bool, t: f32, effect: LockEffect) -> (u8, i32, i32) {
    let (mut alpha, mut inset, thickness) = envelope(locked, t);
    match effect.animation {
        LockAnimation::Flow => {}
        LockAnimation::Pulse => {
            alpha =
                (f32::from(alpha) * (0.65 + 0.35 * (t * std::f32::consts::TAU).sin().abs())) as u8;
        }
        LockAnimation::Fade => inset = 0,
    }
    (
        alpha,
        inset,
        (thickness * effect.thickness as i32 / 6).max(1),
    )
}

unsafe fn run(rx: mpsc::Receiver<(bool, LockEffect)>) -> windows::core::Result<()> {
    // All HWND and GDI operations stay on this worker, never the input hook.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hwnd = CreateWindowExW(
            // Establish the overlay's Z-order at creation, including when the
            // main application is minimized and cannot promote a normal window.
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("MineShare lock feedback"),
            WS_POPUP | WS_DISABLED,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )?;
        tracing::info!("lock feedback window created");
        let black = CreateSolidBrush(COLORREF(0));
        loop {
            // Keep the HWND responsive between animations, including display
            // changes/fullscreen transitions. A blocking recv starves its pump.
            let mut idle_msg = MSG::default();
            while PeekMessageW(&mut idle_msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&idle_msg);
                DispatchMessageW(&idle_msg);
            }
            let (mut locked, mut effect) = match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            while let Ok(latest) = rx.try_recv() {
                (locked, effect) = latest;
            }
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                tracing::warn!("lock feedback monitor lookup failed");
                continue;
            }
            let rect = info.rcMonitor;
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let surface = Surface::new(width, height)?;
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )?;
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let badge_width = 264 * dpi / 96;
            let badge_height = 56 * dpi / 96;
            // Cache both states once; rapid toggles reuse the same animation window.
            let badges = [status_badge(dpi, false)?, status_badge(dpi, true)?];
            let mut start = Instant::now();
            let mut frames = 0u32;
            tracing::info!(locked, width, height, "lock feedback animation started");
            loop {
                if let Ok(latest) = rx.try_recv() {
                    (locked, effect) = latest;
                    start = Instant::now();
                }
                let duration = effect.duration_ms as f32 / 1000.0 * if locked { 1.0 } else { 0.61 };
                let t = start.elapsed().as_secs_f32() / duration;
                if t >= 1.0 {
                    break;
                }
                let (alpha, inset, thickness) = styled_envelope(locked, t, effect);
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let dc = surface.dc;
                if !dc.0.is_null() {
                    let _ = FillRect(
                        dc,
                        &RECT {
                            left: 0,
                            top: 0,
                            right: width,
                            bottom: height,
                        },
                        black,
                    );
                    for i in 0..96 {
                        let hue = i as f32 / 96.0
                            + if effect.animation == LockAnimation::Flow {
                                t * 0.3
                            } else {
                                0.0
                            };
                        let channel = |offset: f32| {
                            (128.0 + 127.0 * ((hue + offset) * std::f32::consts::TAU).sin()) as u32
                        };
                        let rgb = if effect.rainbow {
                            channel(0.0) | channel(0.333) << 8 | channel(0.666) << 16
                        } else {
                            u32::from(effect.color[0])
                                | u32::from(effect.color[1]) << 8
                                | u32::from(effect.color[2]) << 16
                        };
                        // Pure black is the transparency key; near-black remains visible.
                        let brush = CreateSolidBrush(COLORREF(rgb.max(1)));
                        let x0 = inset + (width - 2 * inset) * i / 96;
                        let x1 = inset + (width - 2 * inset) * (i + 1) / 96;
                        let y0 = inset + (height - 2 * inset) * i / 96;
                        let y1 = inset + (height - 2 * inset) * (i + 1) / 96;
                        for edge in [
                            RECT {
                                left: x0,
                                top: inset,
                                right: x1,
                                bottom: inset + thickness,
                            },
                            RECT {
                                left: x0,
                                top: height - inset - thickness,
                                right: x1,
                                bottom: height - inset,
                            },
                            RECT {
                                left: inset,
                                top: y0,
                                right: inset + thickness,
                                bottom: y1,
                            },
                            RECT {
                                left: width - inset - thickness,
                                top: y0,
                                right: width - inset,
                                bottom: y1,
                            },
                        ] {
                            let _ = FillRect(dc, &edge, brush);
                        }
                        let _ = DeleteObject(brush.into());
                    }
                }
                BitBlt(
                    dc,
                    (width - badge_width) / 2,
                    32 * dpi / 96,
                    badge_width,
                    badge_height,
                    Some(badges[usize::from(locked)].dc),
                    0,
                    0,
                    SRCCOPY,
                )?;
                // Present a complete off-screen frame. Drawing into the STATIC
                // window's DC did not reliably reach the desktop compositor.
                let _ = GdiFlush();
                for pixel in
                    std::slice::from_raw_parts_mut(surface.pixels, (width * height) as usize)
                {
                    *pixel = if *pixel & 0x00ff_ffff == 0 {
                        0
                    } else {
                        *pixel | 0xff00_0000
                    };
                }
                UpdateLayeredWindow(
                    hwnd,
                    None,
                    Some(&POINT {
                        x: rect.left,
                        y: rect.top,
                    }),
                    Some(&SIZE {
                        cx: width,
                        cy: height,
                    }),
                    Some(dc),
                    Some(&POINT::default()),
                    COLORREF(0),
                    Some(&BLENDFUNCTION {
                        BlendOp: AC_SRC_OVER as u8,
                        BlendFlags: 0,
                        SourceConstantAlpha: alpha,
                        AlphaFormat: AC_SRC_ALPHA as u8,
                    }),
                    ULW_ALPHA,
                )?;
                frames += 1;
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )?;
                std::thread::sleep(Duration::from_millis(16));
            }
            let _ = ShowWindow(hwnd, SW_HIDE);
            tracing::info!(
                frames,
                topmost = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0,
                elapsed_ms = start.elapsed().as_millis(),
                "lock feedback animation finished"
            );
        }
        let _ = DeleteObject(black.into());
        let _ = DestroyWindow(hwnd);
        Ok(())
    }
}

fn status_text(locked: bool) -> &'static str {
    if locked {
        "GAME MODE ON"
    } else {
        "GAME MODE OFF"
    }
}

unsafe fn status_badge(dpi: i32, locked: bool) -> windows::core::Result<Surface> {
    unsafe {
        let px = |v: i32| v * dpi / 96;
        let badge = Surface::new(px(264), px(56))?;
        let font = CreateFontW(
            -px(18),
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            w!("Segoe UI"),
        );
        if font.0.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let brush = CreateSolidBrush(COLORREF(0x00251e18));
        let old_brush = SelectObject(badge.dc, brush.into());
        let old_pen = SelectObject(badge.dc, GetStockObject(NULL_PEN));
        let _ = RoundRect(badge.dc, 0, 0, px(264), px(56), px(28), px(28));
        let _ = SelectObject(badge.dc, old_brush);
        let _ = DeleteObject(brush.into());
        let dot = CreateSolidBrush(COLORREF(if locked { 0x00b4e55e } else { 0x00b8aca0 }));
        let _ = SelectObject(badge.dc, dot.into());
        let _ = Ellipse(badge.dc, px(22), px(23), px(32), px(33));
        let _ = SelectObject(badge.dc, old_brush);
        let _ = SelectObject(badge.dc, old_pen);
        let _ = DeleteObject(dot.into());
        let old_font = SelectObject(badge.dc, font.into());
        SetBkMode(badge.dc, TRANSPARENT);
        SetTextColor(badge.dc, COLORREF(0x00faf7f5));
        let mut text: Vec<u16> = status_text(locked).encode_utf16().collect();
        let mut rect = RECT {
            left: px(42),
            top: 0,
            right: px(250),
            bottom: px(56),
        };
        let drawn = DrawTextW(
            badge.dc,
            &mut text,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(badge.dc, old_font);
        let _ = DeleteObject(font.into());
        if drawn == 0 {
            return Err(windows::core::Error::from_thread());
        }
        Ok(badge)
    }
}

// Own the animation's back buffer so all early-error paths release GDI objects.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u32,
}
impl Surface {
    unsafe fn new(width: i32, height: i32) -> windows::core::Result<Self> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.0.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = std::ptr::null_mut();
            let bitmap =
                match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut pixels, None, 0) {
                    Ok(bitmap) => bitmap,
                    Err(error) => {
                        let _ = DeleteDC(dc);
                        return Err(error);
                    }
                };
            let previous = SelectObject(dc, bitmap.into());
            Ok(Self {
                dc,
                bitmap,
                previous,
                pixels: pixels.cast(),
            })
        }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_badge_renders_both_labels_at_display_scales() {
        assert_eq!(status_text(true), "GAME MODE ON");
        assert_eq!(status_text(false), "GAME MODE OFF");
        for dpi in [96, 144, 192] {
            for locked in [false, true] {
                unsafe {
                    let badge = status_badge(dpi, locked).unwrap();
                    let _ = GdiFlush();
                    let pixels = std::slice::from_raw_parts(
                        badge.pixels,
                        (264 * dpi / 96 * 56 * dpi / 96) as usize,
                    );
                    assert!(
                        pixels.iter().filter(|p| **p & 0xffffff == 0xf5f7fa).count() > 20,
                        "label must contain readable text pixels"
                    );
                    assert_eq!(pixels[0] & 0xffffff, 0, "rounded corners stay transparent");
                }
            }
        }
    }
    #[test]
    fn custom_styles_remain_bounded_and_always_disappear() {
        let effect = LockEffect {
            duration_ms: u32::MAX,
            thickness: u32::MAX,
            ..Default::default()
        }
        .clamped();
        assert_eq!(effect.duration_ms, 3000);
        assert_eq!(effect.thickness, 16);
        for animation in [
            LockAnimation::Flow,
            LockAnimation::Pulse,
            LockAnimation::Fade,
        ] {
            for locked in [true, false] {
                assert_eq!(
                    styled_envelope(
                        locked,
                        1.0,
                        LockEffect {
                            animation,
                            ..effect
                        }
                    )
                    .0,
                    0
                );
            }
        }
        assert_eq!(
            styled_envelope(
                true,
                0.05,
                LockEffect {
                    animation: LockAnimation::Fade,
                    ..effect
                }
            )
            .1,
            0
        );
    }
    #[test]
    #[ignore = "briefly displays desktop rainbow feedback in the interactive session"]
    fn desktop_feedback_never_activates_and_hides_after_animation() {
        unsafe {
            let before = GetForegroundWindow();
            show(true);
            std::thread::sleep(Duration::from_millis(250));
            let hwnd = FindWindowW(None, w!("MineShare lock feedback")).unwrap();
            assert!(IsWindowVisible(hwnd).as_bool());
            assert_eq!(GetForegroundWindow(), before);
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            assert_ne!(style & WS_EX_NOACTIVATE.0, 0);
            assert_ne!(style & WS_EX_TRANSPARENT.0, 0);
            assert_ne!(style & WS_EX_TOPMOST.0, 0);
            std::thread::sleep(Duration::from_millis(1200));
            assert!(!IsWindowVisible(hwnd).as_bool());
            show(false);
            std::thread::sleep(Duration::from_millis(900));
            assert!(!IsWindowVisible(hwnd).as_bool());
            assert_eq!(GetForegroundWindow(), before);
            assert_ne!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            assert_ne!(
                SendMessageTimeoutW(
                    hwnd,
                    WM_NULL,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    250,
                    None
                )
                .0,
                0,
                "the hidden feedback window must keep processing messages between animations"
            );
        }
    }
    #[test]
    #[ignore = "samples desktop edge pixels while showing a brief red border"]
    fn desktop_feedback_paints_visible_pixels() {
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let mut point = POINT::default();
            GetCursorPos(&mut point).unwrap();
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            assert!(
                GetMonitorInfoW(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST), &mut info)
                    .as_bool()
            );
            show_configured(
                true,
                LockEffect {
                    rainbow: false,
                    color: [255, 0, 0],
                    animation: LockAnimation::Fade,
                    thickness: 16,
                    duration_ms: 1500,
                },
            );
            std::thread::sleep(Duration::from_millis(400));
            let hwnd = FindWindowW(None, w!("MineShare lock feedback")).unwrap();
            let mut bounds = RECT::default();
            GetWindowRect(hwnd, &mut bounds).unwrap();
            eprintln!(
                "feedback visible={} bounds={bounds:?}",
                IsWindowVisible(hwnd).as_bool()
            );
            let dc = GetDC(None);
            let sample = Surface::new(1, 1).unwrap();
            BitBlt(
                sample.dc,
                0,
                0,
                1,
                1,
                Some(dc),
                info.rcMonitor.left + 4,
                info.rcMonitor.top + 100,
                SRCCOPY | CAPTUREBLT,
            )
            .unwrap();
            let pixel = GetPixel(sample.dc, 0, 0).0;
            std::thread::sleep(Duration::from_millis(1300));
            BitBlt(
                sample.dc,
                0,
                0,
                1,
                1,
                Some(dc),
                info.rcMonitor.left + 4,
                info.rcMonitor.top + 100,
                SRCCOPY | CAPTUREBLT,
            )
            .unwrap();
            eprintln!("hidden edge COLORREF={:06x}", GetPixel(sample.dc, 0, 0).0);
            let _ = ReleaseDC(None, dc);
            eprintln!("desktop edge COLORREF={pixel:06x}");
            assert!(
                (pixel & 255) > 180 && ((pixel >> 8) & 255) < 80 && ((pixel >> 16) & 255) < 80,
                "red feedback must be visible on the composed desktop, got {pixel:06x}"
            );
        }
    }
    #[test]
    #[ignore = "manual 15-second overlay observation without changing lock state"]
    fn desktop_feedback_visual_probe() {
        for _ in 0..5 {
            show_configured(
                true,
                LockEffect {
                    duration_ms: 3000,
                    thickness: 16,
                    ..Default::default()
                },
            );
            std::thread::sleep(Duration::from_millis(3000));
        }
    }
    #[test]
    fn feedback_is_brief_and_unlock_relaxes() {
        assert_eq!(envelope(true, 0.0).0, 0);
        assert!(envelope(true, 0.3).0 > 0);
        assert_eq!(envelope(true, 1.0).0, 0);
        assert_eq!(envelope(false, 1.0).0, 0);
        assert!(envelope(false, 0.8).1 > envelope(false, 0.2).1);
        assert!(envelope(false, 0.8).2 < envelope(false, 0.2).2);
    }
}
