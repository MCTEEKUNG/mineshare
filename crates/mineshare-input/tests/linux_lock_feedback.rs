#![cfg(target_os = "linux")]
use std::time::{Duration, Instant};
use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};

#[test]
#[ignore = "requires a private X11 display (Xvfb) or an interactive desktop"]
fn linux_preview_draws_click_through_border_and_cleans_up() {
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let name = conn
        .intern_atom(false, b"_MINESHARE_LOCK_FEEDBACK")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let windows = || {
        conn.query_tree(root)
            .unwrap()
            .reply()
            .unwrap()
            .children
            .into_iter()
            .filter(|w| {
                conn.get_property(
                    false,
                    *w,
                    name,
                    x11rb::protocol::xproto::AtomEnum::CARDINAL,
                    0,
                    1,
                )
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|p| !p.value.is_empty())
            })
            .collect::<Vec<_>>()
    };
    let focus = conn.get_input_focus().unwrap().reply().unwrap().focus;
    let locked = mineshare_input::is_input_locked();
    mineshare_input::preview_lock_effect(
        true,
        mineshare_input::LockEffect {
            rainbow: false,
            color: [255, 30, 100],
            animation: mineshare_input::LockAnimation::Pulse,
            duration_ms: 900,
            thickness: 8,
        },
    );
    let deadline = Instant::now() + Duration::from_millis(600);
    let mut visible = vec![];
    while Instant::now() < deadline {
        visible = windows();
        if !visible.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !visible.is_empty(),
        "Linux preview is a no-op: no native feedback windows"
    );
    for window in visible {
        let shape = x11rb::protocol::shape::ConnectionExt::shape_get_rectangles(
            &conn,
            window,
            x11rb::protocol::shape::SK::INPUT,
        )
        .unwrap()
        .reply()
        .unwrap();
        assert!(
            shape.rectangles.is_empty(),
            "feedback must never intercept clicks"
        );
        assert!(
            conn.get_window_attributes(window)
                .unwrap()
                .reply()
                .unwrap()
                .override_redirect
        );
    }
    // A single compositor surface per monitor, with a shaped border/hole.
    std::thread::sleep(Duration::from_millis(60));
    let visible = windows();
    let monitor_count =
        x11rb::protocol::randr::ConnectionExt::randr_get_monitors(&conn, root, true)
            .unwrap()
            .reply()
            .unwrap()
            .monitors
            .len()
            .max(1);
    assert!(
        visible.len() <= monitor_count,
        "feedback creates too many compositor surfaces: {} for {} monitors",
        visible.len(),
        monitor_count
    );
    let window = visible[0];
    let bounds = x11rb::protocol::shape::ConnectionExt::shape_get_rectangles(
        &conn,
        window,
        x11rb::protocol::shape::SK::BOUNDING,
    )
    .unwrap()
    .reply()
    .unwrap();
    let top = bounds.rectangles[0];
    let image = conn
        .get_image(
            x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
            window,
            top.x + (top.width / 2) as i16,
            top.y + (top.height / 2) as i16,
            1,
            1,
            u32::MAX,
        )
        .unwrap()
        .reply()
        .unwrap();
    let bytes: [u8; 4] = image.data[..4].try_into().unwrap();
    let pixel = if conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    };
    assert_eq!(
        pixel & 0x00ff_ffff,
        0x00ff_1e64,
        "saved custom RGB color was not painted"
    );
    assert_eq!(
        focus,
        conn.get_input_focus().unwrap().reply().unwrap().focus,
        "feedback stole focus"
    );
    assert_eq!(
        locked,
        mineshare_input::is_input_locked(),
        "preview changed the real lock"
    );
    std::thread::sleep(Duration::from_millis(1100));
    assert!(windows().is_empty(), "transient feedback leaked windows");
}

#[test]
#[ignore = "requires a private X11 display (Xvfb) or an interactive desktop"]
fn linux_rainbow_feedback_keeps_xserver_responsive() {
    let (conn, _) = x11rb::connect(None).unwrap();
    mineshare_input::preview_lock_effect(true, mineshare_input::LockEffect::default());
    let deadline = Instant::now() + Duration::from_millis(1600);
    let mut samples = Vec::new();
    while Instant::now() < deadline {
        let start = Instant::now();
        conn.get_input_focus().unwrap().reply().unwrap();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(Duration::from_millis(10));
    }
    samples.sort_by(f64::total_cmp);
    let p95 = samples[samples.len() * 95 / 100];
    println!(
        "rainbow X11 query latency: samples={} p50={:.3}ms p95={:.3}ms max={:.3}ms",
        samples.len(),
        samples[samples.len() / 2],
        p95,
        samples.last().unwrap()
    );
    assert!(
        p95 < 100.0,
        "rainbow feedback stalls the desktop X server: p95={p95:.3}ms"
    );
}
