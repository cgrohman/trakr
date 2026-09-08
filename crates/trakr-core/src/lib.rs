//! trakr-core: hardware-agnostic launch monitor model.
//!
//! A driver (one per hardware family) implements [`LaunchMonitor`] and emits
//! [`Event`]s on a channel. Outputs (GSPro Open Connect, loggers, ...) consume
//! those events and may send [`Command`]s back. Nothing here knows about wire
//! protocols or simulators.

pub mod device;
pub mod shot;

pub use device::{Capabilities, ConnectionKind, DeviceInfo, DeviceStatus, Handedness, ShotMode};
pub use shot::{BallData, ClubData, Confidence, FlightEstimate, Shot};

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

/// Everything a driver can tell the rest of the system.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A device was found but not yet connected.
    Discovered(DeviceInfo),
    Connected(DeviceInfo),
    Disconnected {
        reason: String,
    },
    /// Periodic status (battery, tilt, handedness, current mode, armed).
    Status(DeviceStatus),
    /// Device is armed and waiting for a ball.
    Ready,
    /// A ball strike was detected; data follows in `Shot`.
    ShotStarted,
    Shot(Shot),
    /// A strike happened but the device could not measure it.
    Misread {
        reason: String,
    },
    Error {
        message: String,
    },
}

/// Everything an output can ask a driver to do.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Arm,
    Disarm,
    SetHandedness(Handedness),
    SetShotMode(ShotMode),
    Disconnect,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("device not connected")]
    NotConnected,
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("transport: {0}")]
    Transport(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A launch monitor driver. Implementations own their transport and run until
/// `commands` closes or `Command::Disconnect` arrives.
#[async_trait]
pub trait LaunchMonitor: Send {
    /// Stable identifier for config and logs, e.g. `"skytrak"`.
    fn kind(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    /// Run the driver: discover, connect, and stream events until stopped.
    async fn run(
        self: Box<Self>,
        events: broadcast::Sender<Event>,
        commands: mpsc::Receiver<Command>,
    ) -> Result<()>;
}

/// Event bus capacity. Shots are rare; status is at most a few Hz.
pub const EVENT_BUS_CAPACITY: usize = 256;
