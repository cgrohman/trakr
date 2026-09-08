//! UDP broadcast discovery. Confirmed against a real unit — see
//! docs/skytrak-protocol/discovery.md.
//!
//! Like the vendor SDK (see discovery.md §5, "Adapter selection"), we
//! broadcast on every active local network's directed broadcast address
//! rather than relying on the global `255.255.255.255` address, which many
//! routers and OSes don't forward the way you'd hope. No manual "what's my
//! subnet" step required.

use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::wire::{discovery_request, parse_status, UDP_PORT};

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredBox {
    pub name: String,
    pub address: Ipv4Addr,
    /// From the reply's mode field: 0 = network mode, 1 = direct (box-as-AP).
    pub direct_mode: bool,
}

/// Directed broadcast address of every active, non-loopback, non-p2p IPv4
/// interface on this machine (e.g. `192.168.7.255` for a host at
/// `192.168.5.11/22`). This is what "scan the current network" means in
/// practice — a launch monitor on any interface this host has a live IP on
/// gets a chance to answer, WiFi or wired alike.
pub fn local_broadcast_addresses() -> Vec<Ipv4Addr> {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "could not enumerate network interfaces");
            return Vec::new();
        }
    };
    let mut addrs = Vec::new();
    for iface in interfaces {
        if iface.is_loopback() || iface.is_p2p() {
            continue;
        }
        if let if_addrs::IfAddr::V4(v4) = iface.addr {
            let broadcast = v4
                .broadcast
                .unwrap_or_else(|| compute_broadcast(v4.ip, v4.netmask));
            if !addrs.contains(&broadcast) {
                debug!(interface = %iface.name, ip = %v4.ip, %broadcast, "will scan");
                addrs.push(broadcast);
            }
        }
    }
    addrs
}

fn compute_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(ip) | !u32::from(netmask))
}

/// Broadcast the discovery request and collect replies for `window`.
/// `broadcast_addrs` is combined with every locally detected subnet's
/// directed broadcast address (see [`local_broadcast_addresses`]); pass an
/// explicit address here for a network this host isn't itself on, or to
/// force a specific address if auto-detection picks the wrong interface.
pub async fn discover(
    window: Duration,
    broadcast_addrs: &[Ipv4Addr],
) -> std::io::Result<Vec<DiscoveredBox>> {
    let mut addrs = local_broadcast_addresses();
    for a in broadcast_addrs {
        if !addrs.contains(a) {
            addrs.push(*a);
        }
    }
    if addrs.is_empty() {
        addrs.push(Ipv4Addr::BROADCAST);
    }
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
