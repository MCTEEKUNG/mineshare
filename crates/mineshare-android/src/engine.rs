use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use mineshare_core::DeviceId;
use mineshare_input::{ForwardedInputState, InputEvent};
use mineshare_net::{EncryptedSession, Initiator, Responder, TransportDomain};
use mineshare_protocol::*;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

const FRAME_SAMPLES: usize = 1920;
const EVENT_LIMIT: usize = 2048;
const SETUP_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Serialize, Deserialize)]
struct Identity {
    device_id: DeviceId,
    private_key: Vec<u8>,
    public_key: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Trusted {
    name: String,
    key: Vec<u8>,
    #[serde(default)]
    send: bool,
    #[serde(default = "enabled")]
    receive: bool,
}

fn enabled() -> bool {
    true
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Saved {
    trusted: HashMap<String, Trusted>,
}

#[derive(Clone)]
struct Advert {
    name: String,
    os: String,
    address: SocketAddr,
    compatible: bool,
}

struct Peer {
    name: String,
    os: String,
    token: u64,
    preferred: bool,
    cancel: watch::Sender<bool>,
    control: mpsc::Sender<ControlMsg>,
}

struct Pending {
    peer_id: String,
    kind: &'static str,
    reply: oneshot::Sender<Value>,
}

struct PendingGuard<'a> {
    state: &'a State,
    attempt: u64,
}

struct PairingUiGuard<'a>(&'a State);
impl Drop for PairingUiGuard<'_> {
    fn drop(&mut self) {
        self.0.event(json!({"type":"pairing","kind":"closed"}));
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.state.pending.lock().remove(&self.attempt);
    }
}

struct State {
    identity: Identity,
    name: String,
    directory: PathBuf,
    saved: Mutex<Saved>,
    adverts: Mutex<HashMap<String, Advert>>,
    peers: Mutex<HashMap<String, Peer>>,
    dialing: Mutex<HashMap<String, bool>>,
    pending: Mutex<HashMap<u64, Pending>>,
    owner: Mutex<Option<String>>,
    events: Mutex<VecDeque<Value>>,
    audio: Mutex<HashMap<String, VecDeque<Vec<f32>>>>,
    pcm: mpsc::Sender<Vec<f32>>,
    encoded: broadcast::Sender<AudioFrame>,
    serial: AtomicU64,
    width: AtomicU32,
    height: AtomicU32,
    paused: AtomicBool,
    stop: watch::Sender<bool>,
    setup_cancel: watch::Sender<u64>,
    pairing_gate: tokio::sync::Mutex<()>,
    prompt: AtomicU64,
}

/// A process-local engine, also usable in native transport integration tests.
pub struct Engine {
    state: Arc<State>,
    runtime: Option<Runtime>,
    port: u16,
}

impl Engine {
    pub fn start(directory: &Path, name: &str, width: u32, height: u32) -> Result<Self> {
        ensure!(
            !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control),
            "invalid device name"
        );
        ensure!(width > 0 && height > 0, "invalid display size");
        std::fs::create_dir_all(directory)?;
        let identity_path = directory.join("identity.json");
        let identity: Identity = if identity_path.exists() {
            serde_json::from_slice(&std::fs::read(identity_path)?)?
        } else {
            let key = mineshare_net::pairing::generate_static_key()?;
            let identity = Identity {
                device_id: DeviceId::new(),
                private_key: key.private,
                public_key: key.public,
            };
            persist(&identity_path, &identity)?;
            identity
        };
        ensure!(
            identity.private_key.len() == 32 && identity.public_key.len() == 32,
            "invalid saved identity"
        );
        let saved_path = directory.join("peers.json");
        let saved = if saved_path.exists() {
            serde_json::from_slice(&std::fs::read(saved_path)?)?
        } else {
            Saved::default()
        };
        let listener: std::net::TcpListener = match dual_stack_listener() {
            Ok(listener) => listener,
            Err(_) => std::net::TcpListener::bind("0.0.0.0:0")?,
        };
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (stop, _) = watch::channel(false);
        let (setup_cancel, _) = watch::channel(0);
        let (pcm, pcm_rx) = mpsc::channel(4);
        let (encoded, _) = broadcast::channel(4);
        let state = Arc::new(State {
            identity,
            name: name.to_owned(),
            directory: directory.to_path_buf(),
            saved: Mutex::new(saved),
            adverts: Mutex::new(HashMap::new()),
            peers: Mutex::new(HashMap::new()),
            dialing: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            owner: Mutex::new(None),
            events: Mutex::new(VecDeque::new()),
            audio: Mutex::new(HashMap::new()),
            pcm,
            encoded,
            serial: AtomicU64::new(1),
            width: AtomicU32::new(width),
            height: AtomicU32::new(height),
            paused: AtomicBool::new(false),
            stop,
            setup_cancel,
            pairing_gate: tokio::sync::Mutex::new(()),
            prompt: AtomicU64::new(0),
        });
        {
            let _guard = runtime.enter();
            let listener = TcpListener::from_std(listener)?;
            runtime.spawn(accept_loop(state.clone(), listener));
            runtime.spawn(reconnect_loop(state.clone()));
            runtime.spawn(encode_loop(state.clone(), pcm_rx));
        }
        state.event(json!({"type":"identity","device_id":state.identity.device_id.to_string(),"port":port,"version":version()}));
        state.snapshot();
        Ok(Self {
            state,
            runtime: Some(runtime),
            port,
        })
    }

    pub fn device_id(&self) -> String {
        self.state.identity.device_id.to_string()
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn poll(&self) -> Option<String> {
        self.state.events.lock().pop_front().map(|v| v.to_string())
    }
    pub fn send_pcm(&self, pcm: Vec<f32>) {
        if !pcm.is_empty() && pcm.len() <= FRAME_SAMPLES * 8 && pcm.iter().all(|v| v.is_finite()) {
            let _ = self.state.pcm.try_send(pcm);
        }
    }
    pub fn poll_audio(&self) -> Option<Vec<f32>> {
        let mut audio = self.state.audio.lock();
        mix_audio(&mut audio)
    }

    pub fn command(&self, message: &str) -> Result<()> {
        let v: Value = serde_json::from_str(message)?;
        let state = &self.state;
        match v["type"]
            .as_str()
            .or(v["command"].as_str())
            .context("missing command type")?
        {
            "discover_peer" => {
                let id = text_field(&v, "device_id")?;
                let _: uuid::Uuid = id.parse()?;
                if id == self.device_id() {
                    return Ok(());
                }
                let host = text_field(&v, "host")?;
                let ip: IpAddr = host.split('%').next().context("invalid host")?.parse()?;
                ensure!(
                    !ip.is_multicast() && !ip.is_unspecified(),
                    "invalid peer address"
                );
                let port = v["port"]
                    .as_u64()
                    .filter(|p| *p > 0 && *p <= 65535)
                    .context("invalid port")? as u16;
                state.adverts.lock().insert(
                    id.to_owned(),
                    Advert {
                        name: v["display_name"]
                            .as_str()
                            .unwrap_or("MineShare")
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(256)
                            .collect(),
                        os: v["os"].as_str().unwrap_or("unknown").to_owned(),
                        address: match ip {
                            IpAddr::V6(ip) => SocketAddr::V6(std::net::SocketAddrV6::new(
                                ip,
                                port,
                                0,
                                v["scope_id"].as_u64().unwrap_or(0) as u32,
                            )),
                            IpAddr::V4(_) => SocketAddr::new(ip, port),
                        },
                        compatible: v["compatible"].as_bool().unwrap_or(true),
                    },
                );
                state.snapshot();
            }
            "peer_lost" => {
                state.adverts.lock().remove(text_field(&v, "device_id")?);
                state.snapshot();
            }
            "pair" => {
                let id = text_field(&v, "device_id")?;
                ensure!(
                    !state.paused.load(Ordering::Acquire),
                    "resume connections first"
                );
                ensure!(state.adverts.lock().contains_key(id), "device is offline");
                ensure!(
                    state.adverts.lock().get(id).is_some_and(|p| p.compatible),
                    "update MineShare on the other device first"
                );
                ensure!(
                    !state.peers.lock().contains_key(id),
                    "device is already connected"
                );
                self.runtime.as_ref().context("engine stopped")?.spawn(dial(
                    state.clone(),
                    id.to_owned(),
                    true,
                ));
            }
            "consent" | "pin" => {
                let attempt = v["attempt_id"].as_u64().context("missing attempt ID")?;
                let mut pending = state.pending.lock();
                let p = pending.get(&attempt).context("pairing attempt expired")?;
                let kind = v["type"]
                    .as_str()
                    .or(v["command"].as_str())
                    .unwrap_or_default();
                ensure!(
                    (p.kind == "consent" && kind == "consent")
                        || (p.kind == "enter_pin" && kind == "pin"),
                    "unexpected pairing response"
                );
                if kind == "pin" {
                    let pin = text_field(&v, "pin")?;
                    ensure!(
                        pin.len() == 6 && pin.bytes().all(|b| b.is_ascii_digit()),
                        "PIN must contain six digits"
                    );
                }
                let p = pending.remove(&attempt).unwrap();
                p.reply
                    .send(v)
                    .map_err(|_| anyhow::anyhow!("pairing attempt expired"))?;
            }
            "cancel_pairing" => {
                if let Some(attempt) = v["attempt_id"].as_u64() {
                    ensure!(
                        attempt != 0 && state.prompt.load(Ordering::Acquire) == attempt,
                        "pairing attempt expired"
                    );
                }
                state.setup_cancel.send_modify(|value| *value += 1);
                state.event(json!({"type":"pairing","kind":"closed"}));
                state.pending.lock().clear();
                state.snapshot();
            }
            "pause" => {
                let paused = v["paused"].as_bool().context("missing paused")?;
                state.paused.store(paused, Ordering::Release);
                if paused {
                    state.setup_cancel.send_modify(|value| *value += 1);
                    state.event(json!({"type":"pairing","kind":"closed"}));
                    for peer in state.peers.lock().values() {
                        peer.cancel.send_replace(true);
                    }
                    state.pending.lock().clear();
                }
                state.snapshot();
            }
            "audio_route" => {
                let id = text_field(&v, "device_id")?;
                let mut saved = state.saved.lock();
                let mut next = saved.clone();
                let peer = next.trusted.get_mut(id).context("device is not paired")?;
                peer.send = v["send"].as_bool().context("missing send")?;
                peer.receive = v["receive"].as_bool().context("missing receive")?;
                let receive_system = peer.receive;
                persist(&state.directory.join("peers.json"), &next)?;
                *saved = next;
                drop(saved);
                if !receive_system {
                    state.audio.lock().remove(id);
                }
                if let Some(peer) = state.peers.lock().get(id) {
                    let _ = peer
                        .control
                        .try_send(ControlMsg::AudioRoute { receive_system });
                }
                state.snapshot();
            }
            "display" => {
                let width = v["width"]
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= 32768)
                    .context("invalid width")? as u32;
                let height = v["height"]
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= 32768)
                    .context("invalid height")? as u32;
                state.width.store(width, Ordering::Release);
                state.height.store(height, Ordering::Release);
                for peer in state.peers.lock().values() {
                    let _ = peer
                        .control
                        .try_send(ControlMsg::DisplayGeometry { width, height });
                }
            }
            "release_input" => {
                let owner = state.owner.lock().take();
                if let Some(id) = owner {
                    if let Some(peer) = state.peers.lock().get(&id) {
                        let _ = peer.control.try_send(ControlMsg::ForceRelease);
                    }
                    state.event(json!({"type":"input","kind":"release","device_id":id}));
                    state.snapshot();
                }
            }
            "forget" => {
                let id = text_field(&v, "device_id")?;
                let mut saved = state.saved.lock();
                let mut next = saved.clone();
                next.trusted.remove(id);
                persist(&state.directory.join("peers.json"), &next)?;
                *saved = next;
                drop(saved);
                if let Some(peer) = state.peers.lock().get(id) {
                    peer.cancel.send_replace(true);
                }
                state.snapshot();
            }
            other => bail!("unknown command {other}"),
        }
        Ok(())
    }
}

// Reuse the first queued frame as the mix destination. Empty polls allocate
// nothing, and a single source needs no additional PCM allocation or copy.
fn mix_audio(audio: &mut HashMap<String, VecDeque<Vec<f32>>>) -> Option<Vec<f32>> {
    let mut frames = audio.values_mut().filter_map(VecDeque::pop_front);
    let mut mixed = frames.next()?;
    for frame in frames {
        for (out, value) in mixed.iter_mut().zip(frame) {
            *out += value;
        }
    }
    for sample in &mut mixed {
        *sample = sample.clamp(-1.0, 1.0);
    }
    Some(mixed)
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.state.stop.send_replace(true);
        for peer in self.state.peers.lock().values() {
            peer.cancel.send_replace(true);
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    }
}

impl State {
    fn event(&self, mut value: Value) {
        if value["type"] == "pairing" {
            match value["kind"].as_str().unwrap_or_default() {
                "consent" | "enter_pin" | "show_pin" | "waiting" => {
                    let attempt = value["attempt_id"]
                        .as_u64()
                        .unwrap_or_else(|| self.serial.fetch_add(1, Ordering::Relaxed));
                    value["attempt_id"] = json!(attempt);
                    self.prompt.store(attempt, Ordering::Release);
                }
                "closed" | "complete" => {
                    self.prompt.store(0, Ordering::Release);
                }
                _ => {}
            }
        }
        let mut events = self.events.lock();
        // Frequent snapshots supersede earlier snapshots. Motion coalesces only
        // across adjacent moves so clicks and releases retain their ordering.
        if value["type"] == "snapshot" {
            events.retain(|e| e["type"] != "snapshot");
        }
        if value["kind"] == "move"
            && let Some(last) = events.back_mut()
            && last["kind"] == "move"
            && last["device_id"] == value["device_id"]
        {
            last["dx"] =
                json!(last["dx"].as_i64().unwrap_or(0) + value["dx"].as_i64().unwrap_or(0));
            last["dy"] =
                json!(last["dy"].as_i64().unwrap_or(0) + value["dy"].as_i64().unwrap_or(0));
            return;
        }
        if events.len() >= EVENT_LIMIT {
            events.clear();
            events.push_back(json!({"type":"input","kind":"release"}));
            drop(events);
            let owner = self.owner.lock().take();
            if let Some(id) = owner
                && let Some(peer) = self.peers.lock().get(&id)
            {
                let _ = peer.control.try_send(ControlMsg::ForceRelease);
            }
            return;
        }
        events.push_back(value);
    }
    fn error(&self, error: impl std::fmt::Display) {
        self.event(json!({"type":"error","message":error.to_string()}));
    }
    fn snapshot(&self) {
        let saved = self.saved.lock();
        let adverts = self.adverts.lock();
        let peers = self.peers.lock();
        let owner = self.owner.lock();
        let mut ids: Vec<_> = saved
            .trusted
            .keys()
            .chain(adverts.keys())
            .chain(peers.keys())
            .cloned()
            .collect();
        ids.sort();
        ids.dedup();
        let devices: Vec<_> = ids.into_iter().map(|id| {
            let trusted = saved.trusted.get(&id); let advert = adverts.get(&id); let peer = peers.get(&id);
            json!({"device_id":id,"display_name":peer.map(|p|p.name.as_str()).or(advert.map(|p|p.name.as_str())).or(trusted.map(|p|p.name.as_str())).unwrap_or("MineShare"),
                "os":peer.map(|p|p.os.as_str()).or(advert.map(|p|p.os.as_str())).unwrap_or("unknown"),
                "connected":peer.is_some(),"online":advert.is_some() || peer.is_some(),"paired":trusted.is_some(),"compatible":advert.is_none_or(|p|p.compatible),
                "input_active":owner.as_deref()==Some(id.as_str()),"send_audio":trusted.is_some_and(|p|p.send),"receive_audio":trusted.is_some_and(|p|p.receive)})
        }).collect();
        let snapshot = json!({"type":"snapshot","device_id":self.identity.device_id.to_string(),"paused":self.paused.load(Ordering::Acquire),"peers":devices});
        drop(owner);
        drop(peers);
        drop(adverts);
        drop(saved);
        self.event(snapshot);
    }
    fn trust(&self, id: &str, name: &str, key: &[u8]) -> Result<()> {
        let mut saved = self.saved.lock();
        let mut next = saved.clone();
        next.trusted.insert(
            id.into(),
            Trusted {
                name: name.into(),
                key: key.to_vec(),
                send: false,
                receive: true,
            },
        );
        persist(&self.directory.join("peers.json"), &next)?;
        *saved = next;
        Ok(())
    }
    fn is_trusted(&self, id: &str, key: &[u8]) -> bool {
        self.saved
            .lock()
            .trusted
            .get(id)
            .is_some_and(|p| p.key == key)
    }
    fn receiving(&self, id: &str) -> bool {
        self.saved.lock().trusted.get(id).is_some_and(|p| p.receive)
    }
    fn sending(&self, id: &str) -> bool {
        self.saved.lock().trusted.get(id).is_some_and(|p| p.send)
    }
    fn release(&self, id: &str) {
        let mut owner = self.owner.lock();
        if owner.as_deref() == Some(id) {
            *owner = None;
            drop(owner);
            self.event(json!({"type":"input","device_id":id,"kind":"release"}));
            self.snapshot();
        }
    }
}

fn version() -> String {
    format!("{} · Android · control-v5", env!("CARGO_PKG_VERSION"))
}

fn dual_stack_listener() -> std::io::Result<std::net::TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_only_v6(false)?;
    socket.bind(&"[::]:0".parse::<SocketAddr>().unwrap().into())?;
    socket.listen(32)?;
    Ok(socket.into())
}
fn text_field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("missing {name}"))
}
fn persist(path: &Path, value: &impl Serialize) -> Result<()> {
    let temp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
    }
    std::fs::rename(temp, path)?;
    Ok(())
}

async fn accept_loop(state: Arc<State>, listener: TcpListener) {
    let mut stop = state.stop.subscribe();
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream,_)) if !state.paused.load(Ordering::Acquire) => {
                    let state = state.clone();
                    tokio::spawn(async move { if let Err(error) = session(state.clone(),stream,false,None,false).await { state.error(error); } });
                }
                Ok(_) => {},
                Err(error) => { state.error(error); tokio::time::sleep(Duration::from_secs(1)).await; }
            }
        }
    }
}
async fn reconnect_loop(state: Arc<State>) {
    let mut stop = state.stop.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {
                if state.paused.load(Ordering::Acquire) { continue; }
                let ids: Vec<_> = state.saved.lock().trusted.keys().cloned().collect();
                for id in ids {
                    if state.adverts.lock().get(&id).is_some_and(|p|p.compatible) && !state.peers.lock().contains_key(&id) {
                        tokio::spawn(dial(state.clone(),id,false));
                    }
                }
            }
        }
    }
}
async fn dial(state: Arc<State>, id: String, requested: bool) {
    if state.dialing.lock().insert(id.clone(), true).is_some() {
        return;
    }
    let advert = state.adverts.lock().get(&id).cloned();
    if let Some(advert) = advert {
        let result = async {
            let stream =
                tokio::time::timeout(SETUP_TIMEOUT, TcpStream::connect(advert.address)).await??;
            session(state.clone(), stream, true, Some(id.clone()), requested).await
        }
        .await;
        if let Err(error) = result
            && requested
        {
            state.error(error);
        }
    }
    state.dialing.lock().remove(&id);
}

async fn encode_loop(state: Arc<State>, mut pcm: mpsc::Receiver<Vec<f32>>) {
    let encoder = opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio);
    let Ok(mut encoder) = encoder else {
        state.error("Opus encoder unavailable");
        return;
    };
    let _ = encoder.set_bitrate(opus::Bitrate::Bits(96000));
    let mut pending = VecDeque::new();
    let mut seq = 0u32;
    let mut output = vec![0u8; 1500];
    let mut stop = state.stop.subscribe();
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            block = pcm.recv() => {
                let Some(block)=block else {break}; pending.extend(block);
                while pending.len() >= FRAME_SAMPLES {
                    let samples:Vec<_>=pending.drain(..FRAME_SAMPLES).collect();
                    if let Ok(n)=encoder.encode_float(&samples,&mut output) {
                        let _=state.encoded.send(AudioFrame{stream:StreamKind::SysOut,seq,opus:output[..n].to_vec()}); seq=seq.wrapping_add(1);
                    }
                }
            }
        }
    }
}

async fn write_control<W: AsyncWriteExt + Unpin, T: Serialize>(
    stream: &mut W,
    aead: &EncryptedSession,
    value: &T,
) -> Result<()> {
    let bytes = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
    ensure!(bytes.len() <= 65519, "control frame too large");
    let bytes = aead.seal_for(TransportDomain::Control, &bytes)?;
    tokio::time::timeout(SETUP_TIMEOUT, async {
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await?;
        stream.write_all(&bytes).await
    })
    .await??;
    Ok(())
}
async fn read_control<R: AsyncReadExt + Unpin, T: for<'a> Deserialize<'a>>(
    stream: &mut R,
    aead: &EncryptedSession,
) -> Result<T> {
    let len = stream.read_u32().await? as usize;
    ensure!((24..=65543).contains(&len), "invalid control frame length");
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes).await?;
    let bytes = aead.open_for(TransportDomain::Control, &bytes)?;
    let (value, used) = bincode::serde::decode_from_slice(
        &bytes,
        bincode::config::standard().with_limit::<65535>(),
    )?;
    ensure!(used == bytes.len(), "trailing control bytes");
    Ok(value)
}

async fn answer(
    state: &Arc<State>,
    stream: &mut TcpStream,
    aead: &EncryptedSession,
    peer: &ConnectionHello,
    kind: &'static str,
) -> Result<Value> {
    let attempt = state.serial.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    state.pending.lock().insert(
        attempt,
        Pending {
            peer_id: peer.device_id.to_string(),
            kind,
            reply: tx,
        },
    );
    let _pending = PendingGuard { state, attempt };
    state.event(json!({"type":"pairing","attempt_id":attempt,"device_id":peer.device_id.to_string(),"display_name":peer.display_name,"kind":kind}));
    let mut stop = state.stop.subscribe();
    let response = tokio::select! {
        response=tokio::time::timeout(Duration::from_secs(90),rx)=>response.context("pairing timed out")?.context("pairing cancelled"),
        _=stop.changed()=>Err(anyhow::anyhow!("engine stopped")),
        result=read_control::<_,ControlMsg>(stream,aead)=>{result.context("device disconnected during pairing")?;Err(anyhow::anyhow!("unexpected pairing message"))}
    };
    state.pending.lock().remove(&attempt);
    response
}

async fn session(
    state: Arc<State>,
    stream: TcpStream,
    initiator: bool,
    expected: Option<String>,
    requested: bool,
) -> Result<()> {
    let mut cancelled = state.setup_cancel.subscribe();
    let mut stop = state.stop.subscribe();
    let (ready_tx, mut ready_rx) = oneshot::channel();
    let operation = session_inner(
        state.clone(),
        stream,
        initiator,
        expected,
        requested,
        ready_tx,
    );
    tokio::pin!(operation);
    let result = tokio::select! {
        result = &mut operation => result,
        _ = cancelled.changed() => Err(anyhow::anyhow!("pairing cancelled")),
        _ = stop.changed() => Err(anyhow::anyhow!("engine stopped")),
        ready = &mut ready_rx => {
            ready.context("session setup failed")?;
            operation.await
        }
    };
    result
}

async fn session_inner(
    state: Arc<State>,
    mut stream: TcpStream,
    initiator: bool,
    expected: Option<String>,
    requested: bool,
    ready: oneshot::Sender<()>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let noise = tokio::time::timeout(SETUP_TIMEOUT, async {
        if initiator {
            Initiator::new(&state.identity.private_key)?
                .handshake(&mut stream)
                .await
        } else {
            Responder::new(&state.identity.private_key)?
                .handshake(&mut stream)
                .await
        }
    })
    .await??;
    let key = noise.remote_static;
    let known = state.saved.lock().trusted.values().any(|p| p.key == key);
    let aead = EncryptedSession::from(noise);
    let hello = ConnectionHello {
        magic: CONNECTION_MAGIC,
        device_id: state.identity.device_id,
        display_name: state.name.clone(),
        trusts_peer: known,
        requested,
    };
    let peer: ConnectionHello = tokio::time::timeout(SETUP_TIMEOUT, async {
        write_control(&mut stream, &aead, &hello).await?;
        read_control(&mut stream, &aead).await
    })
    .await??;
    peer.validate_for(
        expected
            .as_ref()
            .map(|s| s.parse::<uuid::Uuid>().map(DeviceId))
            .transpose()?,
    )?;
    ensure!(peer.device_id != hello.device_id, "self connection");
    ensure!(!initiator || !peer.requested, "reverse pairing request");
    let id = peer.device_id.to_string();
    let trusted = state.is_trusted(&id, &key) && peer.trusts_peer;
    ensure!(
        trusted || requested || peer.requested,
        "pair this device first"
    );
    ensure!(!state.paused.load(Ordering::Acquire), "connections paused");
    let pairing_slot = if requested || peer.requested {
        Some(
            state
                .pairing_gate
                .try_lock()
                .context("another pairing request is pending")?,
        )
    } else {
        None
    };
    let pairing_ui = pairing_slot.as_ref().map(|_| PairingUiGuard(&state));
    if requested || peer.requested {
        if initiator {
            state.event(json!({"type":"pairing","device_id":id,"display_name":peer.display_name,"kind":"waiting"}));
            match tokio::time::timeout(
                Duration::from_secs(90),
                read_control::<_, ControlMsg>(&mut stream, &aead),
            )
            .await??
            {
                ControlMsg::PairingDecision { accepted: true, .. } => {}
                _ => bail!("pairing request declined"),
            }
        } else {
            let response = answer(&state, &mut stream, &aead, &peer, "consent").await?;
            let accepted = response["accept"].as_bool().unwrap_or(false);
            write_control(
                &mut stream,
                &aead,
                &ControlMsg::PairingDecision {
                    accepted,
                    reason: if accepted {
                        String::new()
                    } else {
                        "Declined on phone".into()
                    },
                },
            )
            .await?;
            ensure!(accepted, "pairing declined");
        }
        if !trusted {
            if initiator {
                let response = answer(&state, &mut stream, &aead, &peer, "enter_pin").await?;
                write_control(
                    &mut stream,
                    &aead,
                    &ControlMsg::PinAttempt {
                        pin: text_field(&response, "pin")?.into(),
                        device_id: hello.device_id.to_string(),
                        display_name: hello.display_name.clone(),
                    },
                )
                .await?;
                match tokio::time::timeout(
                    Duration::from_secs(60),
                    read_control::<_, ControlMsg>(&mut stream, &aead),
                )
                .await??
                {
                    ControlMsg::PinAck {
                        ok: true,
                        device_id,
                        ..
                    } if device_id == id => {}
                    _ => bail!("PIN was rejected"),
                }
            } else {
                let pin = format!("{:06}", rand::random_range(0..1_000_000u32));
                let attempt = state.serial.fetch_add(1, Ordering::Relaxed);
                state.event(json!({"type":"pairing","attempt_id":attempt,"device_id":id,"display_name":peer.display_name,"kind":"show_pin","pin":pin}));
                let input = tokio::time::timeout(
                    Duration::from_secs(60),
                    read_control::<_, ControlMsg>(&mut stream, &aead),
                )
                .await??;
                let ok = matches!(input,ControlMsg::PinAttempt{pin:ref p,device_id:ref d,..} if p==&pin && d==&id);
                write_control(
                    &mut stream,
                    &aead,
                    &ControlMsg::PinAck {
                        ok,
                        device_id: hello.device_id.to_string(),
                        display_name: hello.display_name.clone(),
                    },
                )
                .await?;
                ensure!(ok, "incorrect PIN");
            }
            ensure!(!state.paused.load(Ordering::Acquire), "connections paused");
            state.trust(&id, &peer.display_name, &key)?;
        }
    }
    ensure!(state.is_trusted(&id, &key), "device identity changed");
    let mut peer_address = stream.peer_addr()?;
    if let IpAddr::V6(ip) = peer_address.ip()
        && let Some(ip) = ip.to_ipv4_mapped()
    {
        peer_address = SocketAddr::new(ip.into(), peer_address.port());
    }
    let udp = UdpSocket::bind(if peer_address.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })
    .await?;
    let announce = PortAnnounce {
        udp_port: udp.local_addr()?.port(),
        daemon_version: version(),
        screen_w: state.width.load(Ordering::Acquire),
        screen_h: state.height.load(Ordering::Acquire),
        touchpad_capabilities: 0,
        capabilities: DeviceCapabilities {
            os: "android".into(),
            pointer: true,
            keyboard: true,
            text_input: true,
            system_audio_send: true,
            system_audio_receive: true,
            clipboard: false,
            files: false,
        },
    };
    let remote: PortAnnounce = tokio::time::timeout(SETUP_TIMEOUT, async {
        write_control(&mut stream, &aead, &announce).await?;
        read_control(&mut stream, &aead).await
    })
    .await??;
    ensure!(
        remote.udp_port != 0 && remote.daemon_version.ends_with(" · control-v5"),
        "update MineShare on both devices"
    );
    let mut remote_udp = peer_address;
    remote_udp.set_port(remote.udp_port);
    udp.connect(remote_udp).await?;
    ensure!(!state.paused.load(Ordering::Acquire), "connections paused");
    let token = state.serial.fetch_add(1, Ordering::Relaxed);
    let (cancel, mut cancelled) = watch::channel(false);
    let (tx, rx) = mpsc::channel(64);
    let preferred = initiator == (state.identity.public_key.as_slice() < key.as_slice());
    {
        let mut peers = state.peers.lock();
        if let Some(old) = peers.get(&id) {
            ensure!(preferred && !old.preferred, "duplicate connection");
            old.cancel.send_replace(true);
        }
        peers.insert(
            id.clone(),
            Peer {
                name: peer.display_name.clone(),
                os: remote.capabilities.os,
                token,
                preferred,
                cancel,
                control: tx,
            },
        );
    }
    drop(pairing_ui);
    if requested || peer.requested {
        state.event(json!({"type":"pairing","device_id":id,"kind":"complete"}));
    }
    drop(pairing_slot);
    state.snapshot();
    let _ = ready.send(());
    let result = tokio::select! {
        _=cancelled.changed()=>Ok(()),
        result=run_connected(state.clone(),&id,stream,udp,aead,rx)=>result,
    };
    let current = {
        let mut peers = state.peers.lock();
        if peers.get(&id).is_some_and(|p| p.token == token) {
            peers.remove(&id);
            true
        } else {
            false
        }
    };
    if current {
        state.release(&id);
        state.audio.lock().remove(&id);
        state.snapshot();
    }
    state.pending.lock().retain(|_, p| p.peer_id != id);
    result
}

async fn run_connected(
    state: Arc<State>,
    id: &str,
    stream: TcpStream,
    udp: UdpSocket,
    aead: EncryptedSession,
    mut controls: mpsc::Receiver<ControlMsg>,
) -> Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let (reader_tx, mut reader_rx) = mpsc::channel(64);
    let reader_aead = aead.clone_handle();
    let read_task = tokio::spawn(async move {
        loop {
            let value = tokio::time::timeout(
                Duration::from_secs(6),
                read_control::<_, ControlMsg>(&mut reader, &reader_aead),
            )
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
            let done = value.is_err();
            if reader_tx.send(value).await.is_err() || done {
                break;
            }
        }
    });
    // Abort on every exit, including cancellation of this future by its caller.
    struct Abort(tokio::task::JoinHandle<()>);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _reader = Abort(read_task);
    let mut encoded = state.encoded.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut stop = state.stop.subscribe();
    let mut buffer = vec![0u8; 16384];
    let mut decoder = opus::Decoder::new(48000, opus::Channels::Stereo)?;
    let mut decode_buffer = vec![0f32; FRAME_SAMPLES * 6];
    let mut last_audio = None;
    let mut last_input = None;
    let mut held = ForwardedInputState::default();
    let mut remote_receives = true;
    write_control(
        &mut writer,
        &aead,
        &ControlMsg::AudioRoute {
            receive_system: state.receiving(id),
        },
    )
    .await?;
    loop {
        tokio::select! {
            _=stop.changed()=>return Ok(()),
            control=controls.recv()=>{let Some(control)=control else{return Ok(())};write_control(&mut writer,&aead,&control).await?;},
            _=tick.tick()=>write_control(&mut writer,&aead,&ControlMsg::Ping{ts_us:0,lock_visual:None}).await?,
            value=reader_rx.recv()=>{
                let control=value.context("peer disconnected")??;
                match control {
                    ControlMsg::Ping{ts_us,..}=>write_control(&mut writer,&aead,&ControlMsg::Pong{ts_us}).await?,
                    ControlMsg::TakeControl{epoch}=>{
                        // The device whose physical mouse is crossing now wins: a previous owner (e.g. the laptop
                        // touchpad) is told to hand the cursor back instead of locking the new peer out.
                        let previous={let mut owner=state.owner.lock();let old=owner.take().filter(|o|o!=id);*owner=Some(id.into());old};
                        if let Some(old)=previous{
                            if let Some(peer)=state.peers.lock().get(&old){let _=peer.control.try_send(ControlMsg::ForceRelease);}
                            state.event(json!({"type":"input","device_id":old,"kind":"release"}));
                        }
                        let allowed=true;
                        if allowed {held=ForwardedInputState::default();state.event(json!({"type":"input","device_id":id,"kind":"take_control","epoch":epoch}));state.snapshot();}
                        else{write_control(&mut writer,&aead,&ControlMsg::ForceRelease).await?;}
                    }
                    ControlMsg::ReleaseControl|ControlMsg::ForceRelease=>{held=ForwardedInputState::default();state.release(id);},
                    ControlMsg::TextInput(text) if state.owner.lock().as_deref()==Some(id)&&text.len()<=65500=>state.event(json!({"type":"input","kind":"text","device_id":id,"text":text})),
                    ControlMsg::AudioRoute{receive_system}=>remote_receives=receive_system,
                    ControlMsg::SetPeerSide(side)=>state.event(json!({"type":"input","kind":"peer_side","device_id":id,"side":side})),
                    _=>{},
                }
            }
            result=udp.recv(&mut buffer)=>{
                let size=result?;let Ok(bytes)=aead.open_for(TransportDomain::Datagram,&buffer[..size])else{continue};
                let Ok((frame,used))=bincode::serde::decode_from_slice::<WireFrame,_>(&bytes,bincode::config::standard().with_limit::<16384>())else{continue};if used!=bytes.len(){continue;}
                match frame {
                    WireFrame::Audio(frame) if frame.stream==StreamKind::SysOut && state.receiving(id)=>{
                        if frame.opus.len()>1500 || last_audio.is_some_and(|seq:u32|frame.seq.wrapping_sub(seq) as i32<=0){continue;}last_audio=Some(frame.seq);
                        if let Ok(samples)=decoder.decode_float(&frame.opus,&mut decode_buffer,false){
                            let mut audio=state.audio.lock();let queue=audio.entry(id.into()).or_default();
                            for chunk in decode_buffer[..samples*2].chunks_exact(FRAME_SAMPLES){if queue.len()>=4{queue.pop_front();}queue.push_back(chunk.to_vec());}
                        }
                    }
                    WireFrame::Input{seq,event} if state.owner.lock().as_deref()==Some(id)=>{
                        if last_input.is_some_and(|old|seq<=old){continue;}last_input=Some(seq);held.apply_event(event);input_event(&state,id,event);
                    }
                    WireFrame::InputState{after_input_seq,state:desired,..} if state.owner.lock().as_deref()==Some(id)=>{
                        if last_input.is_none_or(|seq|after_input_seq>=seq){for event in held.reconciliation_events(&desired){input_event(&state,id,event);}held=desired;last_input=Some(after_input_seq);}
                    }
                    _=>{},
                }
            }
            audio=encoded.recv()=>{
                if let Ok(audio)=audio && state.sending(id)&&remote_receives {
                    let bytes=bincode::serde::encode_to_vec(WireFrame::Audio(audio),bincode::config::standard())?;
                    let bytes=aead.seal_for(TransportDomain::Datagram,&bytes)?;udp.send(&bytes).await?;
                }
            }
        }
    }
}

fn input_event(state: &State, id: &str, event: InputEvent) {
    let mut value = match event {
        InputEvent::MouseMove { dx, dy } => json!({"kind":"move","dx":dx,"dy":dy}),
        InputEvent::MouseButton { btn, down } => {
            json!({"kind":"button","button":format!("{btn:?}"),"down":down})
        }
        InputEvent::Scroll { dx, dy } => json!({"kind":"scroll","dx":dx,"dy":dy}),
        InputEvent::Key { code, down } => json!({"kind":"key","code":code.0,"down":down}),
        InputEvent::Touchpad(_) => return,
    };
    value["type"] = json!("input");
    value["device_id"] = json!(id);
    state.event(value);
}

#[cfg(test)]
mod audio_mix_tests {
    use super::*;

    #[test]
    fn empty_polls_and_single_source_reuse_the_queued_buffer() {
        let mut audio = HashMap::new();
        assert!(mix_audio(&mut audio).is_none());
        let frame = vec![0.25; FRAME_SAMPLES];
        let address = frame.as_ptr();
        audio.insert("phone".into(), VecDeque::from([frame]));
        let output = mix_audio(&mut audio).unwrap();
        assert_eq!(
            output.as_ptr(),
            address,
            "single-source mixing must reuse PCM storage"
        );
        assert!(output.iter().all(|value| *value == 0.25));
        assert!(mix_audio(&mut audio).is_none());
    }

    #[test]
    fn multiple_sources_mix_clip_and_preserve_the_next_frame() {
        let mut audio = HashMap::from([
            (
                "first".into(),
                VecDeque::from([vec![0.75, -0.75, 0.2], vec![0.1; 3]]),
            ),
            ("second".into(), VecDeque::from([vec![0.75, -0.75, -0.2]])),
        ]);
        assert_eq!(mix_audio(&mut audio).unwrap(), vec![1.0, -1.0, 0.0]);
        assert_eq!(mix_audio(&mut audio).unwrap(), vec![0.1; 3]);
        assert!(mix_audio(&mut audio).is_none());
    }
}
