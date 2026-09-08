//! UDP broadcast discovery. Confirmed against a real unit — see
//! docs/skytrak-protocol/discovery.md.

use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::debug;

use crate::wire::{discovery_request, parse_status, UDP_PORT};

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredBox {
    pub name: String,
    pub address: Ipv4Addr,
    /// From the reply's mode field: 0 = network mode, 1 = direct (box-as-AP).
    pub direct_mode: bool,
}

/// Broadcast the discovery request and collect replies for `window`.
/// `broadcast_addrs` defaults to the global broadcast address; pass your
/// subnet's directed broadcast (e.g. `192.168.7.255` for a `/22`) if your
/// network doesn't forward `255.255.255.255` between interfaces.
pub async fn discover(
    window: Duration,
    broadcast_addrs: &[Ipv4Addr],
) -> std::io::Result<Vec<DiscoveredBox>> {
    let addrs: Vec<Ipv4Addr> = if broadcast_addrs.is_empty() {
        vec![Ipv4Addr::BROADCAST]
    } else {
        broadcast_addrs.to_vec()
    };
    let sock = UdpSocket::bind(("0.0.0.0", 0)).await?;
    sock.set_broadcast(true)?;
    let req = discovery_request();
    for a in &addrs {
        sock.send_to(&req, (*a, UDP_PORT)).await?;
    }

    let mut found: Vec<DiscoveredBox> = Vec::new();
    let mut buf = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                if let Some(status) = parse_status(&buf[..n]) {
                    let Some(ip) = ipv4_of(from) else { continue };
                    if found.iter().any(|b| b.address == ip) {
                        continue;
                    }
                    debug!(name = %status.box_name, %ip, "discovered box");
                    found.push(DiscoveredBox {
                        name: status.box_name,
                        address: ip,
                        direct_mode: status.connection_mode == 1,
                    });
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => break, // overall window elapsed
        }
    }
    Ok(found)
}

fn ipv4_of(addr: std::net::SocketAddr) -> Option<Ipv4Addr> {
    match addr {
        std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
        std::net::SocketAddr::V6(_) => None,
    }
}
