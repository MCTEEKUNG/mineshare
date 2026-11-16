//! Opt-in persistent physical-keyboard grab, before third-party shortcut helpers.
//! Only local edges reach the proxy. Remote edges bypass local hotkey consumers.
use super::keyboard_routes::{Destination, Routes};
use anyhow::{Context, Result};
use evdev::{Device, EventType, InputEvent, uinput::VirtualDevice};
use std::path::PathBuf;
use std::time::{Duration, Instant};

static LIVE: parking_lot::Mutex<usize> = parking_lot::const_mutex(0);

pub(super) fn enabled() -> bool {
    if let Ok(value) = std::env::var("MINESHARE_KEYBOARD_BROKER") {
        return value == "1";
    }
    config_root().is_some_and(|root| root.join("MineShare/keyboard-broker.enabled").is_file())
}

fn config_root() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

pub(super) fn virtual_source(path: &std::path::Path, name: &str) -> bool {
    name.starts_with("MineShare Virtual")
        || name.starts_with("Typeless ")
        || path.file_name().is_some_and(|node| {
            std::fs::canonicalize(PathBuf::from("/sys/class/input").join(node).join("device"))
                .is_ok_and(|path| path.starts_with("/sys/devices/virtual"))
        })
}

pub(super) fn readiness_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|root| PathBuf::from(root).join("mineshare-keyboard-broker.ready"))
}

pub(super) fn clear_readiness() {
    if let Some(path) = readiness_path() {
        // Never invalidate another live capture instance (including an isolated
        // integration test running alongside the real application).
        if std::fs::read_to_string(&path).is_ok_and(|value| {
            value.lines().next() == Some(std::process::id().to_string().as_str())
        }) {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(super) fn ready(count: usize) -> Result<()> {
    let live = LIVE.lock();
    anyhow::ensure!(
        *live == count && count > 0,
        "keyboard broker lost a device before readiness"
    );
    if let Some(path) = readiness_path() {
        let temporary = path.with_extension(format!("{}-tmp", std::process::id()));
        std::fs::write(&temporary, format!("{}\n{count}\n", std::process::id()))?;
        std::fs::rename(temporary, path)?;
    }
    Ok(())
}

pub(super) struct Broker {
    proxy: VirtualDevice,
    pub(super) routes: Routes,
}

impl Broker {
    pub(super) fn prepare(device: &mut Device) -> Result<Self> {
        let keys = device.supported_keys().context("keyboard has no keys")?;
        let proxy = VirtualDevice::builder()?
            .name("MineShare Virtual Local Keyboard")
            .with_keys(keys)?
            .build()
            .context("create local keyboard proxy")?;
        // The desktop may already have seen a physical key-down. Taking the
        // grab before that key goes up would hide the release from libinput,
        // which can then suppress that key even on other keyboards.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // EVIOCGKEY also flushes pre-grab key events from this reader.
            if device.get_key_state()?.iter().next().is_none() {
                device.grab().context(
                    "keyboard broker needs first ownership; log in with MineShare before Typeless",
                )?;
                // If a key arrived between the snapshot and the grab, give
                // the desktop its release before trying again.
                match device.get_key_state() {
                    Ok(keys) if keys.iter().next().is_none() => break,
                    state => {
                        device.ungrab()?;
                        state?;
                    }
                }
            }
            anyhow::ensure!(Instant::now() < deadline, "keyboard did not become idle");
            std::thread::sleep(Duration::from_millis(10));
        }
        *LIVE.lock() += 1;
        Ok(Self {
            proxy,
            routes: Routes::default(),
        })
    }

    pub(super) fn local_edge(&mut self, code: u16, value: i32) -> Result<()> {
        self.proxy
            .emit(&[InputEvent::new(EventType::KEY.0, code, value)])?;
        Ok(())
    }

    pub(super) fn dispatch(&mut self, code: u16, value: i32, remote: bool) -> Result<bool> {
        match self.routes.edge(code, value, remote) {
            Some(Destination::Local) => {
                self.local_edge(code, value)?;
                Ok(false)
            }
            Some(Destination::Remote) => Ok(true),
            _ => Ok(false),
        }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        let mut live = LIVE.lock();
        *live -= 1;
        clear_readiness();
        for code in self.routes.local_held() {
            let _ = self
                .proxy
                .emit(&[InputEvent::new(EventType::KEY.0, code, 0)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::{AttributeSet, KeyCode};
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "needs /dev/uinput on an isolated Linux seat"]
    fn desktop_receives_release_before_broker_takes_ownership() -> Result<()> {
        let keys = [KeyCode::KEY_F, KeyCode::KEY_G]
            .into_iter()
            .collect::<AttributeSet<_>>();
        let mut source = VirtualDevice::builder()?
            .name("MineShare Virtual Broker Handoff Test")
            .with_keys(&keys)?
            .build()?;
        let node = source
            .enumerate_dev_nodes_blocking()?
            .next()
            .context("source node")??;
        let mut desktop = Device::open(&node)?;
        desktop.set_nonblocking(true)?;
        source.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_F.0, 1)])?;
        assert!(desktop.fetch_events()?.any(|event| {
            event.event_type() == EventType::KEY
                && event.code() == KeyCode::KEY_F.0
                && event.value() == 1
        }));

        let mut physical = Device::open(&node)?;
        let _broker = std::thread::scope(|scope| -> Result<_> {
            let release = scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                source.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_F.0, 0)])
            });
            let broker = Broker::prepare(&mut physical);
            release.join().expect("release test key")?;
            broker
        })?;
        let released = match desktop.fetch_events() {
            Ok(mut events) => events.any(|event| {
                event.event_type() == EventType::KEY
                    && event.code() == KeyCode::KEY_F.0
                    && event.value() == 0
            }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => return Err(error.into()),
        };
        assert!(released, "the desktop saw F down and must also see F up");
        Ok(())
    }

    #[test]
    #[ignore = "needs /dev/uinput; isolated virtual-device integration test"]
    fn proxy_local_only_and_exclusive_ownership() -> Result<()> {
        fn open_node(path: &std::path::Path) -> Result<Device> {
            // Device creation precedes udev's group/ACL application.
            for _ in 0..50 {
                match Device::open(path) {
                    Ok(device) => return Ok(device),
                    Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Device::open(path).context("open test-owned node after udev permissions")
        }
        let keys = [KeyCode::KEY_A, KeyCode::KEY_RIGHTALT, KeyCode::KEY_LEFTMETA]
            .into_iter()
            .collect::<AttributeSet<_>>();
        let mut source = VirtualDevice::builder()?
            .name("MineShare Virtual Broker Test")
            .with_keys(&keys)?
            .build()?;
        let source_node = source
            .enumerate_dev_nodes_blocking()?
            .next()
            .context("source node")??;
        let mut physical = open_node(&source_node)?;
        let mut broker = Broker::prepare(&mut physical)?;
        let proxy_node = broker
            .proxy
            .enumerate_dev_nodes_blocking()?
            .next()
            .context("proxy node")??;
        let mut observer = open_node(&proxy_node)?;
        // Grab the observer before emitting: test events cannot reach the desktop.
        observer.grab()?;
        observer.set_nonblocking(true)?;
        let mut competitor = open_node(&source_node)?;
        assert!(
            competitor.grab().is_err(),
            "broker owns physical source first"
        );
        for code in [100, 125] {
            assert!(!broker.dispatch(code, 1, false)?);
            assert!(!broker.dispatch(code, 0, false)?);
            assert!(broker.dispatch(code, 1, true)?);
            assert!(broker.dispatch(code, 0, true)?);
        }
        let mut received = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            match observer.fetch_events() {
                Ok(events) => received.extend(
                    events
                        .filter(|event| event.event_type() == EventType::KEY)
                        .map(|event| (event.code(), event.value())),
                ),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if received.len() == 4 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(received, vec![(100, 1), (100, 0), (125, 1), (125, 0)]);
        drop(broker);
        physical.ungrab()?;
        competitor.grab()?;
        competitor.ungrab()?;
        Ok(())
    }
}
