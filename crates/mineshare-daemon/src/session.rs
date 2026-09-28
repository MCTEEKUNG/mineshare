//! One owner for the process-wide input/audio/clipboard routing state.
use anyhow::{Result, bail};
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::watch;

pub(crate) struct Sessions {
    active: Mutex<Option<Active>>,
    // A replacement must wait until all old workers and input cleanup finish.
    pub(crate) lifetime: tokio::sync::Mutex<()>,
}

struct Active {
    peer: Vec<u8>,
    preferred: bool,
    token: Arc<()>,
    cancel: watch::Sender<bool>,
}

impl Sessions {
    pub(crate) const fn new() -> Self {
        Self {
            active: Mutex::new(None),
            lifetime: tokio::sync::Mutex::const_new(()),
        }
    }

    pub(crate) fn claim(&self, peer: &[u8], preferred: bool) -> Result<Claim<'_>> {
        let mut active = self.active.lock();
        if let Some(old) = active.as_ref() {
            if old.peer != peer || old.preferred || !preferred {
                bail!("a peer session already owns the bridge");
            }
            // Both endpoints rank the same TCP connection first: the one
            // initiated by the lower authenticated static key. This also
            // allows the other direction when only one side discovers us.
            let _ = old.cancel.send(true);
        }
        let (cancel, cancelled) = watch::channel(false);
        let token = Arc::new(());
        *active = Some(Active {
            peer: peer.to_vec(),
            preferred,
            token: token.clone(),
            cancel,
        });
        Ok(Claim {
            sessions: self,
            token,
            cancelled,
        })
    }
}

pub(crate) struct Claim<'a> {
    sessions: &'a Sessions,
    token: Arc<()>,
    cancelled: watch::Receiver<bool>,
}

impl Claim<'_> {
    pub(crate) async fn cancelled(&mut self) {
        if !*self.cancelled.borrow() {
            let _ = self.cancelled.changed().await;
        }
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut active = self.sessions.active.lock();
        if active
            .as_ref()
            .is_some_and(|a| Arc::ptr_eq(&a.token, &self.token))
        {
            *active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn simultaneous_dial_converges_and_cleanup_precedes_replacement() {
        let a = Sessions::new();
        let b = Sessions::new();
        // Opposite connections reach the two endpoints first.
        let preferred_a = a.claim(b"b", true).unwrap();
        let mut fallback_b = b.claim(b"a", false).unwrap();
        let old_workers = b.lifetime.lock().await;
        assert!(a.claim(b"b", false).is_err());
        let preferred_b = b.claim(b"a", true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), fallback_b.cancelled())
            .await
            .unwrap();
        assert!(b.lifetime.try_lock().is_err());
        drop(old_workers);
        drop(fallback_b);
        let new_workers = b.lifetime.try_lock().unwrap();
        // The retired owner's Drop must not erase the replacement.
        assert!(b.claim(b"a", true).is_err());
        assert!(b.claim(b"another peer", true).is_err());
        drop(new_workers);
        drop(preferred_a);
        drop(preferred_b);
        assert!(a.claim(b"b", false).is_ok());
        assert!(b.claim(b"a", false).is_ok());
    }

    #[tokio::test]
    async fn failed_setup_or_cancelled_waiter_releases_its_claim() {
        let sessions = Sessions::new();
        let failed_setup = sessions.claim(b"peer", true).unwrap();
        let lifetime = sessions.lifetime.lock().await;
        drop(lifetime);
        drop(failed_setup);
        let fallback = sessions.claim(b"peer", false).unwrap();
        let preferred = sessions.claim(b"peer", true).unwrap();
        drop(preferred); // e.g. handshake/port exchange failed
        drop(fallback);
        assert!(sessions.claim(b"peer", true).is_ok());
    }
}
