//! Wire protocol: messages, encrypted framing, handshake.
//!
//! A connection starts with each side sending "AWRA1" and 32 random bytes;
//! both derive the connection key from the session secret and the two
//! random values (so no nonce is ever reused across connections). Every
//! frame after that is ChaCha20-Poly1305 encrypted with a per-direction
//! counter nonce; a wrong secret fails the first frame.

use std::io::{Read, Write};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use studio_core::model::Project;
use studio_core::ops::Transaction;

pub type PeerId = u32;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: PeerId,
    pub name: String,
    pub color: String,
    /// "idle", "listening", "recording", "writing"
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "t")]
pub enum TransportMsg {
    /// Start at `pos` (s) when the host's clock reads `at_ms`.
    Play { pos: f64, at_ms: f64 },
    Stop { pos: f64 },
    Seek { pos: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "m")]
pub enum Msg {
    Hello { name: String, color: String, version: u32 },
    Welcome { you: PeerId, project: Box<Project>, peers: Vec<PeerInfo>, seq: u64 },
    /// A participant's change (to the host).
    Propose { tx: Transaction },
    /// The host's ordered change (to everyone).
    Commit { seq: u64, author: PeerId, tx: Transaction },
    /// The host gives a whole project (a newly opened one).
    Reset { project: Box<Project>, seq: u64 },
    Peers { peers: Vec<PeerInfo> },
    Status { status: String },
    Chat { from: PeerId, name: String, text: String },
    Transport { from: PeerId, msg: TransportMsg },
    Ping { t0: f64 },
    Pong { t0: f64, host_ms: f64 },
    /// Asks for an asset's bytes.
    AssetRequest { id: String },
    /// Announces an asset about to be sent in chunks.
    AssetBegin { id: String, name: String, size: u64 },
    Bye,
}

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 8 << 20;
pub const CHUNK: usize = 256 << 10;

/// A frame's plaintext: a message, or a chunk of an asset.
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Msg(Msg),
    Chunk { id: String, offset: u64, data: Vec<u8> },
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Frame::Msg(m) => {
                let mut v = vec![0u8];
                v.extend(serde_json::to_vec(m).expect("message serializes"));
                v
            }
            Frame::Chunk { id, offset, data } => {
                let mut v = vec![1u8];
                v.extend((id.len() as u16).to_be_bytes());
                v.extend(id.as_bytes());
                v.extend(offset.to_be_bytes());
                v.extend(data);
                v
            }
        }
    }
    pub fn decode(b: &[u8]) -> Option<Frame> {
        match *b.first()? {
            0 => serde_json::from_slice(&b[1..]).ok().map(Frame::Msg),
            1 => {
                let n = u16::from_be_bytes([*b.get(1)?, *b.get(2)?]) as usize;
                let id = std::str::from_utf8(b.get(3..3 + n)?).ok()?.to_string();
                let off = u64::from_be_bytes(b.get(3 + n..11 + n)?.try_into().ok()?);
                Some(Frame::Chunk { id, offset: off, data: b[11 + n..].to_vec() })
            }
            _ => None,
        }
    }
}

/// The session's master key, from its secret.
pub fn master_key(secret: &str) -> [u8; 32] {
    blake3::derive_key("awra-studio collab v1 master", secret.trim().to_uppercase().as_bytes())
}

pub fn connection_key(master: &[u8; 32], client_rand: &[u8; 32], host_rand: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(master);
    h.update(client_rand);
    h.update(host_rand);
    *h.finalize().as_bytes()
}

pub struct Cipher {
    aead: ChaCha20Poly1305,
    /// 0: client → host, 1: host → client
    dir: u8,
    counter: u64,
}

impl Cipher {
    pub fn new(key: &[u8; 32], dir: u8) -> Cipher {
        Cipher { aead: ChaCha20Poly1305::new(Key::from_slice(key)), dir, counter: 0 }
    }
    fn nonce(&mut self) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[0] = self.dir;
        n[4..].copy_from_slice(&self.counter.to_le_bytes());
        self.counter += 1;
        n
    }
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let n = self.nonce();
        self.aead.encrypt(Nonce::from_slice(&n), plain).expect("encryption")
    }
    pub fn open(&mut self, sealed: &[u8]) -> Option<Vec<u8>> {
        let n = self.nonce();
        self.aead.decrypt(Nonce::from_slice(&n), sealed).ok()
    }
}

pub fn random32() -> [u8; 32] {
    let mut r = [0u8; 32];
    for b in r.iter_mut() {
        *b = fastrand::u8(..);
    }
    // mix in the clock and the process so that seeds differ across machines
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut h = blake3::Hasher::new();
    h.update(&r);
    h.update(&t.to_le_bytes());
    h.update(&std::process::id().to_le_bytes());
    *h.finalize().as_bytes()
}

pub fn write_frame(w: &mut impl Write, cipher: &mut Cipher, f: &Frame) -> std::io::Result<()> {
    let sealed = cipher.seal(&f.encode());
    w.write_all(&(sealed.len() as u32).to_be_bytes())?;
    w.write_all(&sealed)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read, cipher: &mut Cipher) -> std::io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME + 64 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "trame trop grande"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    let plain = cipher
        .open(&buf)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::PermissionDenied, "code de session incorrect"))?;
    Frame::decode(&plain).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "trame illisible"))
}

pub const MAGIC: &[u8; 5] = b"AWRA1";

/// Exchanges the greeting; returns the other side's random value.
pub fn greet(s: &mut (impl Read + Write), mine: &[u8; 32]) -> std::io::Result<[u8; 32]> {
    s.write_all(MAGIC)?;
    s.write_all(mine)?;
    s.flush()?;
    let mut magic = [0u8; 5];
    s.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "ce n'est pas une session Awra Studio"));
    }
    let mut theirs = [0u8; 32];
    s.read_exact(&mut theirs)?;
    Ok(theirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip_encrypted() {
        let k = connection_key(&master_key("ABCD"), &[1; 32], &[2; 32]);
        let mut a = Cipher::new(&k, 0);
        let mut b = Cipher::new(&k, 0);
        let mut wire = Vec::new();
        let f1 = Frame::Msg(Msg::Chat { from: 1, name: "Sam".into(), text: "yo".into() });
        let f2 = Frame::Chunk { id: "abc".into(), offset: 42, data: vec![1, 2, 3] };
        write_frame(&mut wire, &mut a, &f1).unwrap();
        write_frame(&mut wire, &mut a, &f2).unwrap();
        let mut r = &wire[..];
        assert_eq!(read_frame(&mut r, &mut b).unwrap(), f1);
        assert_eq!(read_frame(&mut r, &mut b).unwrap(), f2);
        // a wrong secret can't read
        let k2 = connection_key(&master_key("WRONG"), &[1; 32], &[2; 32]);
        let mut c = Cipher::new(&k2, 0);
        let mut r = &wire[..];
        assert!(read_frame(&mut r, &mut c).is_err());
    }
}
