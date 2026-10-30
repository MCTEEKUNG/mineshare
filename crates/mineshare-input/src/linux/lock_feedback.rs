//! Transient X11/XWayland border strips: no focus, input, grabs or capture.
use crate::{LockAnimation, LockEffect};
use anyhow::{Context, Result, ensure};
use std::{
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        shape::{ConnectionExt as _, SK, SO},
        xproto::*,
    },
    wrapper::ConnectionExt as _,
};

pub(crate) fn show_configured(locked: bool, effect: LockEffect) {
    static TX: OnceLock<Option<mpsc::Sender<(bool, LockEffect)>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("lock-feedback".into()).spawn(move || {
            while let Ok(event) = rx.recv() {
                if let Err(error) = render(&rx, event) {
                    tracing::warn!(%error, "Linux lock feedback unavailable (requires X11 or XWayland DISPLAY)");
                }
            }
        }).map(|_| tx).map_err(|error| tracing::warn!(%error, "Linux lock feedback worker unavailable")).ok()
    });
    if let Some(tx) = tx {
        let _ = tx.send((locked, effect.clamped()));
    }
}

#[derive(Clone, Copy)]
struct Monitor {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn envelope(locked: bool, progress: f32, effect: LockEffect) -> (f32, u32) {
    let t = progress.clamp(0.0, 1.0);
    let mut alpha = if locked {
        (t / 0.12).min(1.0) * ((1.0 - t) / 0.35).min(1.0)
    } else {
        1.0 - t
    };
    if effect.animation == LockAnimation::Pulse {
        alpha *= 0.65 + 0.35 * (t * std::f32::consts::TAU).sin().abs();
    }
    let inset = if effect.animation == LockAnimation::Fade {
        0.0
    } else if locked {
        12.0 * (1.0 - t / 0.2).max(0.0)
    } else {
        t * 18.0
    };
    (alpha * 0.86, inset as u32)
}

fn segment(m: Monitor, edge: usize, part: u32, inset: u32, thickness: u32) -> (i32, i32, u32, u32) {
    let inset = inset.min(m.width.min(m.height).saturating_sub(2) / 2);
    let w = m.width.saturating_sub(2 * inset).max(1);
    let h = m.height.saturating_sub(2 * inset).max(1);
    let thick = thickness.min(w).min(h).max(1);
    let length = if edge < 2 { w } else { h };
    let a = length * part / 16;
    let b = length * (part + 1) / 16;
    let x = m.x + inset as i32;
    let y = m.y + inset as i32;
    match edge {
        0 => (x + a as i32, y, (b - a).max(1), thick),
        1 => (x + a as i32, y + (h - thick) as i32, (b - a).max(1), thick),
        2 => (x, y + a as i32, thick, (b - a).max(1)),
        _ => (x + (w - thick) as i32, y + a as i32, thick, (b - a).max(1)),
    }
}

fn border_bounds(m: Monitor, inset: u32, thickness: u32) -> [Rectangle; 4] {
    let inset = inset.min(m.width.min(m.height).saturating_sub(2) / 2);
    let w = m.width.saturating_sub(2 * inset).max(1) as u16;
    let h = m.height.saturating_sub(2 * inset).max(1) as u16;
    let thick = thickness.min(u32::from(w)).min(u32::from(h)).max(1) as u16;
    let x = inset as i16;
    let y = x;
    [
        Rectangle {
            x,
            y,
            width: w,
            height: thick,
        },
        Rectangle {
            x,
            y: y + (h - thick) as i16,
            width: w,
            height: thick,
        },
        Rectangle {
            x,
            y,
            width: thick,
            height: h,
        },
        Rectangle {
            x: x + (w - thick) as i16,
            y,
            width: thick,
            height: h,
        },
    ]
}

fn channel(value: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    ((u32::from(value) * (mask >> mask.trailing_zeros()) / 255) << mask.trailing_zeros()) & mask
}

fn render(rx: &mpsc::Receiver<(bool, LockEffect)>, mut event: (bool, LockEffect)) -> Result<()> {
    let (conn, index) = x11rb::connect(None).context("Open desktop X11/XWayland display")?;
    let screen = &conn.setup().roots[index];
    conn.shape_query_version()?
        .reply()
        .context("Input-transparent overlay requires SHAPE")?;
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == screen.root_visual)
        .context("Root visual")?;
    ensure!(
        visual.class == VisualClass::TRUE_COLOR,
        "Feedback requires a TrueColor visual"
    );
    let geometry = conn.get_geometry(screen.root)?.reply()?;
    let monitors: Vec<Monitor> = conn
        .randr_get_monitors(screen.root, true)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| {
            r.monitors
                .into_iter()
                .filter(|m| m.width > 0 && m.height > 0)
                .map(|m| Monitor {
                    x: i32::from(m.x),
                    y: i32::from(m.y),
                    width: u32::from(m.width),
                    height: u32::from(m.height),
                })
                .collect()
        })
        .filter(|m: &Vec<_>| !m.is_empty())
        .unwrap_or_else(|| {
            vec![Monitor {
                x: 0,
                y: 0,
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
            }]
        });
    let atom = |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let marker = atom(b"_MINESHARE_LOCK_FEEDBACK")?;
    let opacity = atom(b"_NET_WM_WINDOW_OPACITY")?;
    let window_type = atom(b"_NET_WM_WINDOW_TYPE")?;
    let notification = atom(b"_NET_WM_WINDOW_TYPE_NOTIFICATION")?;
    // One stable compositor surface per monitor. Paint offscreen and copy
    // only the border, rather than resize/restack 64 independent surfaces.
    struct Surface {
        window: u32,
        buffer: u32,
        gcs: Vec<u32>,
        monitor: Monitor,
        last_paint: Option<(u32, LockEffect)>,
        last_inset: Option<(u32, u32)>,
    }
    let mut surfaces = Vec::new();
    for m in &monitors {
        let window = conn.generate_id()?;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            m.x as i16,
            m.y as i16,
            m.width as u16,
            m.height as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0),
        )?;
        conn.shape_rectangles(
            SO::SET,
            SK::INPUT,
            ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[],
        )?;
        // Hide the initial full-screen background until the first frame is ready.
        conn.shape_rectangles(
            SO::SET,
            SK::BOUNDING,
            ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[],
        )?;
        conn.change_property32(PropMode::REPLACE, window, marker, AtomEnum::CARDINAL, &[1])?;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            window_type,
            AtomEnum::ATOM,
            &[notification],
        )?;
        conn.change_property32(PropMode::REPLACE, window, opacity, AtomEnum::CARDINAL, &[0])?;
        let buffer = conn.generate_id()?;
        conn.create_pixmap(
            screen.root_depth,
            buffer,
            window,
            m.width as u16,
            m.height as u16,
        )?;
        let mut gcs = Vec::new();
        for _ in 0..16 {
            let gc = conn.generate_id()?;
            conn.create_gc(gc, buffer, &CreateGCAux::new().graphics_exposures(0))?;
            gcs.push(gc);
        }
        conn.map_window(window)?;
        // Stack once. Restacking every frame stalls Mutter/XWayland.
        conn.configure_window(
            window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        surfaces.push(Surface {
            window,
            buffer,
            gcs,
            monitor: *m,
            last_paint: None,
            last_inset: None,
        });
    }
    conn.flush()?;
    tracing::info!(
        monitors = surfaces.len(),
        "Linux lock feedback shown (one click-through surface per monitor)"
    );
    let mut started = Instant::now();
    let frame_interval = Duration::from_secs_f64(1.0 / 60.0);
    let mut next_frame = started;
    loop {
        while let Ok(next) = rx.try_recv() {
            event = next;
            started = Instant::now();
        }
        let (locked, effect) = event;
        let duration = effect.duration_ms as f32 * if locked { 1.0 } else { 0.61 };
        let t = started.elapsed().as_secs_f32() * 1000.0 / duration;
        if t >= 1.0 {
            break;
        }
        let (alpha, inset) = envelope(locked, t, effect);
        for surface in &mut surfaces {
            let m = Monitor {
                x: 0,
                y: 0,
                ..surface.monitor
            };
            let bounds = border_bounds(m, inset, effect.thickness);
            if surface.last_inset != Some((inset, effect.thickness)) {
                conn.shape_rectangles(
                    SO::SET,
                    SK::BOUNDING,
                    ClipOrdering::UNSORTED,
                    surface.window,
                    0,
                    0,
                    &bounds,
                )?;
                surface.last_inset = Some((inset, effect.thickness));
            }
            let moving_color = effect.rainbow && effect.animation == LockAnimation::Flow;
            if moving_color || surface.last_paint != Some((inset, effect)) {
                if effect.rainbow {
                    for part in 0..16 {
                        let hue = part as f32 / 16.0 + if moving_color { t * 0.3 } else { 0.0 };
                        let color = [0.0, 0.333, 0.666].map(|offset| {
                            (128.0 + 127.0 * ((hue + offset) * std::f32::consts::TAU).sin()) as u8
                        });
                        let pixel = channel(color[0], visual.red_mask)
                            | channel(color[1], visual.green_mask)
                            | channel(color[2], visual.blue_mask);
                        let gc = surface.gcs[part as usize];
                        conn.change_gc(gc, &ChangeGCAux::new().foreground(pixel))?;
                        let rects: Vec<Rectangle> = (0..4)
                            .map(|edge| {
                                let (x, y, w, h) = segment(
                                    m,
                                    edge,
                                    (part + edge as u32 * 4) % 16,
                                    inset,
                                    effect.thickness,
                                );
                                Rectangle {
                                    x: x as i16,
                                    y: y as i16,
                                    width: w as u16,
                                    height: h as u16,
                                }
                            })
                            .collect();
                        conn.poly_fill_rectangle(surface.buffer, gc, &rects)?;
                    }
                } else {
                    let pixel = channel(effect.color[0], visual.red_mask)
                        | channel(effect.color[1], visual.green_mask)
                        | channel(effect.color[2], visual.blue_mask);
                    conn.change_gc(surface.gcs[0], &ChangeGCAux::new().foreground(pixel))?;
                    conn.poly_fill_rectangle(surface.buffer, surface.gcs[0], &bounds)?;
                }
                for rect in &bounds {
                    conn.copy_area(
                        surface.buffer,
                        surface.window,
                        surface.gcs[0],
                        rect.x,
                        rect.y,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                    )?;
                }
                surface.last_paint = Some((inset, effect));
            }
            conn.change_property32(
                PropMode::REPLACE,
                surface.window,
                opacity,
                AtomEnum::CARDINAL,
                &[(alpha * u32::MAX as f32) as u32],
            )?;
        }
        conn.flush()?;
        next_frame += frame_interval;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else {
            next_frame = now;
        }
    }
    for surface in surfaces {
        conn.destroy_window(surface.window)?;
        conn.free_pixmap(surface.buffer)?;
        for gc in surface.gcs {
            conn.free_gc(gc)?;
        }
    }
    conn.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feedback_ends_transparent_and_geometry_stays_inside_monitor() {
        for animation in [
            LockAnimation::Flow,
            LockAnimation::Pulse,
            LockAnimation::Fade,
        ] {
            let effect = LockEffect {
                animation,
                ..Default::default()
            };
            for locked in [false, true] {
                assert_eq!(envelope(locked, 1.0, effect).0, 0.0);
            }
        }
        let m = Monitor {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        for inset in [0, 12, 18, 2000] {
            for edge in 0..4 {
                for part in 0..16 {
                    let (x, y, w, h) = segment(m, edge, part, inset, 16);
                    assert!(x >= m.x && y >= m.y && x + w as i32 <= 0 && y + h as i32 <= 1080);
                }
            }
        }
    }
}
