//! Per-physical-device ownership. A local press must not migrate on repeat/up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Destination {
    Local,
    Remote,
    Consumed,
}

pub(super) struct Routes([Option<Destination>; 1024]);

impl Default for Routes {
    fn default() -> Self {
        Self([None; 1024])
    }
}

impl Routes {
    pub(super) fn edge(&mut self, code: u16, value: i32, remote: bool) -> Option<Destination> {
        let slot = self.0.get_mut(usize::from(code))?;
        match value {
            1 => {
                let destination = slot.get_or_insert(if remote {
                    Destination::Remote
                } else {
                    Destination::Local
                });
                Some(*destination)
            }
            2 => *slot,
            0 => slot.take(),
            _ => None,
        }
    }

    pub(super) fn consume(&mut self, code: u16) {
        if let Some(slot) = self.0.get_mut(usize::from(code)) {
            *slot = Some(Destination::Consumed);
        }
    }

    pub(super) fn is_local(&self, code: u16) -> bool {
        self.0.get(usize::from(code)) == Some(&Some(Destination::Local))
    }

    pub(super) fn local_held(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.iter().enumerate().filter_map(|(code, destination)| {
            (*destination == Some(Destination::Local)).then_some(code as u16)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_alt_and_super_only_reach_selected_destination() {
        for code in [56, 100, 125, 126] {
            let mut routes = Routes::default();
            assert_eq!(routes.edge(code, 1, true), Some(Destination::Remote));
            assert_eq!(routes.edge(code, 0, true), Some(Destination::Remote));
            assert_eq!(routes.edge(code, 1, false), Some(Destination::Local));
            assert_eq!(routes.edge(code, 0, false), Some(Destination::Local));
        }
    }

    #[test]
    fn local_repeat_and_release_stay_local_after_crossing() {
        let mut routes = Routes::default();
        routes.edge(100, 1, false);
        assert!(routes.is_local(100));
        assert_eq!(routes.edge(100, 2, true), Some(Destination::Local));
        assert_eq!(routes.edge(100, 0, true), Some(Destination::Local));
        assert!(!routes.is_local(100));
    }

    #[test]
    fn remote_release_stays_remote_and_never_leaks_to_local_typeless() {
        let mut routes = Routes::default();
        routes.edge(100, 1, true);
        assert_eq!(routes.edge(100, 0, false), Some(Destination::Remote));
        assert_eq!(routes.edge(100, 0, false), None);
        assert_eq!(routes.edge(100, 2, false), None);
    }

    #[test]
    fn consumed_hotkey_has_no_local_repeat_or_release() {
        let mut routes = Routes::default();
        routes.consume(38);
        assert_eq!(routes.edge(38, 2, false), Some(Destination::Consumed));
        assert_eq!(routes.edge(38, 0, false), Some(Destination::Consumed));
        assert_eq!(routes.edge(38, 1, false), Some(Destination::Local));
    }

    #[test]
    fn cleanup_releases_only_local_keys_and_rejects_malformed_events() {
        let mut routes = Routes::default();
        routes.edge(42, 1, false);
        routes.edge(100, 1, true);
        assert_eq!(routes.local_held().collect::<Vec<_>>(), vec![42]);
        assert_eq!(routes.edge(65535, 1, false), None);
        assert_eq!(routes.edge(42, 3, false), None);
    }
}
