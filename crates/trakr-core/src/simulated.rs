//! A fake [`LaunchMonitor`] with no transport, for exercising the daemon,
//! HTTP API, and Tauri UI end to end without real hardware. It tracks
//! handedness/mode/armed locally and turns `Command::FireShot` into the same
//! `ShotStarted` + `Shot` events a real driver would emit, so everything
//! downstream (the Open Connect output, `trakr events`, the UI's event log
//! and shot panel) behaves exactly as it would with a real box.

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

use crate::{
    Capabilities, Command, ConnectionKind, DeviceInfo, DeviceStatus, Event, Handedness,
    LaunchMonitor, Result, ShotMode,
};

pub const KIND: &str = "simulated";

pub const CAPABILITIES: Capabilities = Capabilities {
    ball_speed: true,
    launch_angles: true,
    spin: true,
    club_data: false,
    putting_mode: true,
    handedness: true,
    arm_disarm: true,
};

pub struct SimulatedDriver {
    name: String,
    right_handed: bool,
}

impl SimulatedDriver {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            right_handed: true,
        }
    }

    pub fn with_handedness(mut self, right_handed: bool) -> Self {
        self.right_handed = right_handed;
        self
    }
}

impl Default for SimulatedDriver {
    fn default() -> Self {
        Self::new("Simulated Launch Monitor")
    }
}

#[async_trait]
impl LaunchMonitor for SimulatedDriver {
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
        events
            .send(Event::Connected(DeviceInfo {
                kind: KIND,
                name: self.name.clone(),
                address: None,
                connection: ConnectionKind::WifiNetwork,
                firmware: None,
            }))
            .ok();
        let mut mode = ShotMode::Normal;
        let mut armed = false;
        let mut sequence = 0u32;

        while let Some(cmd) = commands.recv().await {
            match cmd {
                Command::Disconnect => break,
                Command::Arm => {
                    armed = true;
                    events
                        .send(Event::Status(status(armed, self.right_handed, mode)))
                        .ok();
                    events.send(Event::Ready).ok();
                }
                Command::Disarm => {
                    armed = false;
                    events
                        .send(Event::Status(status(armed, self.right_handed, mode)))
                        .ok();
                }
                Command::SetHandedness(h) => {
                    self.right_handed = matches!(h, Handedness::Right);
                    events
                        .send(Event::Status(status(armed, self.right_handed, mode)))
                        .ok();
                }
                Command::SetShotMode(m) => {
                    mode = m;
                    events
                        .send(Event::Status(status(armed, self.right_handed, mode)))
                        .ok();
                }
                Command::SetChipViaPutting(_) => {}
                Command::FireShot(mut shot) => {
                    // Only fires while armed and waiting, like a real device.
                    if !armed {
                        continue;
                    }
                    sequence += 1;
                    shot.sequence = sequence;
                    events.send(Event::ShotStarted).ok();
                    events.send(Event::Shot(shot)).ok();
                    armed = false;
                    events
                        .send(Event::Status(status(armed, self.right_handed, mode)))
                        .ok();
                }
            }
        }
        events
            .send(Event::Disconnected {
                reason: "disconnected".into(),
            })
            .ok();
        Ok(())
    }
}

fn status(armed: bool, right_handed: bool, mode: ShotMode) -> DeviceStatus {
    DeviceStatus {
        battery_pct: None,
        charging: None,
        roll_deg: None,
        tilt_deg: None,
        rssi: None,
        handedness: Some(if right_handed {
            Handedness::Right
        } else {
            Handedness::Left
        }),
        shot_mode: Some(mode),
        armed,
    }
}
