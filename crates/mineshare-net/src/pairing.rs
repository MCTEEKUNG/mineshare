//! Noise XX pairing handshake (POC for M0 — no PIN UI yet, no PSK).

use anyhow::{Context, Result};
use snow::{Builder, HandshakeState, TransportState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Generate a fresh static keypair (caller persists this on disk).
pub fn generate_static_key() -> Result<snow::Keypair> {
    Builder::new(NOISE_PARAMS.parse().context("invalid noise params")?)
        .generate_keypair()
        .context("noise keygen failed")
}

pub struct Initiator {
    state: Option<HandshakeState>,
    transport: Option<TransportState>,
}

pub struct Responder {
    state: Option<HandshakeState>,
    transport: Option<TransportState>,
}

pub enum NoiseSession {
    Init(Initiator),
    Resp(Responder),
}

impl Initiator {
    pub fn new(static_key: &[u8]) -> Result<Self> {
        let state = Builder::new(NOISE_PARAMS.parse()?)
            .local_private_key(static_key)?
            .build_initiator()
            .context("build_initiator failed")?;
        Ok(Self {
            state: Some(state),
            transport: None,
        })
    }

    /// Run the XX handshake on a tokio TcpStream.
    pub async fn handshake(&mut self, stream: &mut TcpStream) -> Result<[u8; 32]> {
        let mut state = self.state.take().context("handshake already done")?;
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

        let transport = state.into_transport_mode()?;
        let remote_static = transport
            .get_remote_static()
            .context("missing remote static after XX")?;
        let mut peer_pub = [0u8; 32];
        peer_pub.copy_from_slice(remote_static);
        self.transport = Some(transport);
        Ok(peer_pub)
    }
}

impl Responder {
    pub fn new(static_key: &[u8]) -> Result<Self> {
        let state = Builder::new(NOISE_PARAMS.parse()?)
            .local_private_key(static_key)?
            .build_responder()
            .context("build_responder failed")?;
        Ok(Self {
            state: Some(state),
            transport: None,
        })
    }

    pub async fn handshake(&mut self, stream: &mut TcpStream) -> Result<[u8; 32]> {
        let mut state = self.state.take().context("handshake already done")?;
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

        let transport = state.into_transport_mode()?;
        let remote_static = transport
            .get_remote_static()
            .context("missing remote static after XX")?;
        let mut peer_pub = [0u8; 32];
        peer_pub.copy_from_slice(remote_static);
        self.transport = Some(transport);
        Ok(peer_pub)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn xx_handshake_roundtrip() {
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
        let server_pub = i.handshake(&mut s).await.unwrap();
        let client_pub = server.await.unwrap();

        assert_eq!(server_pub.as_slice(), resp_pub.as_slice());
        assert_eq!(client_pub.as_slice(), init_pub.as_slice());
    }
}
