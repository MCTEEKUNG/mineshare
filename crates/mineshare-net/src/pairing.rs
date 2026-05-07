//! Noise XX pairing handshake (POC for M0/M1 — no PIN UI yet, no PSK).
//!
//! After the handshake completes we move the Noise session into **stateless
//! transport mode**. Stateless mode lets us encrypt/decrypt with explicit
//! per-message nonces, which is mandatory for UDP where packets may be lost
//! or reordered.

use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use snow::{Builder, HandshakeState, StatelessTransportState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Generate a fresh static keypair (caller persists this on disk).
pub fn generate_static_key() -> Result<snow::Keypair> {
    Builder::new(NOISE_PARAMS.parse().context("invalid noise params")?)
        .generate_keypair()
        .context("noise keygen failed")
}

/// Result of a successful Noise XX handshake.
pub struct NoiseSession {
    /// 32-byte X25519 public key of the peer.
    pub remote_static: [u8; 32],
    /// Stateless transport state — wrap in `EncryptedSession` for actual use.
    pub transport: StatelessTransportState,
}

pub struct Initiator(Option<HandshakeState>);
pub struct Responder(Option<HandshakeState>);

impl Initiator {
    pub fn new(static_key: &[u8]) -> Result<Self> {
        let st = Builder::new(NOISE_PARAMS.parse()?)
            .local_private_key(static_key)?
            .build_initiator()
            .context("build_initiator")?;
        Ok(Self(Some(st)))
    }

    /// Run XX handshake on a tokio TcpStream and return the transport session.
    pub async fn handshake(&mut self, stream: &mut TcpStream) -> Result<NoiseSession> {
        let mut state = self.0.take().context("handshake already done")?;
        let mut buf = vec![0u8; 1024];

        // -> e
        let n = state.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;

        // <- e, ee, s, es
        let frame = read_frame(stream).await?;
        let _ = state.read_message(&frame, &mut buf)?;

        // -> s, se
        let n = state.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;

        finalize(state)
    }
}

impl Responder {
    pub fn new(static_key: &[u8]) -> Result<Self> {
        let st = Builder::new(NOISE_PARAMS.parse()?)
            .local_private_key(static_key)?
            .build_responder()
            .context("build_responder")?;
        Ok(Self(Some(st)))
    }

    pub async fn handshake(&mut self, stream: &mut TcpStream) -> Result<NoiseSession> {
        let mut state = self.0.take().context("handshake already done")?;
        let mut buf = vec![0u8; 1024];

        // <- e
        let frame = read_frame(stream).await?;
        let _ = state.read_message(&frame, &mut buf)?;

        // -> e, ee, s, es
        let n = state.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;

        // <- s, se
        let frame = read_frame(stream).await?;
        let _ = state.read_message(&frame, &mut buf)?;

        finalize(state)
    }
}

fn finalize(state: HandshakeState) -> Result<NoiseSession> {
    let mut remote_static = [0u8; 32];
    if let Some(rs) = state.get_remote_static() {
        remote_static.copy_from_slice(rs);
    }
    let transport = state
        .into_stateless_transport_mode()
        .context("into_stateless_transport_mode")?;
    Ok(NoiseSession {
        remote_static,
        transport,
    })
}

async fn write_frame(stream: &mut TcpStream, msg: &[u8]) -> Result<()> {
    let len = u16::try_from(msg.len()).context("noise frame too large")?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(msg).await?;
    Ok(())
}

async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).await?;
    let len = u16::from_be_bytes(hdr) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// AEAD wrapper around `StatelessTransportState` with explicit nonces and
/// a sliding replay window. Both peers share one cipher state — Noise
/// internally derives a key per direction.
pub struct EncryptedSession {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<StatelessTransportState>,
    next_send_nonce: std::sync::atomic::AtomicU64,
    replay: Mutex<ReplayWindow>,
}

impl EncryptedSession {
    pub fn from(session: NoiseSession) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(session.transport),
                next_send_nonce: std::sync::atomic::AtomicU64::new(0),
                replay: Mutex::new(ReplayWindow::default()),
            }),
        }
    }

    pub fn clone_handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Encrypt `plaintext`. Returns wire bytes: `[u64 nonce][ciphertext]`.
    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self
            .inner
            .next_send_nonce
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut buf = vec![0u8; plaintext.len() + 16];
        let n = self
            .inner
            .state
            .lock()
            .write_message(nonce, plaintext, &mut buf)?;
        buf.truncate(n);
        let mut out = Vec::with_capacity(8 + buf.len());
        out.extend_from_slice(&nonce.to_be_bytes());
        out.extend_from_slice(&buf);
        Ok(out)
    }

    /// Decrypt a wire frame into plaintext, rejecting replays.
    pub fn open(&self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.len() < 8 {
            anyhow::bail!("frame too short");
        }
        let mut n = [0u8; 8];
        n.copy_from_slice(&frame[..8]);
        let nonce = u64::from_be_bytes(n);

        if !self.inner.replay.lock().check_and_record(nonce) {
            anyhow::bail!("replay or out-of-window nonce {nonce}");
        }

        let ct = &frame[8..];
        let mut buf = vec![0u8; ct.len()];
        let n = self.inner.state.lock().read_message(nonce, ct, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}

const WINDOW_SIZE: u64 = 128;

/// Sliding replay window keyed on monotonically rising nonces.
#[derive(Default)]
struct ReplayWindow {
    /// Highest nonce we've seen.
    highest: u64,
    /// Bitmap covering [highest - WINDOW_SIZE + 1 ..= highest]. Bit 0 = highest.
    bitmap: u128,
}

impl ReplayWindow {
    /// Returns true if `nonce` is fresh (and records it). False on replay
    /// or too-old.
    fn check_and_record(&mut self, nonce: u64) -> bool {
        if nonce > self.highest {
            let shift = nonce - self.highest;
            if shift >= 128 {
                self.bitmap = 1;
            } else {
                self.bitmap = self.bitmap.checked_shl(shift as u32).unwrap_or(0) | 1;
            }
            self.highest = nonce;
            return true;
        }
        let lag = self.highest - nonce;
        if lag >= WINDOW_SIZE {
            return false;
        }
        let bit = 1u128 << lag;
        if self.bitmap & bit != 0 {
            return false; // replay
        }
        self.bitmap |= bit;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn xx_handshake_then_udp_aead_roundtrip() {
        let init_kp = generate_static_key().unwrap();
        let resp_kp = generate_static_key().unwrap();
        let init_pub = init_kp.public.clone();
        let resp_pub = resp_kp.public.clone();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let resp_priv = resp_kp.private.clone();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut r = Responder::new(&resp_priv).unwrap();
            r.handshake(&mut s).await.unwrap()
        });

        let mut s = TcpStream::connect(addr).await.unwrap();
        let mut i = Initiator::new(&init_kp.private).unwrap();
        let init_session = i.handshake(&mut s).await.unwrap();
        let resp_session = server.await.unwrap();

        assert_eq!(init_session.remote_static, resp_pub.as_slice());
        assert_eq!(resp_session.remote_static, init_pub.as_slice());

        let init_aead = EncryptedSession::from(init_session);
        let resp_aead = EncryptedSession::from(resp_session);

        // initiator -> responder
        let pt = b"hello mineshare";
        let frame = init_aead.seal(pt).unwrap();
        let out = resp_aead.open(&frame).unwrap();
        assert_eq!(out, pt);

        // replay should fail
        assert!(resp_aead.open(&frame).is_err());

        // responder -> initiator (reversed direction works because
        // StatelessTransportState exposes both write and read)
        let frame2 = resp_aead.seal(b"reply").unwrap();
        let out2 = init_aead.open(&frame2).unwrap();
        assert_eq!(out2, b"reply");
    }

    #[test]
    fn replay_window_basic() {
        let mut w = ReplayWindow::default();
        assert!(w.check_and_record(1));
        assert!(w.check_and_record(2));
        assert!(!w.check_and_record(1)); // replay
        assert!(w.check_and_record(50));
        assert!(w.check_and_record(40)); // older but inside window
        assert!(!w.check_and_record(40)); // replay
    }
}
