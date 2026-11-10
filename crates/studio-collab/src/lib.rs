//! Live collaboration without a server.
//!
//! One participant hosts (a TCP port on their machine, opened on the
//! router by UPnP when possible); the others join with the invitation
//! code. The host orders every change (a sequence number per
//! transaction) and relays changes, recorded takes and imported files,
//! transport commands and chat. Each participant applies their own
//! changes at once and rebases them over others' when they arrive
//! ([`Replica`]), so everyone converges to the same project. Projects and
//! audio stay on the participants' machines.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

pub mod discovery;
pub mod invite;
pub mod proto;
pub mod replica;

use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;
use studio_core::model::{Id, Project};
use studio_core::ops::Transaction;

pub use invite::Invite;
pub use proto::{PeerId, PeerInfo, TransportMsg};
pub use replica::Replica;

use proto::*;

pub const DEFAULT_PORT: u16 = 47800;
pub const HOST_ID: PeerId = 1;

/// What happens in the session (poll it from the UI thread).
#[derive(Clone, Debug)]
pub enum Event {
    /// Joined: the host's project and the sequence it is at.
    Welcome { you: PeerId, project: Box<Project>, seq: u64 },
    /// Someone else's change, in order: apply it (with a [`Replica`]).
    Remote { seq: u64, author: PeerId, tx: Transaction },
    /// One of our changes got its place in the order.
    Ack { seq: u64, tx: Id },
    /// The host replaced the whole project.
    Reset { project: Box<Project>, seq: u64 },
    Peers(Vec<PeerInfo>),
    PeerJoined(PeerInfo),
    PeerLeft { id: PeerId, name: String },
    Chat { from: PeerId, name: String, text: String },
    /// Transport: `delay_ms` until the moment the host asked for (Play).
    Transport { from: PeerId, msg: TransportMsg, delay_ms: f64 },
    /// (host) A new participant needs the project: call [`Session::welcome`].
    WantsProject { peer: PeerId },
    /// (host) A participant lacks an asset: call [`Session::send_asset_to`].
    WantsAsset { peer: PeerId, id: String },
    /// A complete asset arrived.
    Asset { id: String, name: String, bytes: Arc<Vec<u8>> },
    AssetProgress { id: String, done: u64, total: u64 },
    Disconnected { reason: String },
    Info(String),
}

enum Hub {
    // from the application
    Propose(Transaction),
    Welcome { peer: PeerId, project: Box<Project>, seq: u64 },
    Reset { project: Box<Project> },
    Chat(String),
    Transport(TransportMsg),
    Status(String),
    ShareAsset { id: String, name: String, bytes: Arc<Vec<u8>>, to: Option<PeerId> },
    RequestAsset(String),
    Leave,
    // from the network
    Joined { id: PeerId, info: PeerInfo, out: Sender<Frame> },
    Frame { from: PeerId, frame: Frame },
    Left { id: PeerId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Host,
    Client,
}

pub struct Session {
    pub role: Role,
    pub me: PeerId,
    pub name: String,
    /// The code to give friends (host only).
    pub invite: Option<String>,
    pub port: u16,
    pub upnp: bool,
    hub: Sender<Hub>,
    events: Receiver<Event>,
    /// Host clock minus local clock (ms).
    offset: Arc<Mutex<f64>>,
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0
}

const COLORS: [&str; 6] = ["#e86a5c", "#5c9ce8", "#7bd389", "#e8b25c", "#b77be8", "#5cd6e8"];

impl Session {
    /// Hosts a session on `port` (0: the default, or the next free one).
    pub fn host(name: &str, port: u16, try_upnp: bool) -> anyhow::Result<Session> {
        let mut listener = None;
        let base = if port == 0 { DEFAULT_PORT } else { port };
        for p in base..base + 20 {
            if let Ok(l) = TcpListener::bind(("0.0.0.0", p)) {
                listener = Some(l);
                break;
            }
        }
        let listener = listener.ok_or_else(|| anyhow::anyhow!("aucun port libre pour héberger"))?;
        let port = listener.local_addr()?.port();
        let secret = invite::new_secret();
        let master = master_key(&secret);
        let mut addrs = Vec::new();
        let mut upnp = false;
        if try_upnp {
            if let Some(ext) = invite::upnp_map(port) {
                addrs.push(ext);
                upnp = true;
            }
        }
        if let Some(ip) = invite::local_ip() {
            addrs.push(SocketAddr::new(ip, port));
        }
        addrs.push(SocketAddr::from(([127, 0, 0, 1], port)));
        let code = Invite { addrs, secret }.encode();

        let (hub_tx, hub_rx) = unbounded::<Hub>();
        let (ev_tx, ev_rx) = unbounded::<Event>();
        // accept loop
        {
            let hub = hub_tx.clone();
            std::thread::Builder::new().name("awra-collab-accept".into()).spawn(move || {
                let mut next_id: PeerId = HOST_ID + 1;
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let id = next_id;
                    next_id += 1;
                    let hub = hub.clone();
                    std::thread::spawn(move || host_connection(stream, id, master, hub));
                }
            })?;
        }
        let me = PeerInfo { id: HOST_ID, name: name.to_string(), color: COLORS[0].into(), status: "idle".into() };
        std::thread::Builder::new()
            .name("awra-collab-hub".into())
            .spawn(move || host_hub(me, hub_rx, ev_tx))?;
        discovery::beacon(name, port);
        Ok(Session {
            role: Role::Host,
            me: HOST_ID,
            name: name.to_string(),
            invite: Some(code),
            port,
            upnp,
            hub: hub_tx,
            events: ev_rx,
            offset: Arc::new(Mutex::new(0.0)),
        })
    }

    /// Joins with an invitation code.
    pub fn join(code: &str, name: &str) -> anyhow::Result<Session> {
        let inv = Invite::decode(code).ok_or_else(|| anyhow::anyhow!("code d'invitation invalide"))?;
        let master = master_key(&inv.secret);
        let mut last_err = anyhow::anyhow!("hôte injoignable");
        for addr in &inv.addrs {
            match TcpStream::connect_timeout(addr, Duration::from_secs(3)) {
                Ok(s) => match Session::client(s, master, name) {
                    Ok(sess) => return Ok(sess),
                    Err(e) => last_err = e,
                },
                Err(e) => last_err = anyhow::anyhow!("{addr} : {e}"),
            }
        }
        Err(last_err)
    }

    fn client(mut stream: TcpStream, master: [u8; 32], name: &str) -> anyhow::Result<Session> {
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mine = random32();
        let theirs = greet(&mut stream, &mine)?;
        let key = connection_key(&master, &mine, &theirs);
        let mut tx_cipher = Cipher::new(&key, 0);
        let mut rx_cipher = Cipher::new(&key, 1);
        let mut w = BufWriter::new(stream.try_clone()?);
        write_frame(&mut w, &mut tx_cipher, &Frame::Msg(Msg::Hello { name: name.into(), color: String::new(), version: VERSION }))?;
        let mut r = BufReader::new(stream.try_clone()?);
        // the first answer proves the secret was right
        let first = read_frame(&mut r, &mut rx_cipher).map_err(|e| anyhow::anyhow!("connexion refusée : {e}"))?;
        stream.set_read_timeout(None)?;
        let (hub_tx, hub_rx) = unbounded::<Hub>();
        let (ev_tx, ev_rx) = unbounded::<Event>();
        let offset = Arc::new(Mutex::new(0.0f64));
        let me = match &first {
            Frame::Msg(Msg::Welcome { you, .. }) => *you,
            _ => 0,
        };
        // writer
        let (out_tx, out_rx) = unbounded::<Frame>();
        std::thread::spawn(move || {
            for f in out_rx {
                if write_frame(&mut w, &mut tx_cipher, &f).is_err() {
                    break;
                }
            }
        });
        // reader
        {
            let hub = hub_tx.clone();
            std::thread::spawn(move || {
                let _ = hub.send(Hub::Frame { from: HOST_ID, frame: first });
                loop {
                    match read_frame(&mut r, &mut rx_cipher) {
                        Ok(f) => {
                            if hub.send(Hub::Frame { from: HOST_ID, frame: f }).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            let _ = hub.send(Hub::Left { id: HOST_ID });
                            break;
                        }
                    }
                }
            });
        }
        // clock pings
        {
            let out = out_tx.clone();
            std::thread::spawn(move || loop {
                if out.send(Frame::Msg(Msg::Ping { t0: now_ms() })).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(2));
            });
        }
        {
            let offset = offset.clone();
            std::thread::Builder::new()
                .name("awra-collab-client".into())
                .spawn(move || client_hub(me, hub_rx, ev_tx, out_tx, offset))?;
        }
        Ok(Session {
            role: Role::Client,
            me,
            name: name.to_string(),
            invite: None,
            port: 0,
            upnp: false,
            hub: hub_tx,
            events: ev_rx,
            offset,
        })
    }

    /// Pending events (non-blocking).
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Shares a change made locally (already applied by the caller).
    pub fn propose(&self, tx: Transaction) {
        let _ = self.hub.send(Hub::Propose(tx));
    }
    /// (host) Answers [`Event::WantsProject`]: the project as applied up to `seq`.
    pub fn welcome(&self, peer: PeerId, project: Project, seq: u64) {
        let _ = self.hub.send(Hub::Welcome { peer, project: Box::new(project), seq });
    }
    /// (host) Replaces everyone's project (a project opened while hosting).
    pub fn reset(&self, project: Project) {
        let _ = self.hub.send(Hub::Reset { project: Box::new(project) });
    }
    pub fn chat(&self, text: &str) {
        let _ = self.hub.send(Hub::Chat(text.to_string()));
    }
    /// Shared transport. For Play, `at_ms` is set here (in 250 ms, on the
    /// host's clock) so that everyone starts together.
    pub fn transport(&self, mut msg: TransportMsg) {
        if let TransportMsg::Play { at_ms, .. } = &mut msg {
            *at_ms = now_ms() + *self.offset.lock() + 250.0;
        }
        let _ = self.hub.send(Hub::Transport(msg));
    }
    pub fn set_status(&self, status: &str) {
        let _ = self.hub.send(Hub::Status(status.to_string()));
    }
    /// Sends an asset's bytes to everyone (or to one participant).
    pub fn share_asset(&self, id: &str, name: &str, bytes: Arc<Vec<u8>>) {
        let _ = self.hub.send(Hub::ShareAsset { id: id.into(), name: name.into(), bytes, to: None });
    }
    pub fn send_asset_to(&self, peer: PeerId, id: &str, name: &str, bytes: Arc<Vec<u8>>) {
        let _ = self.hub.send(Hub::ShareAsset { id: id.into(), name: name.into(), bytes, to: Some(peer) });
    }
    pub fn request_asset(&self, id: &str) {
        let _ = self.hub.send(Hub::RequestAsset(id.into()));
    }
    pub fn leave(&self) {
        let _ = self.hub.send(Hub::Leave);
        if self.upnp {
            let port = self.port;
            std::thread::spawn(move || invite::upnp_unmap(port));
        }
    }
    /// Host clock minus ours (ms).
    pub fn clock_offset(&self) -> f64 {
        *self.offset.lock()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.leave();
    }
}

fn host_connection(mut stream: TcpStream, id: PeerId, master: [u8; 32], hub: Sender<Hub>) {
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mine = random32();
    let Ok(theirs) = greet(&mut stream, &mine) else { return };
    let key = connection_key(&master, &theirs, &mine);
    let mut rx_cipher = Cipher::new(&key, 0);
    let mut tx_cipher = Cipher::new(&key, 1);
    let Ok(rs) = stream.try_clone() else { return };
    let mut r = BufReader::new(rs);
    // a wrong secret fails here: the connection just closes
    let Ok(Frame::Msg(Msg::Hello { name, .. })) = read_frame(&mut r, &mut rx_cipher) else { return };
    stream.set_read_timeout(None).ok();
    let (out_tx, out_rx) = unbounded::<Frame>();
    let Ok(ws) = stream.try_clone() else { return };
    std::thread::spawn(move || {
        let mut w = BufWriter::new(ws);
        for f in out_rx {
            if write_frame(&mut w, &mut tx_cipher, &f).is_err() {
                break;
            }
        }
    });
    let info = PeerInfo { id, name, color: COLORS[(id as usize) % COLORS.len()].into(), status: "idle".into() };
    if hub.send(Hub::Joined { id, info, out: out_tx }).is_err() {
        return;
    }
    loop {
        match read_frame(&mut r, &mut rx_cipher) {
            Ok(frame) => {
                if hub.send(Hub::Frame { from: id, frame }).is_err() {
                    break;
                }
            }
            Err(_) => {
                let _ = hub.send(Hub::Left { id });
                break;
            }
        }
    }
}

struct Incoming {
    name: String,
    size: u64,
    data: Vec<u8>,
}

struct Client {
    info: PeerInfo,
    out: Sender<Frame>,
    /// Welcomed: receives commits.
    active: bool,
}

fn send_asset(out: &Sender<Frame>, id: &str, name: &str, bytes: &[u8]) {
    let _ = out.send(Frame::Msg(Msg::AssetBegin { id: id.into(), name: name.into(), size: bytes.len() as u64 }));
    for (i, c) in bytes.chunks(CHUNK).enumerate() {
        let _ = out.send(Frame::Chunk { id: id.into(), offset: (i * CHUNK) as u64, data: c.to_vec() });
    }
    if bytes.is_empty() {
        let _ = out.send(Frame::Chunk { id: id.into(), offset: 0, data: Vec::new() });
    }
}

/// Adds a chunk; returns the completed asset.
fn take_chunk(
    incoming: &mut HashMap<String, Incoming>,
    events: &Sender<Event>,
    id: String,
    offset: u64,
    data: Vec<u8>,
) -> Option<(String, Arc<Vec<u8>>)> {
    let inc = incoming.get_mut(&id)?;
    let end = offset as usize + data.len();
    if inc.data.len() < end {
        inc.data.resize(end, 0);
    }
    inc.data[offset as usize..end].copy_from_slice(&data);
    let done = inc.data.len() as u64;
    let _ = events.send(Event::AssetProgress { id: id.clone(), done, total: inc.size });
    if done >= inc.size {
        let inc = incoming.remove(&id)?;
        let bytes = Arc::new(inc.data);
        let _ = events.send(Event::Asset { id, name: inc.name.clone(), bytes: bytes.clone() });
        return Some((inc.name, bytes));
    }
    None
}

fn host_hub(me: PeerInfo, rx: Receiver<Hub>, events: Sender<Event>) {
    let mut seq: u64 = 0;
    let mut log: Vec<(u64, PeerId, Transaction)> = Vec::new();
    let mut clients: HashMap<PeerId, Client> = HashMap::new();
    let mut incoming: HashMap<String, Incoming> = HashMap::new();
    let mut my_status = me.status.clone();
    let peers = |clients: &HashMap<PeerId, Client>, status: &str| -> Vec<PeerInfo> {
        let mut v = vec![PeerInfo { status: status.to_string(), ..me.clone() }];
        let mut others: Vec<PeerInfo> = clients.values().filter(|c| c.active).map(|c| c.info.clone()).collect();
        others.sort_by_key(|p| p.id);
        v.extend(others);
        v
    };
    let broadcast = |clients: &HashMap<PeerId, Client>, f: &Frame, except: Option<PeerId>| {
        for (id, c) in clients {
            if c.active && Some(*id) != except {
                let _ = c.out.send(f.clone());
            }
        }
    };
    for m in rx {
        match m {
            Hub::Propose(tx) => {
                seq += 1;
                let id = tx.id;
                log.push((seq, HOST_ID, tx.clone()));
                broadcast(&clients, &Frame::Msg(Msg::Commit { seq, author: HOST_ID, tx }), None);
                let _ = events.send(Event::Ack { seq, tx: id });
            }
            Hub::Welcome { peer, project, seq: at } => {
                if let Some(c) = clients.get_mut(&peer) {
                    let _ = c.out.send(Frame::Msg(Msg::Welcome { you: peer, project, peers: Vec::new(), seq: at }));
                    for (s, a, tx) in log.iter().filter(|(s, _, _)| *s > at) {
                        let _ = c.out.send(Frame::Msg(Msg::Commit { seq: *s, author: *a, tx: tx.clone() }));
                    }
                    c.active = true;
                    let info = c.info.clone();
                    let list = peers(&clients, &my_status);
                    broadcast(&clients, &Frame::Msg(Msg::Peers { peers: list.clone() }), None);
                    let _ = events.send(Event::PeerJoined(info));
                    let _ = events.send(Event::Peers(list));
                }
            }
            Hub::Reset { project } => {
                broadcast(&clients, &Frame::Msg(Msg::Reset { project, seq }), None);
            }
            Hub::Chat(text) => {
                broadcast(&clients, &Frame::Msg(Msg::Chat { from: HOST_ID, name: me.name.clone(), text }), None);
            }
            Hub::Transport(msg) => {
                broadcast(&clients, &Frame::Msg(Msg::Transport { from: HOST_ID, msg: msg.clone() }), None);
                let delay = match &msg {
                    TransportMsg::Play { at_ms, .. } => at_ms - now_ms(),
                    _ => 0.0,
                };
                let _ = events.send(Event::Transport { from: HOST_ID, msg, delay_ms: delay });
            }
            Hub::Status(s) => {
                my_status = s;
                let list = peers(&clients, &my_status);
                broadcast(&clients, &Frame::Msg(Msg::Peers { peers: list.clone() }), None);
                let _ = events.send(Event::Peers(list));
            }
            Hub::ShareAsset { id, name, bytes, to } => match to {
                Some(p) => {
                    if let Some(c) = clients.get(&p) {
                        send_asset(&c.out, &id, &name, &bytes);
                    }
                }
                None => {
                    for c in clients.values().filter(|c| c.active) {
                        send_asset(&c.out, &id, &name, &bytes);
                    }
                }
            },
            Hub::RequestAsset(_) => {}
            Hub::Leave => {
                broadcast(&clients, &Frame::Msg(Msg::Bye), None);
                break;
            }
            Hub::Joined { id, info, out } => {
                clients.insert(id, Client { info, out, active: false });
                let _ = events.send(Event::WantsProject { peer: id });
            }
            Hub::Left { id } => {
                if let Some(c) = clients.remove(&id) {
                    let list = peers(&clients, &my_status);
                    broadcast(&clients, &Frame::Msg(Msg::Peers { peers: list.clone() }), None);
                    let _ = events.send(Event::PeerLeft { id, name: c.info.name });
                    let _ = events.send(Event::Peers(list));
                }
            }
            Hub::Frame { from, frame } => match frame {
                Frame::Msg(Msg::Propose { tx }) => {
                    seq += 1;
                    log.push((seq, from, tx.clone()));
                    broadcast(&clients, &Frame::Msg(Msg::Commit { seq, author: from, tx: tx.clone() }), None);
                    let _ = events.send(Event::Remote { seq, author: from, tx });
                }
                Frame::Msg(Msg::Chat { text, .. }) => {
                    let name = clients.get(&from).map(|c| c.info.name.clone()).unwrap_or_default();
                    broadcast(&clients, &Frame::Msg(Msg::Chat { from, name: name.clone(), text: text.clone() }), Some(from));
                    let _ = events.send(Event::Chat { from, name, text });
                }
                Frame::Msg(Msg::Transport { msg, .. }) => {
                    broadcast(&clients, &Frame::Msg(Msg::Transport { from, msg: msg.clone() }), Some(from));
                    let delay = match &msg {
                        TransportMsg::Play { at_ms, .. } => at_ms - now_ms(),
                        _ => 0.0,
                    };
                    let _ = events.send(Event::Transport { from, msg, delay_ms: delay });
                }
                Frame::Msg(Msg::Status { status }) => {
                    if let Some(c) = clients.get_mut(&from) {
                        c.info.status = status;
                    }
                    let list = peers(&clients, &my_status);
                    broadcast(&clients, &Frame::Msg(Msg::Peers { peers: list.clone() }), None);
                    let _ = events.send(Event::Peers(list));
                }
                Frame::Msg(Msg::Ping { t0 }) => {
                    if let Some(c) = clients.get(&from) {
                        let _ = c.out.send(Frame::Msg(Msg::Pong { t0, host_ms: now_ms() }));
                    }
                }
                Frame::Msg(Msg::AssetRequest { id }) => {
                    let _ = events.send(Event::WantsAsset { peer: from, id });
                }
                Frame::Msg(Msg::AssetBegin { id, name, size }) => {
                    // relay to the others as it comes, and keep a copy
                    for (cid, c) in &clients {
                        if *cid != from && c.active {
                            let _ = c.out.send(Frame::Msg(Msg::AssetBegin { id: id.clone(), name: name.clone(), size }));
                        }
                    }
                    incoming.insert(id, Incoming { name, size, data: Vec::with_capacity(size as usize) });
                }
                Frame::Chunk { id, offset, data } => {
                    for (cid, c) in &clients {
                        if *cid != from && c.active {
                            let _ = c.out.send(Frame::Chunk { id: id.clone(), offset, data: data.clone() });
                        }
                    }
                    take_chunk(&mut incoming, &events, id, offset, data);
                }
                Frame::Msg(Msg::Bye) => {
                    if let Some(c) = clients.remove(&from) {
                        let _ = events.send(Event::PeerLeft { id: from, name: c.info.name });
                        let list = peers(&clients, &my_status);
                        broadcast(&clients, &Frame::Msg(Msg::Peers { peers: list.clone() }), None);
                        let _ = events.send(Event::Peers(list));
                    }
                }
                _ => {}
            },
        }
    }
}

fn client_hub(me: PeerId, rx: Receiver<Hub>, events: Sender<Event>, out: Sender<Frame>, offset: Arc<Mutex<f64>>) {
    let mut incoming: HashMap<String, Incoming> = HashMap::new();
    let mut best_rtt = f64::MAX;
    let mut samples = 0u32;
    for m in rx {
        match m {
            Hub::Propose(tx) => {
                let _ = out.send(Frame::Msg(Msg::Propose { tx }));
            }
            Hub::Chat(text) => {
                let _ = out.send(Frame::Msg(Msg::Chat { from: me, name: String::new(), text }));
            }
            Hub::Transport(msg) => {
                let _ = out.send(Frame::Msg(Msg::Transport { from: me, msg: msg.clone() }));
                let delay = match &msg {
                    TransportMsg::Play { at_ms, .. } => at_ms - *offset.lock() - now_ms(),
                    _ => 0.0,
                };
                let _ = events.send(Event::Transport { from: me, msg, delay_ms: delay });
            }
            Hub::Status(status) => {
                let _ = out.send(Frame::Msg(Msg::Status { status }));
            }
            Hub::ShareAsset { id, name, bytes, .. } => send_asset(&out, &id, &name, &bytes),
            Hub::RequestAsset(id) => {
                let _ = out.send(Frame::Msg(Msg::AssetRequest { id }));
            }
            Hub::Leave => {
                let _ = out.send(Frame::Msg(Msg::Bye));
                break;
            }
            Hub::Welcome { .. } | Hub::Reset { .. } | Hub::Joined { .. } => {}
            Hub::Left { .. } => {
                let _ = events.send(Event::Disconnected { reason: "l'hôte a quitté la session".into() });
                break;
            }
            Hub::Frame { frame, .. } => match frame {
                Frame::Msg(Msg::Welcome { you, project, seq, .. }) => {
                    let _ = events.send(Event::Welcome { you, project, seq });
                }
                Frame::Msg(Msg::Commit { seq, author, tx }) => {
                    if author == me {
                        let _ = events.send(Event::Ack { seq, tx: tx.id });
                    } else {
                        let _ = events.send(Event::Remote { seq, author, tx });
                    }
                }
                Frame::Msg(Msg::Reset { project, seq }) => {
                    let _ = events.send(Event::Reset { project, seq });
                }
                Frame::Msg(Msg::Peers { peers }) => {
                    let _ = events.send(Event::Peers(peers));
                }
                Frame::Msg(Msg::Chat { from, name, text }) => {
                    let _ = events.send(Event::Chat { from, name, text });
                }
                Frame::Msg(Msg::Transport { from, msg }) => {
                    let delay = match &msg {
                        TransportMsg::Play { at_ms, .. } => at_ms - *offset.lock() - now_ms(),
                        _ => 0.0,
                    };
                    let _ = events.send(Event::Transport { from, msg, delay_ms: delay });
                }
                Frame::Msg(Msg::Pong { t0, host_ms }) => {
                    let t1 = now_ms();
                    let rtt = t1 - t0;
                    samples += 1;
                    // keep the estimate of the fastest exchange (least queuing)
                    if rtt <= best_rtt * 1.2 || samples % 15 == 0 {
                        best_rtt = best_rtt.min(rtt);
                        *offset.lock() = host_ms - (t0 + t1) / 2.0;
                    }
                }
                Frame::Msg(Msg::AssetBegin { id, name, size }) => {
                    incoming.insert(id, Incoming { name, size, data: Vec::with_capacity(size as usize) });
                }
                Frame::Chunk { id, offset: off, data } => {
                    take_chunk(&mut incoming, &events, id, off, data);
                }
                Frame::Msg(Msg::Bye) => {
                    let _ = events.send(Event::Disconnected { reason: "l'hôte a fermé la session".into() });
                    break;
                }
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::model::*;
    use studio_core::ops::{Op, TrackField};

    fn wait_for<F: Fn(&Event) -> bool>(s: &Session, f: F) -> Event {
        let start = std::time::Instant::now();
        loop {
            for e in s.poll() {
                if f(&e) {
                    return e;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn host_and_join_over_loopback() {
        let host = Session::host("Sam", 0, false).unwrap();
        let code = host.invite.clone().unwrap();
        let mut project = Project::new("Morceau");
        project.tracks.push(Track::new("Instru", TrackKind::Instrumental));
        let tid = project.tracks[0].id;

        let joiner = std::thread::spawn(move || Session::join(&code, "Léa").unwrap());
        let Event::WantsProject { peer } = wait_for(&host, |e| matches!(e, Event::WantsProject { .. })) else { unreachable!() };
        host.welcome(peer, project.clone(), 0);
        let client = joiner.join().unwrap();
        let Event::Welcome { project: p2, .. } = wait_for(&client, |e| matches!(e, Event::Welcome { .. })) else { unreachable!() };
        assert_eq!(*p2, project);

        // a change by the client reaches the host in order
        let tx = Transaction::new("vol", Actor::Peer("Léa".into()), vec![Op::SetTrack { id: tid, field: TrackField::VolumeDb(-4.0) }]);
        client.propose(tx.clone());
        let Event::Remote { seq, tx: got, .. } = wait_for(&host, |e| matches!(e, Event::Remote { .. })) else { unreachable!() };
        assert_eq!(seq, 1);
        assert_eq!(got, tx);
        let Event::Ack { seq, .. } = wait_for(&client, |e| matches!(e, Event::Ack { .. })) else { unreachable!() };
        assert_eq!(seq, 1);

        // chat, assets, transport
        host.chat("salut");
        let Event::Chat { text, name, .. } = wait_for(&client, |e| matches!(e, Event::Chat { .. })) else { unreachable!() };
        assert_eq!((text.as_str(), name.as_str()), ("salut", "Sam"));
        let bytes: Vec<u8> = (0..600_000u32).map(|i| (i % 251) as u8).collect();
        client.share_asset("take1", "Prise 1.wav", Arc::new(bytes.clone()));
        let Event::Asset { bytes: got, name, .. } = wait_for(&host, |e| matches!(e, Event::Asset { .. })) else { unreachable!() };
        assert_eq!(name, "Prise 1.wav");
        assert_eq!(*got, bytes);
        host.transport(TransportMsg::Play { pos: 12.0, at_ms: 0.0 });
        let Event::Transport { msg: TransportMsg::Play { pos, .. }, delay_ms, .. } =
            wait_for(&client, |e| matches!(e, Event::Transport { .. }))
        else {
            unreachable!()
        };
        assert_eq!(pos, 12.0);
        assert!(delay_ms > 50.0 && delay_ms < 400.0, "{delay_ms}");
        client.leave();
        wait_for(&host, |e| matches!(e, Event::PeerLeft { .. }));
    }

    #[test]
    fn wrong_secret_is_refused() {
        let host = Session::host("Sam", 0, false).unwrap();
        let code = host.invite.clone().unwrap();
        let (addrs, _) = code.rsplit_once('-').unwrap();
        let bad = format!("{addrs}-AAAAAAAAAAAAAAAA");
        assert!(Session::join(&bad, "Intrus").is_err());
    }
}
