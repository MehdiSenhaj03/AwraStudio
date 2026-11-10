//! Sessions on the local network: the host announces itself by UDP
//! broadcast (no secret in it); others list what they hear.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const DISCOVERY_PORT: u16 = 47801;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub host: String,
    pub addr: String,
    pub port: u16,
}

#[derive(Serialize, Deserialize)]
struct Beacon {
    awra: u32,
    host: String,
    port: u16,
}

/// Announces a hosted session every 2 s (for as long as the process runs
/// the session; stops when the port no longer accepts).
pub fn beacon(name: &str, port: u16) {
    let msg = serde_json::to_vec(&Beacon { awra: 1, host: name.to_string(), port }).unwrap_or_default();
    let _ = std::thread::Builder::new().name("awra-collab-beacon".into()).spawn(move || {
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return };
        let _ = sock.set_broadcast(true);
        loop {
            let _ = sock.send_to(&msg, ("255.255.255.255", DISCOVERY_PORT));
            std::thread::sleep(Duration::from_secs(2));
            // the session is gone when its port is free again
            if std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() {
                break;
            }
        }
    });
}

/// Listens for announcements for `wait`.
pub fn discover(wait: Duration) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let Ok(sock) = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) else { return out };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
    let start = Instant::now();
    let mut buf = [0u8; 1024];
    while start.elapsed() < wait {
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            if let Ok(b) = serde_json::from_slice::<Beacon>(&buf[..n]) {
                let f = Found { host: b.host, addr: from.ip().to_string(), port: b.port };
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
    }
    out
}
