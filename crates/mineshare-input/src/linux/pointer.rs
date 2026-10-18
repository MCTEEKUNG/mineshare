//! Authoritative compositor coordinates only. Never infer an edge from raw mickeys.
use parking_lot::Mutex;
use std::sync::OnceLock;
use x11rb::{
    connection::Connection, protocol::xproto::ConnectionExt, rust_connection::RustConnection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sample {
    Unsupported,
    Unavailable,
    Position(i32, i32),
    Desktop {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

pub(super) fn sample() -> Sample {
    if std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        return wayland_sample();
    }
    static CONNECTION: OnceLock<Option<Mutex<(RustConnection, u32)>>> = OnceLock::new();
    let connection = CONNECTION.get_or_init(|| {
        match x11rb::connect(None) {
            Ok((conn, screen)) => {
                let root = conn.setup().roots[screen].root;
                tracing::info!("Linux edge crossing: native X11 pointer confirmation enabled");
                Some(Mutex::new((conn, root)))
            }
            Err(error) => {
                tracing::warn!(%error, "X11 pointer unavailable; automatic edge crossing paused (Ctrl+Alt+R remains available)");
                None
            }
        }
    });
    let Some(connection) = connection else {
        return Sample::Unavailable;
    };
    let guard = connection.lock();
    let (conn, root) = &*guard;
    let result = conn
        .query_pointer(*root)
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    match result {
        Some(reply) if reply.same_screen => {
            Sample::Position(i32::from(reply.root_x), i32::from(reply.root_y))
        }
        _ => Sample::Unavailable,
    }
}

fn wayland_sample() -> Sample {
    use std::time::{Duration, Instant};
    // Bound method waits on this candidate-only path. A missing/stalled extension
    // cannot authorize crossing; never cache a successful position as proof.
    struct Bus {
        connection: Option<zbus::blocking::Connection>,
        retry: Instant,
        warned: bool,
    }
    static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();
    let mut bus = BUS
        .get_or_init(|| {
            Mutex::new(Bus {
                connection: None,
                retry: Instant::now(),
                warned: false,
            })
        })
        .lock();
    if Instant::now() < bus.retry {
        return Sample::Unavailable;
    }
    let result = (|| -> zbus::Result<(i32, i32, i32, i32)> {
        if bus.connection.is_none() {
            bus.connection = Some(
                zbus::blocking::connection::Builder::session()?
                    .method_timeout(Duration::from_millis(12))
                    .build()?,
            );
        }
        bus.connection
            .as_ref()
            .unwrap()
            .call_method(
                Some("org.gnome.Shell"),
                "/org/gnome/Shell/Extensions/MineSharePointer",
                Some("org.gnome.Shell.Extensions.MineSharePointer"),
                "GetPointer",
                &(),
            )?
            .body()
            .deserialize()
    })();
    match result {
        Ok((x, y, width, height)) if width > 0 && height > 0 => {
            if bus.warned {
                tracing::info!("GNOME pointer confirmation restored");
            }
            bus.warned = false;
            Sample::Desktop {
                x,
                y,
                width,
                height,
            }
        }
        _ => {
            bus.connection = None;
            if !bus.warned {
                tracing::warn!(
                    "Wayland pointer confirmation unavailable: automatic crossing paused; enable MineShare GNOME pointer extension (Ctrl+Alt+R still works)"
                );
                bus.warned = true;
            }
            bus.retry = Instant::now() + Duration::from_secs(1);
            Sample::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    struct PointerFixture(Arc<AtomicBool>);
    #[zbus::interface(name = "org.gnome.Shell.Extensions.MineSharePointer")]
    impl PointerFixture {
        fn get_pointer(&self) -> (i32, i32, i32, i32) {
            if self.0.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(300));
            }
            (800, 350, 1280, 720)
        }
    }

    #[test]
    #[ignore = "isolated dbus-run-session only, never the live desktop bus"]
    fn private_bus_roundtrip_and_timeout_never_reuse_old_position() {
        assert_eq!(
            std::env::var("MINESHARE_TEST_PRIVATE_BUS").as_deref(),
            Ok("1")
        );
        assert!(std::env::var_os("DISPLAY").is_none());
        assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
        let stalled = Arc::new(AtomicBool::new(false));
        let _service = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name("org.gnome.Shell")
            .unwrap()
            .serve_at(
                "/org/gnome/Shell/Extensions/MineSharePointer",
                PointerFixture(stalled.clone()),
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            wayland_sample(),
            Sample::Desktop {
                x: 800,
                y: 350,
                width: 1280,
                height: 720
            }
        );
        stalled.store(true, Ordering::Relaxed);
        let now = Instant::now();
        assert_eq!(wayland_sample(), Sample::Unavailable);
        assert!(now.elapsed() < Duration::from_millis(200));
        assert_eq!(
            wayland_sample(),
            Sample::Unavailable,
            "backoff cannot reuse the old interior/edge position"
        );
    }
}
