//! Invitation codes: where to reach the host and the session secret, in
//! one string to paste ("AWRA-…"). Also local address discovery and the
//! optional UPnP port mapping that lets friends join over the internet
//! without any server.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for &b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn unbase32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.chars() {
        let c = c.to_ascii_uppercase();
        let c = match c {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        let v = ALPHABET.iter().position(|&a| a as char == c)? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// A fresh session secret (16 characters, 80 bits).
pub fn new_secret() -> String {
    let r = crate::proto::random32();
    (0..16).map(|i| ALPHABET[(r[i] & 31) as usize] as char).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Invite {
    pub addrs: Vec<SocketAddr>,
    pub secret: String,
}

impl Invite {
    pub fn encode(&self) -> String {
        let mut b = vec![1u8, self.addrs.len().min(8) as u8];
        for a in self.addrs.iter().take(8) {
            match a.ip() {
                IpAddr::V4(ip) => {
                    b.push(4);
                    b.extend(ip.octets());
                }
                IpAddr::V6(ip) => {
                    b.push(6);
                    b.extend(ip.octets());
                }
            }
            b.extend(a.port().to_be_bytes());
        }
        format!("AWRA-{}-{}", base32(&b), self.secret)
    }

    pub fn decode(code: &str) -> Option<Invite> {
        let code: String = code.trim().chars().filter(|c| !c.is_whitespace()).collect();
        let rest = code.strip_prefix("AWRA-").or_else(|| code.strip_prefix("awra-"))?;
        let (addr_part, secret) = rest.rsplit_once('-')?;
        let b = unbase32(addr_part)?;
        if b.first() != Some(&1) {
            return None;
        }
        let n = *b.get(1)? as usize;
        let mut i = 2;
        let mut addrs = Vec::new();
        for _ in 0..n {
            let ip = match *b.get(i)? {
                4 => {
                    let o: [u8; 4] = b.get(i + 1..i + 5)?.try_into().ok()?;
                    i += 5;
                    IpAddr::V4(Ipv4Addr::from(o))
                }
                6 => {
                    let o: [u8; 16] = b.get(i + 1..i + 17)?.try_into().ok()?;
                    i += 17;
                    IpAddr::V6(Ipv6Addr::from(o))
                }
                _ => return None,
            };
            let port = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]);
            i += 2;
            addrs.push(SocketAddr::new(ip, port));
        }
        if secret.len() < 8 {
            return None;
        }
        Some(Invite { addrs, secret: secret.to_uppercase() })
    }
}

/// The machine's main LAN address (no packet is sent: connecting a UDP
/// socket only picks the route).
pub fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    if ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}

/// Asks the router (UPnP IGD) to forward a TCP port; returns the public
/// address. Best effort: many routers disable UPnP.
pub fn upnp_map(port: u16) -> Option<SocketAddr> {
    let local = match local_ip()? {
        IpAddr::V4(ip) => SocketAddr::new(IpAddr::V4(ip), port),
        IpAddr::V6(_) => return None,
    };
    let opts = igd_next::SearchOptions { timeout: Some(std::time::Duration::from_secs(2)), ..Default::default() };
    let gw = igd_next::search_gateway(opts).ok()?;
    gw.add_port(igd_next::PortMappingProtocol::TCP, port, local, 6 * 3600, "Awra Studio").ok()?;
    let ext = gw.get_external_ip().ok()?;
    Some(SocketAddr::new(ext, port))
}

pub fn upnp_unmap(port: u16) {
    let opts = igd_next::SearchOptions { timeout: Some(std::time::Duration::from_secs(1)), ..Default::default() };
    if let Ok(gw) = igd_next::search_gateway(opts) {
        let _ = gw.remove_port(igd_next::PortMappingProtocol::TCP, port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_roundtrip() {
        let inv = Invite {
            addrs: vec!["192.168.1.20:47800".parse().unwrap(), "[2001:db8::1]:47800".parse().unwrap()],
            secret: new_secret(),
        };
        let code = inv.encode();
        assert!(code.starts_with("AWRA-"));
        assert_eq!(Invite::decode(&code).unwrap(), inv);
        assert_eq!(Invite::decode(&format!("  {} ", code.to_lowercase().replacen("awra-", "AWRA-", 1))).unwrap(), inv);
        assert!(Invite::decode("hello").is_none());
    }
}
