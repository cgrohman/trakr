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

# Start the daemon: owns the launch monitor session, serves the HTTP + SSE
# API, and bridges shots to your simulator's GSPro Open Connect listener.
./target/release/trakr serve --sim-host 127.0.0.1 --sim-port 921 &

./target/release/trakr devices                                  # find it
./target/release/trakr connect --name <name> --address <ip>     # connect
./target/release/trakr arm                                      # ready for a shot
./target/release/trakr events                                   # watch it happen

# Or prove just the sim link, no hardware involved:
./target/release/trakr test-shot --host 127.0.0.1 --port 921
```

Full HTTP API reference: [docs/api.md](docs/api.md). The API is the source
of truth — the CLI above and the planned Tauri UI are both thin clients over
it, so anything either can do, an agent can do with the same HTTP calls.

## Status

- [x] Core model and event bus
- [x] Open Connect output
- [x] SkyTrak wire protocol documented and confirmed against real hardware (`docs/skytrak-protocol/`)
- [x] SkyTrak driver: discovery, connect handshake, arm/disarm, status — confirmed against real hardware
- [x] `trakr serve` daemon with HTTP + SSE API (`docs/api.md`, `GET /v1/openapi.json`)
- [x] AXI-conventioned CLI (`trakr devices/connect/status/arm/disarm/mode/hand/events`)
- [ ] Shot capture: decoding the original SkyTrak's camera images into ball speed/spin/angles (see `docs/skytrak-protocol/shot-data.md`)
- [ ] Tauri UI on top of the same API
- [ ] Additional drivers

## Legal

trakr is an independent project. It is not affiliated with SkyTrak, GOLFTEC,
Rapsodo, GSPro, or Muni Golf Sim. The SkyTrak driver is a clean-room
implementation for interoperability with hardware the user owns.
