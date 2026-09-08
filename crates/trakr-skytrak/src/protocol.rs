//! Message vocabulary of the SkyTrak "box". Numeric encodings are filled in as
//! each is confirmed against captured traffic; names come from the device's
//! own diagnostic strings.

/// Address the box uses for itself in WiFi Direct mode (it runs a DHCP server
/// and hands the host a 10.0.0.x lease).
pub const DIRECT_MODE_BOX_IP: [u8; 4] = [10, 0, 0, 1];

/// Commands the host can send. Order matches the device's command table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxCommand {
    None,
    Arm,
    Disarm,
    ShotModeNormal,
    ShotModePutting,
    PlayerHandRight,
    PlayerHandLeft,
    AssistAlignmentOn,
    AssistAlignmentOff,
    ModeNormal,
    ModeCalibrate,
    ModeDebug,
    EnableReferenceLaser,
    SetNetworkConfig,
    GetNetworkScanListResult,
    SetUserPersistentData,
    GetUserPersistentData,
    BootForceDirectConnect,
    BootForceNetworkConnect,
    UpgradeFirmware,
    Restart,
    ShutdownBox,
}

/// Events the box reports back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxEvent {
    Connected,
    Disconnected,
    StatusUpdate,
    StaticUpdate,
    Tilted,
    BatteryLow,
    Error,
    TriggerDetected,
    SpeedReady,
    SpinReady,
    UserPersistentDataReady,
    NetworkScanListUpdate,
    FirmwareUpgradeAvailable,
    FirmwareUpgradeSuccess,
    FirmwareUpgradeError,
    FirmwareSdkIncompatible,
}

/// Where the ball sat relative to the capture zone when the shot fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BallPosition {
    Ok,
    Near,
    Far,
    Unknown,
}
