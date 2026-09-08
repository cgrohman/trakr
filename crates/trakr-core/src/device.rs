use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Usb,
    /// Device is the WiFi access point; host joins it.
    WifiDirect,
    /// Device joined the user's LAN.
    WifiNetwork,
    Bluetooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Handedness {
    Right,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotMode {
    Normal,
    Putting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub kind: &'static str,
    /// Device's own name or serial, e.g. the SkyTrak box name.
    pub name: String,
    pub address: Option<String>,
    pub connection: ConnectionKind,
    pub firmware: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DeviceStatus {
    pub battery_pct: Option<f32>,
    pub charging: Option<bool>,
    pub roll_deg: Option<f32>,
    pub tilt_deg: Option<f32>,
    pub rssi: Option<i32>,
    pub handedness: Option<Handedness>,
    pub shot_mode: Option<ShotMode>,
    pub armed: bool,
}

/// What a driver can measure or accept. Outputs use this to decide which
/// fields to send and which commands are worth issuing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub ball_speed: bool,
    pub launch_angles: bool,
    pub spin: bool,
    pub club_data: bool,
    pub putting_mode: bool,
    pub handedness: bool,
    pub arm_disarm: bool,
}
