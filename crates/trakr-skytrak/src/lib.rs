//! trakr-skytrak: clean-room driver for the original SkyTrak (2014 unit).
//!
//! Status: protocol constants and message vocabulary recovered from the
//! device's public behaviour and the retired third-party connector's event
//! names. Wire framing, encryption, and the discovery exchange are being
//! documented in `docs/skytrak-protocol.md` before the transport is written.

pub mod protocol;

use trakr_core::Capabilities;

pub const KIND: &str = "skytrak";

/// The original SkyTrak measures ball only. Club speed is never reported.
pub const CAPABILITIES: Capabilities = Capabilities {
    ball_speed: true,
    launch_angles: true,
    spin: true,
    club_data: false,
    putting_mode: true,
    handedness: true,
    arm_disarm: true,
};
