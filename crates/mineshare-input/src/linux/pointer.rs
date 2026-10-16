//! Query only X11 sessions. XWayland coordinates do not locate a Wayland desktop cursor.
use parking_lot::Mutex;
use std::sync::OnceLock;
use x11rb::{
    connection::Connection, protocol::xproto::ConnectionExt, rust_connection::RustConnection,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum Sample {
    Unsupported,
    Unavailable,
    Position(i32, i32),
}

pub(super) fn sample() -> Sample {
    if std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        return Sample::Unsupported;
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
