use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use trakr_core::{Command, DeviceInfo, DeviceStatus, Event, Shot, EVENT_BUS_CAPACITY};

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub events: broadcast::Sender<Event>,
    pub session: RwLock<Option<Session>>,
    pub sim: trakr_openconnect::Config,
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
    /// Currently only "skytrak" is supported.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Connect to a specific box by name (as returned by `GET /v1/devices`).
    /// If omitted along with `address`, connects to the first box discovery finds.
    pub name: Option<String>,
    pub address: Option<Ipv4Addr>,
    /// Directed broadcast addresses to try when discovering (e.g. `192.168.7.255`
    /// for a `/22`). Defaults to the global broadcast address.
    #[serde(default)]
    pub broadcast: Vec<Ipv4Addr>,
    #[serde(default)]
    pub right_handed: Option<bool>,
}

fn default_kind() -> String {
    "skytrak".into()
}

impl AppState {
    pub fn new(sim: trakr_openconnect::Config) -> Self {
        let (events, _rx) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self(Arc::new(Inner {
            events,
            session: RwLock::new(None),
            sim,
        }))
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
        if req.kind != "skytrak" {
            return Err("unsupported_kind");
        }
        {
            let guard = self.0.session.read().await;
            if guard.is_some() {
                return Err("session_active");
            }
        }
        let right_handed = req.right_handed.unwrap_or(true);
        let driver: Box<dyn trakr_core::LaunchMonitor> = match (req.name, req.address) {
            (Some(name), Some(addr)) => Box::new(
                trakr_skytrak::SkytrakDriver::connect_to(name, addr).with_handedness(right_handed),
            ),
            (Some(_), None) | (None, Some(_)) => return Err("name_and_address_required_together"),
            (None, None) => Box::new(
                trakr_skytrak::SkytrakDriver::discover_and_connect(
                    req.broadcast,
                    Duration::from_secs(3),
                )
                .with_handedness(right_handed),
            ),
        };

        let (commands_tx, commands_rx) = mpsc::channel(16);
        let events_tx = self.0.events.clone();
        let driver_task = tokio::spawn(async move {
            if let Err(e) = driver.run(events_tx, commands_rx).await {
                warn!(error = %e, "driver task ended with error");
            }
        });

        let sim_cfg = self.0.sim.clone();
        let sim_events = self.0.events.subscribe();
        let sim_commands = commands_tx.clone();
        let sim_task = tokio::spawn(async move {
            if let Err(e) = trakr_openconnect::run(sim_cfg, sim_events, sim_commands).await {
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
            Event::Misread { .. } | Event::Error(_) | Event::Discovered(_) => {}
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
