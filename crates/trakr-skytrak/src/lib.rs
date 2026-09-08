//! trakr-skytrak: clean-room driver for the original SkyTrak (2014 unit).
//!
//! Discovery, the connect handshake, arm/disarm, and status decoding are
//! confirmed against a real unit (see `docs/skytrak-protocol/`). Shot capture
//! (turning camera images into ball speed/spin/angles) is not yet
//! implemented — see `session.rs` module docs.

pub mod discovery;
pub mod protocol;
pub mod session;
pub mod wire;

pub use discovery::{discover, DiscoveredBox};
pub use session::{SkytrakDriver, CAPABILITIES, KIND};
