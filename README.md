# trakr

Launch monitor bridge for golf simulators. Drivers talk to hardware; outputs
talk to simulators. First driver: a clean-room implementation of the original
SkyTrak (2014 photometric unit). First output: GSPro Open Connect v1, which is
what Muni Golf Sim, GSPro, and OpenGolfAPI accept.

Runs on Linux (x86_64, aarch64), macOS, and Windows as a single binary. No
vendor app, no subscription, no Wine.

## Layout

| Crate | Purpose |
|---|---|
| `crates/trakr-core` | `LaunchMonitor` trait, `Shot`/`DeviceStatus` model, event and command bus. Add hardware here. |
| `crates/trakr-skytrak` | Original SkyTrak driver: discovery, session, packet codec. |
| `crates/trakr-openconnect` | Open Connect v1 client output with heartbeat, reconnect, and club/handedness feedback. |
| `crates/trakr` | CLI and daemon. |
| `research/` | Reverse-engineering workspace, see `research/README.md`. Not shipped. |
| `docs/` | Protocol notes and architecture decisions. |

## Quick start

```sh
cargo build --release
# Prove the sim link: sends one synthetic drive to Muni / GSPro on port 921.
./target/release/trakr test-shot --host 127.0.0.1 --port 921
```

## Status

- [x] Core model and event bus
- [x] Open Connect output
- [ ] SkyTrak wire protocol documented (`docs/skytrak-protocol.md`)
- [ ] SkyTrak driver: discovery, connect, arm, shots
- [ ] `trakr run` daemon with config file
- [ ] Additional drivers

## Legal

trakr is an independent project. It is not affiliated with SkyTrak, GOLFTEC,
Rapsodo, GSPro, or Muni Golf Sim. The SkyTrak driver is a clean-room
implementation for interoperability with hardware the user owns.
