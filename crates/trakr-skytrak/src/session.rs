//! The `LaunchMonitor` driver: connects to one box and runs the connect /
//! handshake / arm state machine documented in
//! docs/skytrak-protocol/session.md. Confirmed end to end against a real
//! original SkyTrak unit on 2026-09-08 (discovery, handshake, arm, disarm).
//!
//! Shot capture is not yet implemented: the original SkyTrak sends encrypted
//! camera images rather than finished numbers (docs/skytrak-protocol/shot-data.md),
//! and decoding those into a `Shot` is a separate, larger piece of work. Until
//! then a real ball strike is reported as `Event::Misread` so callers know
//! something happened rather than silently seeing nothing.

use std::net::Ipv4Addr;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;
use tracing::{debug, info, warn};
use trakr_core::{
    Capabilities, Command, ConnectionKind, DeviceInfo, DeviceStatus, Error, Event, Handedness,
    LaunchMonitor, Result, ShotMode,
};

use crate::discovery::{discover, DiscoveredBox};
use crate::wire::{
    arm_packet, cam_config, connection_confirm, disarm_packet, magic_name, parse_params,
    parse_status, sys_config, Framer, StatusPacket, MAGIC_PARAMS, MAGIC_SHOT_HEADER, MAGIC_STATUS,
    MAGIC_TRIGGER, TCP_PORT, UDP_PORT,
};

pub const KIND: &str = "skytrak";

pub const CAPABILITIES: Capabilities = Capabilities {
    ball_speed: true,
    launch_angles: true,
    spin: true,
    club_data: false,
    putting_mode: true,
    handedness: true,
    arm_disarm: true,
};

enum Target {
    Known {
        name: String,
        address: Ipv4Addr,
    },
    Discover {
        broadcast_addrs: Vec<Ipv4Addr>,
        window: Duration,
    },
}

pub struct SkytrakDriver {
    target: Target,
    right_handed: bool,
    mode: ShotMode,
    /// The box has no native chipping mode (see
    /// docs/skytrak-protocol/chipping-mode.md). When `true` (the vendor
    /// connector's shipped default), `ShotMode::Chipping` arms the box's
    /// Putting mode; when `false`, it arms Normal mode instead.
    chip_via_putting: bool,
}

impl SkytrakDriver {
    /// Connect to a specific, already-known box (e.g. from a prior `discover`
    /// call, or one the caller typed in).
    pub fn connect_to(name: impl Into<String>, address: Ipv4Addr) -> Self {
        Self {
            target: Target::Known {
                name: name.into(),
                address,
            },
            right_handed: true,
            mode: ShotMode::Normal,
            chip_via_putting: true,
        }
    }

    /// Discover the first box that answers and connect to it.
    pub fn discover_and_connect(broadcast_addrs: Vec<Ipv4Addr>, window: Duration) -> Self {
        Self {
            target: Target::Discover {
                broadcast_addrs,
                window,
            },
            right_handed: true,
            mode: ShotMode::Normal,
            chip_via_putting: true,
        }
    }

    pub fn with_handedness(mut self, right_handed: bool) -> Self {
        self.right_handed = right_handed;
        self
    }

    /// See `chip_via_putting` field docs.
    pub fn with_chip_via_putting(mut self, chip_via_putting: bool) -> Self {
        self.chip_via_putting = chip_via_putting;
        self
    }

    /// Whether the box's hardware Putting-mode bit should be set for the
    /// driver's current logical `ShotMode`.
    fn hw_putting_bit(&self) -> bool {
        match self.mode {
            ShotMode::Normal => false,
            ShotMode::Putting => true,
            ShotMode::Chipping => self.chip_via_putting,
        }
    }
}

#[async_trait]
impl LaunchMonitor for SkytrakDriver {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    async fn run(
        mut self: Box<Self>,
        events: broadcast::Sender<Event>,
        mut commands: mpsc::Receiver<Command>,
    ) -> Result<()> {
        loop {
            let (name, address) = match self.resolve(&events).await {
                Some(v) => v,
                None => return Ok(()), // told to disconnect while still discovering
            };
            match self
                .connect_and_serve(&name, address, &events, &mut commands)
                .await
            {
                Ok(true) => return Ok(()), // Command::Disconnect
                Ok(false) => {}            // link dropped; loop and reconnect
                Err(e) => {
                    events
                        .send(Event::Error {
                            message: e.to_string(),
                        })
                        .ok();
                }
            }
            events
                .send(Event::Disconnected {
                    reason: "link lost, reconnecting".into(),
                })
                .ok();
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

impl SkytrakDriver {
    async fn resolve(&self, _events: &broadcast::Sender<Event>) -> Option<(String, Ipv4Addr)> {
        match &self.target {
            Target::Known { name, address } => Some((name.clone(), *address)),
            Target::Discover {
                broadcast_addrs,
                window,
            } => loop {
                match discover(*window, broadcast_addrs).await {
                    Ok(boxes) => {
                        if let Some(DiscoveredBox { name, address, .. }) = boxes.into_iter().next()
                        {
                            return Some((name, address));
                        }
                    }
                    Err(e) => warn!(error = %e, "discovery failed"),
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            },
        }
    }

    /// Returns `Ok(true)` if the caller asked us to disconnect for good,
    /// `Ok(false)` if the link simply dropped and we should retry.
    async fn connect_and_serve(
        &mut self,
        name: &str,
        address: Ipv4Addr,
        events: &broadcast::Sender<Event>,
        commands: &mut mpsc::Receiver<Command>,
    ) -> Result<bool> {
        confirm_udp(name, address).await?;
        tokio::time::sleep(Duration::from_millis(1000)).await; // matches the vendor SDK's pacing

        let mut stream = TcpStream::connect((address, TCP_PORT)).await?;
        stream.set_nodelay(true).ok();
        info!(name, %address, "connected");
        let device = DeviceInfo {
            kind: KIND,
            name: name.to_string(),
            address: Some(address.to_string()),
            connection: ConnectionKind::WifiNetwork,
            firmware: None,
        };
        events.send(Event::Connected(device)).ok();

        let mut framer = Framer::default();
        let mut buf = [0u8; 65536];
        let mut first_status = true;
        let mut armed = false;
        let mut last_status: Option<StatusPacket> = None;

        loop {
            tokio::select! {
                cmd = commands.recv() => {
                    match cmd {
                        None => return Ok(true),
                        Some(Command::Disconnect) => return Ok(true),
                        Some(Command::Arm) => {
                            send(&mut stream, "ARM", &arm_packet()).await?;
                            armed = true;
                            events.send(Event::Status(build_status(armed, self.right_handed, self.mode, &last_status))).ok();
                        }
                        Some(Command::Disarm) => {
                            send(&mut stream, "DISARM", &disarm_packet()).await?;
                            armed = false;
                            events.send(Event::Status(build_status(armed, self.right_handed, self.mode, &last_status))).ok();
                        }
                        Some(Command::SetHandedness(h)) => {
                            self.right_handed = matches!(h, Handedness::Right);
                            send(&mut stream, "SYS_CONFIG(hand)", &sys_config(false, self.right_handed, self.hw_putting_bit(), false)).await?;
                            send(&mut stream, "CAM_CONFIG(hand)", &cam_config(self.right_handed)).await?;
                        }
                        Some(Command::SetShotMode(m)) => {
                            self.mode = m;
                            send(&mut stream, "SYS_CONFIG(mode)", &sys_config(false, self.right_handed, self.hw_putting_bit(), false)).await?;
                            events.send(Event::Status(build_status(armed, self.right_handed, self.mode, &last_status))).ok();
                        }
                    }
                }
                n = read(&mut stream, &mut buf) => {
                    let n = n?;
                    if n == 0 { return Ok(false); }
                    for (magic, pkt) in framer.feed(&buf[..n]) {
                        match magic {
                            MAGIC_STATUS => {
                                let Some(status) = parse_status(&pkt) else { continue };
                                last_status = Some(status.clone());
                                events.send(Event::Status(DeviceStatus {
                                    battery_pct: Some(status.battery_pct),
                                    charging: Some(status.charging),
                                    rssi: status.rssi,
                                    handedness: Some(if status.handedness_right { Handedness::Right } else { Handedness::Left }),
                                    shot_mode: Some(self.mode),
                                    armed,
                                    roll_deg: None,
                                    tilt_deg: None,
                                })).ok();
                                if first_status {
                                    first_status = false;
                                    send(&mut stream, "DISARM", &disarm_packet()).await?;
                                    send(&mut stream, "SYS_CONFIG(initial)", &sys_config(true, self.right_handed, self.hw_putting_bit(), false)).await?;
                                } else {
                                    match status.code {
                                        1 => send(&mut stream, "SYS_CONFIG", &sys_config(false, self.right_handed, self.hw_putting_bit(), false)).await?,
                                        2 => send(&mut stream, "CAM_CONFIG", &cam_config(self.right_handed)).await?,
                                        3 => send(&mut stream, "ARM", &arm_packet()).await?,
                                        0 if !armed => { armed = true; events.send(Event::Ready).ok(); }
                                        code if code < 0 => warn!(code, name = box_error_name(code), "box reported an error"),
                                        _ => {}
                                    }
                                }
                            }
                            MAGIC_PARAMS => {
                                if let Some(params) = parse_params(&pkt) {
                                    debug!(fw = params.firmware_version, serial = %params.serial, "box params");
                                    events.send(Event::Connected(DeviceInfo {
                                        kind: KIND, name: name.to_string(), address: Some(address.to_string()),
                                        connection: if params.ap_mode { ConnectionKind::WifiDirect } else { ConnectionKind::WifiNetwork },
                                        firmware: Some(format!("{:.4}", params.firmware_version)),
                                    })).ok();
                                }
                                send(&mut stream, "SYS_CONFIG(post-params)", &sys_config(false, self.right_handed, self.hw_putting_bit(), false)).await?;
                                send(&mut stream, "CAM_CONFIG(post-params)", &cam_config(self.right_handed)).await?;
                            }
                            MAGIC_TRIGGER => {
                                events.send(Event::ShotStarted).ok();
                            }
                            MAGIC_SHOT_HEADER => {
                                // Ball detected, but decoding camera images into a Shot
                                // is not implemented yet (docs/skytrak-protocol/shot-data.md).
                                events.send(Event::Misread { reason: "shot image decode not yet implemented".into() }).ok();
                                armed = false;
                            }
                            _ => debug!(magic = magic_name(magic), len = pkt.len(), "unhandled packet"),
                        }
                    }
                }
            }
        }
    }
}

/// Builds a `DeviceStatus` for synthetic updates (arm/disarm commands) using
/// the last real status packet for fields we don't track locally (battery, rssi).
fn build_status(
    armed: bool,
    right_handed: bool,
    mode: ShotMode,
    last: &Option<StatusPacket>,
) -> DeviceStatus {
    DeviceStatus {
        battery_pct: last.as_ref().map(|s| s.battery_pct),
        charging: last.as_ref().map(|s| s.charging),
        rssi: last.as_ref().and_then(|s| s.rssi),
        handedness: Some(if right_handed {
            Handedness::Right
        } else {
            Handedness::Left
        }),
        shot_mode: Some(mode),
        armed,
        roll_deg: None,
        tilt_deg: None,
    }
}

async fn confirm_udp(expected_name: &str, address: Ipv4Addr) -> Result<()> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).await?;
    let req = connection_confirm();
    for attempt in 0..5 {
        sock.send_to(&req, (address, UDP_PORT)).await?;
        let mut buf = [0u8; 0x4000];
        match timeout(Duration::from_secs(4), sock.recv_from(&mut buf)).await {
            Ok(Ok((n, _))) => {
                if let Some(status) = parse_status(&buf[..n]) {
                    if status.box_name == expected_name {
                        return Ok(());
                    }
                    warn!(got = %status.box_name, want = expected_name, "confirm name mismatch, retrying");
                }
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => debug!(attempt, "UDP confirm timeout, retrying"),
        }
    }
    Err(Error::Transport(format!(
        "could not confirm {expected_name} over UDP"
    )))
}

async fn send(stream: &mut TcpStream, label: &str, pkt: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    debug!(label, len = pkt.len(), "sending");
    stream.write_all(pkt).await?;
    Ok(())
}

async fn read(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<usize> {
    use tokio::io::AsyncReadExt;
    stream.read(buf).await
}

fn box_error_name(code: i32) -> &'static str {
    match code {
        -1 => "RESP_ERR_IMGTRANSFER_TIMEOUT",
        -2 => "RESP_ERR_FPGATRIGGER_TIMEOUT",
        -3 => "WIFI_RX_PACKET_HEADER_ERROR",
        -4 => "WIFI_RX_PACKET_SIZE_ERROR",
        -10 => "SYS_CONFIG_PKT_CRC_ERROR",
        -11 => "CAM_CONFIG_PKT_CRC_ERROR",
        -12 => "HOST_READY_PKT_CRC_ERROR",
        -16 => "APP_BOX_BAD_POSITION_ERROR",
        -17 => "APP_POWER_CRITICAL_ERROR",
        -18 => "APP_LASER_SAFETY_ALARM_ERROR",
        -43 => "ENCRYPTION_INIT_FAILURE",
        -44 => "ENCRYPTION_DATA_FAILURE",
        _ => "UNKNOWN",
    }
}
