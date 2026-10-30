//! Prepare device ownership before starting capture pumps.
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(super) struct Prepared<D, B> {
    pub devices: Vec<(PathBuf, D, Option<B>)>,
    pub keyboard_count: usize,
    pub broker_error: Option<anyhow::Error>,
}

pub(super) fn prepare<D, B>(
    devices: Vec<(PathBuf, D)>,
    broker_enabled: bool,
    is_pointer: impl Fn(&D) -> bool,
    mut prepare_broker: impl FnMut(&mut D) -> Result<B>,
) -> Result<Prepared<D, B>> {
    if !broker_enabled {
        return Ok(Prepared {
            devices: devices
                .into_iter()
                .map(|(path, device)| (path, device, None))
                .collect(),
            keyboard_count: 0,
            broker_error: None,
        });
    }
    let (pointers, keyboards): (Vec<_>, Vec<_>) = devices
        .into_iter()
        .partition(|(_, device)| is_pointer(device));
    let mut prepared: Vec<_> = pointers
        .into_iter()
        .map(|(path, device)| (path, device, None))
        .collect();
    // Keyboard ownership remains all-or-nothing. A failed group releases every
    // acquired grab/proxy, but never discards the independent pointer devices.
    let keyboards = (|| -> Result<Vec<_>> {
        anyhow::ensure!(
            !keyboards.is_empty(),
            "keyboard broker found no physical keyboard; Typeless startup must wait"
        );
        let mut group = Vec::new();
        for (path, mut device) in keyboards {
            let broker = prepare_broker(&mut device)
                .with_context(|| format!("prepare keyboard broker for {}", path.display()))?;
            group.push((path, device, Some(broker)));
        }
        Ok(group)
    })();
    match keyboards {
        Ok(keyboards) => {
            let keyboard_count = keyboards.len();
            prepared.extend(keyboards);
            Ok(Prepared {
                devices: prepared,
                keyboard_count,
                broker_error: None,
            })
        }
        Err(error) if !prepared.is_empty() => Ok(Prepared {
            devices: prepared,
            keyboard_count: 0,
            broker_error: Some(error),
        }),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    struct Device {
        pointer: bool,
        busy: bool,
    }
    struct Lease(Rc<Cell<usize>>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }
    fn device(name: &str, pointer: bool, busy: bool) -> (PathBuf, Device) {
        (name.into(), Device { pointer, busy })
    }

    #[test]
    fn busy_keyboard_does_not_disable_mice_or_keep_partial_keyboard_grabs() {
        let live = Rc::new(Cell::new(0));
        let devices = vec![
            device("keyboard-1", false, false),
            device("mouse-1", true, false),
            device("keyboard-2", false, true),
            device("mouse-2", true, false),
        ];
        let plan = prepare(
            devices,
            true,
            |d| d.pointer,
            |d| {
                anyhow::ensure!(!d.busy, "keyboard is owned by Typeless");
                live.set(live.get() + 1);
                Ok(Lease(live.clone()))
            },
        )
        .expect("a keyboard conflict must leave pointer capture available");
        assert_eq!(
            live.get(),
            0,
            "all partial keyboard ownership must be released"
        );
        assert_eq!(plan.keyboard_count, 0, "do not publish broker readiness");
        assert!(plan.broker_error.is_some());
        assert_eq!(
            plan.devices
                .iter()
                .map(|(p, _, _)| p.to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["mouse-1", "mouse-2"]
        );
        assert!(
            plan.devices
                .iter()
                .all(|(_, d, b)| d.pointer && b.is_none())
        );
    }

    #[test]
    fn pointer_only_computer_does_not_need_a_keyboard_for_mouse_capture() {
        let plan = prepare(
            vec![device("mouse", true, false)],
            true,
            |d| d.pointer,
            |_| Ok(()),
        )
        .expect("mouse capture must start without a physical keyboard");
        assert_eq!(plan.devices.len(), 1);
        assert_eq!(plan.keyboard_count, 0);
        assert!(plan.broker_error.is_some());
    }

    #[test]
    fn keyboard_failure_without_any_pointer_still_reports_capture_unavailable() {
        let result = prepare(
            vec![device("keyboard", false, true)],
            true,
            |d| d.pointer,
            |_| -> Result<()> { anyhow::bail!("keyboard busy") },
        );
        assert!(
            result.is_err(),
            "no usable device must not report capture started"
        );
    }

    #[test]
    fn ready_keyboard_group_and_disabled_broker_keep_their_existing_routes() {
        let plan = prepare(
            vec![
                device("keyboard", false, false),
                device("mouse", true, false),
            ],
            true,
            |d| d.pointer,
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(plan.keyboard_count, 1);
        assert!(plan.broker_error.is_none());
        assert_eq!(
            plan.devices
                .iter()
                .filter(|(_, _, broker)| broker.is_some())
                .count(),
            1
        );
        let plan = prepare(
            vec![
                device("keyboard", false, false),
                device("mouse", true, false),
            ],
            false,
            |d| d.pointer,
            |_| -> Result<()> { panic!("broker is disabled") },
        )
        .unwrap();
        assert_eq!(plan.devices.len(), 2);
        assert_eq!(plan.keyboard_count, 0);
        assert!(plan.devices.iter().all(|(_, _, broker)| broker.is_none()));
    }
}
