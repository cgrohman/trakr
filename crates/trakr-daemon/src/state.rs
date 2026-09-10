use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use trakr_core::{
    ChipSettings, ClubCarry, Command, DeviceInfo, DeviceStatus, Event, Player, Shot,
    EVENT_BUS_CAPACITY,
};

use crate::{player_store, settings};

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub events: broadcast::Sender<Event>,
    pub session: RwLock<Option<Session>>,
    pub sim: trakr_openconnect::Config,
    /// Live, persisted chipping settings. `watch` because it's exactly
    /// "latest value, readable without consuming, updatable from elsewhere" --
    /// a running session's Open Connect output picks up a change on its very
    /// next message, no reconnect needed.
    pub chip_settings: watch::Sender<ChipSettings>,
    /// Persisted player roster. Reference data (see `trakr_core::player`):
    /// `POST /v1/session/shot` reads it to turn a club into plausible ball
    /// data, nothing else in the daemon depends on it.
    pub players: RwLock<Vec<Player>>,
}

pub struct Session {
    pub device: Option<DeviceInfo>,
    pub status: DeviceStatus,
    pub last_shot: Option<Shot>,
    pub commands: mpsc::Sender<Command>,
    driver_task: JoinHandle<()>,
    sim_task: JoinHandle<()>,
    tracker_task: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.driver_task.abort();
        self.sim_task.abort();
        self.tracker_task.abort();
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConnectRequest {
    /// "skytrak" (real hardware, default) or "simulated" (a fake device for
    /// exercising the daemon/UI without hardware -- see `POST /v1/session/shot`).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Connect to a specific box by name (as returned by `GET /v1/devices`).
    /// If omitted along with `address`, connects to the first box discovery
    /// finds. For `kind: "simulated"`, this is just the display name (address
    /// is ignored); defaults to "Simulated Launch Monitor".
    pub name: Option<String>,
    pub address: Option<Ipv4Addr>,
    /// Directed broadcast addresses to try when discovering (e.g. `192.168.7.255`
    /// for a `/22`). Defaults to the global broadcast address.
    #[serde(default)]
    pub broadcast: Vec<Ipv4Addr>,
    #[serde(default)]
    pub right_handed: Option<bool>,
    /// Whether the box's "chipping" mode should arm as hardware Putting
    /// (true, the vendor default) or hardware Normal (false). See
    /// docs/skytrak-protocol/chipping-mode.md.
    #[serde(default)]
    pub chip_via_putting: Option<bool>,
}

fn default_kind() -> String {
    "skytrak".into()
}

impl AppState {
    /// `initial_chip_settings` only matters the very first time this runs on
    /// a machine (no settings file yet); after that, whatever was last saved
    /// via `update_chip_settings` wins, which is what "persistent" means.
    pub fn new(sim: trakr_openconnect::Config, initial_chip_settings: ChipSettings) -> Self {
        let (events, _rx) = broadcast::channel(EVENT_BUS_CAPACITY);
        let chip = settings::load().unwrap_or(initial_chip_settings);
        let (chip_settings, _chip_rx) = watch::channel(chip);
        Self(Arc::new(Inner {
            events,
            session: RwLock::new(None),
            sim,
            chip_settings,
            players: RwLock::new(player_store::load()),
        }))
    }

    pub fn chip_settings(&self) -> ChipSettings {
        *self.0.chip_settings.borrow()
    }

    /// Any parameter left `None` is unchanged. `force_chip_distance_yd` is
    /// `Option<Option<f32>>` so "unchanged" and "explicitly cleared to
    /// disabled" are distinguishable. Persists to disk and, if a session is
    /// connected, applies `chip_via_putting` immediately.
    pub async fn update_chip_settings(
        &self,
        chip_on_lob_wedge: Option<bool>,
        force_chip_distance_yd: Option<Option<f32>>,
        chip_via_putting: Option<bool>,
    ) -> ChipSettings {
        let mut updated = self.chip_settings();
        if let Some(v) = chip_on_lob_wedge {
            updated.chip_on_lob_wedge = v;
        }
        if let Some(v) = force_chip_distance_yd {
            updated.force_chip_distance_yd = v;
        }
        let hw_mapping_changed = chip_via_putting.is_some_and(|v| v != updated.chip_via_putting);
        if let Some(v) = chip_via_putting {
            updated.chip_via_putting = v;
        }
        self.0.chip_settings.send_replace(updated);
        settings::save(&updated);
        if hw_mapping_changed {
            // Best-effort: fine if nothing is connected yet, it'll pick up
            // the setting from chip_settings the next time it connects.
            let _ = self
                .send_command(Command::SetChipViaPutting(updated.chip_via_putting))
                .await;
        }
        updated
    }

    pub async fn snapshot(&self) -> Option<(DeviceInfo, DeviceStatus, Option<Shot>)> {
        let guard = self.0.session.read().await;
        guard.as_ref().map(|s| {
            (
                s.device.clone().unwrap_or(placeholder_device()),
                s.status.clone(),
                s.last_shot.clone(),
            )
        })
    }

    pub async fn is_connected(&self, name: &str, address: Option<Ipv4Addr>) -> bool {
        let guard = self.0.session.read().await;
        guard
            .as_ref()
            .and_then(|s| s.device.as_ref())
            .is_some_and(|d| {
                d.name == name
                    && address
                        .map(|a| d.address.as_deref() == Some(&a.to_string()))
                        .unwrap_or(true)
            })
    }

    pub async fn has_session(&self) -> bool {
        self.0.session.read().await.is_some()
    }

    pub async fn send_command(&self, cmd: Command) -> Result<(), &'static str> {
        let guard = self.0.session.read().await;
        let Some(session) = guard.as_ref() else {
            return Err("no_session");
        };
        session
            .commands
            .send(cmd)
            .await
            .map_err(|_| "driver_task_gone")
    }

    pub async fn disconnect(&self) -> bool {
        let mut guard = self.0.session.write().await;
        if let Some(session) = guard.take() {
            let _ = session.commands.send(Command::Disconnect).await;
            true
        } else {
            false
        }
    }

    pub async fn connect(&self, req: ConnectRequest) -> Result<(), &'static str> {
        if req.kind != "skytrak" && req.kind != "simulated" {
            return Err("unsupported_kind");
        }
        {
            let guard = self.0.session.read().await;
            if guard.is_some() {
                return Err("session_active");
            }
        }
        let right_handed = req.right_handed.unwrap_or(true);
        let chip_via_putting = req
            .chip_via_putting
            .unwrap_or_else(|| self.chip_settings().chip_via_putting);
        let driver: Box<dyn trakr_core::LaunchMonitor> = if req.kind == "simulated" {
            Box::new(
                trakr_core::simulated::SimulatedDriver::new(
                    req.name
                        .unwrap_or_else(|| "Simulated Launch Monitor".into()),
                )
                .with_handedness(right_handed),
            )
        } else {
            match (req.name, req.address) {
                (Some(name), Some(addr)) => Box::new(
                    trakr_skytrak::SkytrakDriver::connect_to(name, addr)
                        .with_handedness(right_handed)
                        .with_chip_via_putting(chip_via_putting),
                ),
                (Some(_), None) | (None, Some(_)) => {
                    return Err("name_and_address_required_together")
                }
                (None, None) => Box::new(
                    trakr_skytrak::SkytrakDriver::discover_and_connect(
                        req.broadcast,
                        Duration::from_secs(3),
                    )
                    .with_handedness(right_handed)
                    .with_chip_via_putting(chip_via_putting),
                ),
            }
        };

        let (commands_tx, commands_rx) = mpsc::channel(16);
        let events_tx = self.0.events.clone();
        let driver_task = tokio::spawn(async move {
            if let Err(e) = driver.run(events_tx, commands_rx).await {
                warn!(error = %e, "driver task ended with error");
            }
        });

        let sim_cfg = self.0.sim.clone();
        let sim_chip_settings = self.0.chip_settings.subscribe();
        let sim_events = self.0.events.subscribe();
        let sim_commands = commands_tx.clone();
        let sim_task = tokio::spawn(async move {
            if let Err(e) =
                trakr_openconnect::run(sim_cfg, sim_chip_settings, sim_events, sim_commands).await
            {
                warn!(error = %e, "sim output task ended with error");
            }
        });

        let tracker_state = self.clone();
        let mut tracker_events = self.0.events.subscribe();
        let tracker_task = tokio::spawn(async move {
            while let Ok(ev) = tracker_events.recv().await {
                tracker_state.apply(ev).await;
            }
        });

        let mut guard = self.0.session.write().await;
        *guard = Some(Session {
            device: None,
            status: DeviceStatus::default(),
            last_shot: None,
            commands: commands_tx,
            driver_task,
            sim_task,
            tracker_task,
        });
        info!("session started");
        Ok(())
    }

    pub async fn list_players(&self) -> Vec<Player> {
        self.0.players.read().await.clone()
    }

    pub async fn get_player(&self, name: &str) -> Option<Player> {
        self.0
            .players
            .read()
            .await
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .cloned()
    }

    pub async fn create_player(
        &self,
        name: String,
        right_handed: bool,
    ) -> Result<Player, &'static str> {
        let mut guard = self.0.players.write().await;
        if guard.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
            return Err("player_exists");
        }
        let player = Player {
            name,
            right_handed,
            bag: Vec::new(),
        };
        guard.push(player.clone());
        player_store::save(guard.as_slice());
        Ok(player)
    }

    /// Idempotent: `true` if a player was actually removed.
    pub async fn delete_player(&self, name: &str) -> bool {
        let mut guard = self.0.players.write().await;
        let before = guard.len();
        guard.retain(|p| !p.name.eq_ignore_ascii_case(name));
        let removed = guard.len() != before;
        if removed {
            player_store::save(guard.as_slice());
        }
        removed
    }

    /// Upserts one club's carry distance in `name`'s bag.
    pub async fn set_club_carry(
        &self,
        name: &str,
        club: &str,
        carry_yd: f32,
    ) -> Result<Player, &'static str> {
        let mut guard = self.0.players.write().await;
        let Some(player) = guard.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name)) else {
            return Err("no_such_player");
        };
        if let Some(entry) = player
            .bag
            .iter_mut()
            .find(|c| c.club.eq_ignore_ascii_case(club))
        {
            entry.carry_yd = carry_yd;
        } else {
            player.bag.push(ClubCarry {
                club: club.to_string(),
                carry_yd,
            });
        }
        let updated = player.clone();
        player_store::save(guard.as_slice());
        Ok(updated)
    }

    /// Idempotent: removing a club that isn't in the bag is not an error.
    pub async fn remove_club(&self, name: &str, club: &str) -> Result<Player, &'static str> {
        let mut guard = self.0.players.write().await;
        let Some(player) = guard.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name)) else {
            return Err("no_such_player");
        };
        player.bag.retain(|c| !c.club.eq_ignore_ascii_case(club));
        let updated = player.clone();
        player_store::save(guard.as_slice());
        Ok(updated)
    }

    async fn apply(&self, ev: Event) {
        let mut guard = self.0.session.write().await;
        let Some(session) = guard.as_mut() else {
            return;
        };
        match ev {
            Event::Connected(info) => session.device = Some(info),
            Event::Status(status) => session.status = status,
            Event::Ready => session.status.armed = true,
            Event::ShotStarted => {}
            Event::Shot(shot) => session.last_shot = Some(shot),
            Event::Disconnected { .. } => session.status.armed = false,
            Event::Misread { .. } | Event::Error { .. } | Event::Discovered(_) => {}
        }
    }
}

fn placeholder_device() -> DeviceInfo {
    DeviceInfo {
        kind: "skytrak",
        name: "(connecting)".into(),
        address: None,
        connection: trakr_core::ConnectionKind::WifiNetwork,
        firmware: None,
    }
}
