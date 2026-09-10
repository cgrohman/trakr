//! trakr-skytrak: clean-room driver for the original SkyTrak (2014 unit).
//!
//! Discovery, the connect handshake, arm/disarm, and status decoding are
//! confirmed against a real unit (see `docs/skytrak-protocol/`). Shot decode
//! (turning camera images into ball speed/launch/horizontal angle) is
//! implemented with a simplified ball detector in `shot_decode` — see that
//! module's docs for what is and isn't covered (notably: no spin yet).

pub mod discovery;
pub mod protocol;
pub mod session;
pub mod shot_decode;
pub mod wire;

pub use discovery::{discover, DiscoveredBox};
pub use session::{SkytrakDriver, CAPABILITIES, KIND};
