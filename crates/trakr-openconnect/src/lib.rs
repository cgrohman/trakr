//! GSPro Open Connect v1 output. Muni Golf Sim, GSPro, and OpenGolfAPI all
//! accept this. We act as the *launch monitor client*: connect to the sim's
//! listener (default 127.0.0.1:921), send heartbeats and shots as JSON, and
//! read player-info responses (handedness, club, distance to target) which we
//! turn into driver commands.

pub mod wire;

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, info, warn};
use trakr_core::{ChipSettings, Command, Event, Handedness, ShotMode};
use wire::{Request, Response, ShotDataOptions};

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub device_id: String,
    pub heartbeat: Duration,
    pub reconnect_delay: Duration,
    /// Derive shot mode from the sim's reported club/distance-to-target
    /// (putter -> Putting, everything else per the current [`ChipSettings`]).
    /// If false, shot mode is only ever changed by an explicit
    /// `SetShotMode` command.
    pub auto_shot_mode: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 921,
            device_id: "trakr".into(),
            heartbeat: Duration::from_secs(2),
            reconnect_delay: Duration::from_secs(5),
            auto_shot_mode: true,
        }
    }
}

/// Chooses shot mode from the sim's reported club and distance to target,
/// reproducing the original vendor connector's net behaviour (see
/// docs/skytrak-protocol/chipping-mode.md): putter always wins; a configured
/// distance threshold overrides club selection entirely; otherwise a lob
/// wedge triggers chipping if enabled; everything else is Normal.
fn resolve_shot_mode(chip: &ChipSettings, club: &str, distance_to_target: Option<f32>) -> ShotMode {
    if club == "PT" {
        return ShotMode::Putting;
    }
    if let (Some(threshold), Some(dist)) = (chip.force_chip_distance_yd, distance_to_target) {
        if threshold > 0.0 {
            return if dist <= threshold {
                ShotMode::Chipping
            } else {
                ShotMode::Normal
            };
        }
    }
    if chip.chip_on_lob_wedge && club == "LW" {
        return ShotMode::Chipping;
    }
    ShotMode::Normal
}

/// Run the output until the event bus closes. Reconnects to the sim forever.
pub async fn run(
    cfg: Config,
    chip_settings: watch::Receiver<ChipSettings>,
    mut events: broadcast::Receiver<Event>,
    commands: mpsc::Sender<Command>,
) -> anyhow::Result<()> {
    let mut shot_number: u32 = 0;
    let mut ready = false;
    loop {
        let addr = format!("{}:{}", cfg.host, cfg.port);
        let mut stream = match TcpStream::connect(&addr).await {
            Ok(s) => {
                info!(%addr, "connected to sim");
                s
            }
            Err(e) => {
                warn!(%addr, error = %e, "sim not reachable, retrying");
                tokio::time::sleep(cfg.reconnect_delay).await;
                continue;
            }
        };
        stream.set_nodelay(true)?;
        if let Err(e) = send(
            &mut stream,
            &Request::heartbeat(&cfg.device_id, shot_number, ready),
        )
        .await
        {
            warn!(error = %e, "initial heartbeat failed");
            continue;
        }
        let mut buf = vec![0u8; 8192];
        let mut acc: Vec<u8> = Vec::new();
        let mut tick = tokio::time::interval(cfg.heartbeat);
        let outcome: anyhow::Result<()> = async {
            loop {
                tokio::select! {
                    ev = events.recv() => {
                        let ev = match ev {
                            Ok(ev) => ev,
                            Err(broadcast::error::RecvError::Lagged(n)) => { warn!(n, "event bus lagged"); continue; }
                            Err(broadcast::error::RecvError::Closed) => return Ok(()),
                        };
                        match ev {
                            Event::Ready => { ready = true; send(&mut stream, &Request::heartbeat(&cfg.device_id, shot_number, true)).await?; }
                            Event::Disconnected { .. } | Event::ShotStarted => { ready = false; }
                            Event::Shot(shot) if shot.valid => {
                                shot_number += 1;
                                let req = Request::from_shot(&cfg.device_id, shot_number, &shot);
                                debug!(?req, "sending shot");
                                send(&mut stream, &req).await?;
                            }
                            _ => {}
                        }
                    }
                    _ = tick.tick() => {
                        send(&mut stream, &Request::heartbeat(&cfg.device_id, shot_number, ready)).await?;
                    }
                    n = stream.read(&mut buf) => {
                        let n = n?;
                        if n == 0 { anyhow::bail!("sim closed connection"); }
                        acc.extend_from_slice(&buf[..n]);
                        for resp in wire::drain_responses(&mut acc) {
                            handle_response(&cfg, &resp, &commands, &chip_settings).await;
                        }
                    }
                }
            }
        }
        .await;
        match outcome {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(error = %e, "sim link dropped, reconnecting");
                tokio::time::sleep(cfg.reconnect_delay).await;
            }
        }
    }
}

async fn send(stream: &mut TcpStream, req: &Request) -> anyhow::Result<()> {
    let body = serde_json::to_vec(req)?;
    stream.write_all(&body).await?;
    Ok(())
}

async fn handle_response(
    cfg: &Config,
    resp: &Response,
    commands: &mpsc::Sender<Command>,
    chip_settings: &watch::Receiver<ChipSettings>,
) {
    debug!(?resp, "sim response");
    if resp.code == 201 {
        if let Some(p) = &resp.player {
            match p.handed.as_deref() {
                Some("RH") => {
                    let _ = commands
                        .send(Command::SetHandedness(Handedness::Right))
                        .await;
                }
                Some("LH") => {
                    let _ = commands
                        .send(Command::SetHandedness(Handedness::Left))
                        .await;
                }
                _ => {}
            }
            if cfg.auto_shot_mode {
                if let Some(club) = &p.club {
                    // .borrow() gives the latest value the settings API has
                    // pushed, even mid-session -- no reconnect needed.
                    let chip = *chip_settings.borrow();
                    let mode = resolve_shot_mode(&chip, club, p.distance_to_target);
                    let _ = commands.send(Command::SetShotMode(mode)).await;
                }
            }
        }
    } else if resp.code >= 500 {
        warn!(
            code = resp.code,
            msg = resp.message.as_deref().unwrap_or(""),
            "sim rejected message"
        );
    }
}

impl Request {
    pub fn heartbeat(device_id: &str, shot_number: u32, ready: bool) -> Self {
        Request {
            device_id: device_id.into(),
            units: "Yards".into(),
            shot_number,
            api_version: "1".into(),
            ball_data: None,
            club_data: None,
            shot_data_options: ShotDataOptions {
                contains_ball_data: false,
                contains_club_data: false,
                launch_monitor_is_ready: Some(ready),
                launch_monitor_ball_detected: None,
                is_heart_beat: Some(true),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(chip_on_lob_wedge: bool, force_chip_distance_yd: Option<f32>) -> ChipSettings {
        ChipSettings {
            chip_on_lob_wedge,
            force_chip_distance_yd,
            chip_via_putting: true,
        }
    }

    #[test]
    fn putter_always_wins() {
        let c = cfg(true, Some(20.0));
        assert_eq!(resolve_shot_mode(&c, "PT", Some(5.0)), ShotMode::Putting);
        let c = cfg(false, None);
        assert_eq!(resolve_shot_mode(&c, "PT", None), ShotMode::Putting);
    }

    #[test]
    fn lob_wedge_triggers_chipping_by_default() {
        let c = cfg(true, None);
        assert_eq!(resolve_shot_mode(&c, "LW", None), ShotMode::Chipping);
        assert_eq!(resolve_shot_mode(&c, "DR", None), ShotMode::Normal);
    }

    #[test]
    fn lob_wedge_chipping_can_be_disabled() {
        let c = cfg(false, None);
        assert_eq!(resolve_shot_mode(&c, "LW", None), ShotMode::Normal);
    }

    #[test]
    fn distance_threshold_overrides_club_selection_entirely() {
        // Matches the vendor connector's ForceChipDistanceToTarget: distance
        // decides, even for a driver, and a lob wedge outside the range does
        // NOT chip just because it's a lob wedge.
        let c = cfg(true, Some(20.0));
        assert_eq!(resolve_shot_mode(&c, "DR", Some(15.0)), ShotMode::Chipping);
        assert_eq!(resolve_shot_mode(&c, "LW", Some(25.0)), ShotMode::Normal);
        assert_eq!(resolve_shot_mode(&c, "9I", Some(20.0)), ShotMode::Chipping);
        // exactly at threshold
    }

    #[test]
    fn zero_distance_threshold_means_disabled() {
        // Config::force_chip_distance_yd: Some(0.0) behaves like None, matching
        // the vendor's "0 = off" convention for ForceChipDistanceToTarget.
        let c = cfg(true, Some(0.0));
        assert_eq!(resolve_shot_mode(&c, "LW", Some(1.0)), ShotMode::Chipping); // falls through to lob-wedge rule
        assert_eq!(resolve_shot_mode(&c, "DR", Some(1.0)), ShotMode::Normal);
    }

    #[test]
    fn missing_distance_falls_back_to_club_rule() {
        let c = cfg(true, Some(20.0));
        assert_eq!(resolve_shot_mode(&c, "LW", None), ShotMode::Chipping);
        assert_eq!(resolve_shot_mode(&c, "DR", None), ShotMode::Normal);
    }
}
