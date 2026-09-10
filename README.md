# trakr

Launch monitor bridge for golf simulators. A driver talks to hardware; an
output talks to a simulator; a daemon in the middle exposes the whole session
over an HTTP + Server-Sent-Events API so it's driveable by a CLI, an AI
agent, or a GUI without preferring any one of them.

First driver: a clean-room implementation of the original SkyTrak (2014
photometric launch monitor). First output: GSPro Open Connect v1, which is
what Muni Golf Sim, GSPro, and OpenGolfAPI accept.

Written in Rust so it compiles to a single binary with no vendor app, no
subscription, and no Wine. Built and tested on macOS (arm64) so far; nothing
in the code is macOS-specific, but Linux and Windows builds haven't been
exercised yet.

## Layout

| Crate | Purpose |
|---|---|
| `crates/trakr-core` | `LaunchMonitor` trait, `Shot`/`DeviceStatus` model, event and command bus. Add new hardware here. |
| `crates/trakr-skytrak` | Original SkyTrak driver: UDP discovery, TCP connect handshake, packet framing/CRC, arm/disarm, status decoding. |
| `crates/trakr-openconnect` | GSPro Open Connect v1 client output: heartbeat, reconnect, club/handedness feedback. |
| `crates/trakr-daemon` | The long-running process: owns one session, serves the HTTP + SSE API. |
| `crates/trakr` | CLI — a thin, AXI-conventioned (Agent eXperience Interface) client over the daemon's API. |
| `docs/` | Protocol reverse-engineering notes and the HTTP API reference. |
| `research/` | Reverse-engineering workspace (decompiles, capture scripts). Not shipped — see `research/README.md`. |

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

Run `trakr <command> --help` for flags and examples on any subcommand.

### Commands

| Command | Purpose |
|---|---|
| `serve` | Run the daemon in the foreground |
| `devices` | Broadcast-discover launch monitors on the network |
| `connect` | Connect to a device by name+address, or auto-discover |
| `status` | Show the current session's device and status |
| `arm` / `disarm` | Arm or disarm the connected device |
| `mode` | Set shot mode (`normal` \| `putting` \| `chipping`) |
| `hand` | Set player handedness (`right` \| `left`) |
| `disconnect` | End the current session |
| `events` | Stream session events (status, shots, errors) live |
| `test-shot` | Send one synthetic shot straight to a simulator, bypassing the daemon |
| `shot` | Fire a synthetic shot through a `connect --simulate` session -- see below |

Full HTTP API reference: [docs/api.md](docs/api.md), machine-readable at
`GET /v1/openapi.json` once the daemon is running. The API is the source of
truth — the CLI above and the Tauri desktop app (`apps/trakr-ui`) are both
thin clients over it, so anything either can do, an agent can do with the
same HTTP calls.

## Connecting to Muni Golf Sim

Muni doesn't talk to launch monitors from the game itself — a bundled
companion app, **Launch Monitor Connect**, does, and it ships for both
Windows and Linux. Its "SkyTrak / SkyTrak+" and "Open Connect API" device
modes are the same code path: a plain GSPro Open Connect v1 listener,
default port `921`, that accepts connections from any machine on the
network, not just its own. That's exactly the protocol `trakr-openconnect`
already speaks as a client, so there's nothing to configure on trakr's side
beyond pointing it at the right address.

Both `trakr serve` and the desktop app already default to `127.0.0.1:921`,
and the connection to Muni starts automatically the instant you connect to a
launch monitor — there's no separate step. In practice:

1. Launch Muni's game and its Launch Monitor Connect app.
2. In Launch Monitor Connect's device dropdown, pick **SkyTrak / SkyTrak+**
   or **Open Connect API** (either works identically) and note the port it
   shows (`921` by default).
3. If Muni is running on a different machine than trakr, use that machine's
   LAN address: `trakr serve --sim-host <address> --sim-port 921` (or
   `TRAKR_SIM_HOST`/`TRAKR_SIM_PORT` for the desktop app).
4. Connect trakr to your launch monitor as usual — `trakr connect`, the API,
   or the desktop app. Muni picks it up with no further action.

Prove the link before touching hardware:

```sh
trakr test-shot --host <address> --port 921
```

This works today for heartbeat, ready state, arming, and handedness/club
feedback from Muni. Real ball strikes won't produce real numbers in Muni yet
— see the shot capture gap in Status below.

To drive several shots through a round and watch trakr's own state react
(not just prove the link once), use a simulated session instead -- unlike
`test-shot`, this goes through the daemon, so shots show up in `trakr
events` and the Tauri UI, and mode/handedness auto-switch from Muni's
player-info responses exactly as they would with real hardware:

```sh
trakr serve --sim-host <address> --sim-port 921 &
trakr connect --simulate
trakr arm
trakr shot                          # defaults; repeat with --speed/--vla/... per shot
trakr events                        # watch it happen
```

## Development

```sh
cargo build --workspace       # build everything
cargo test --workspace        # unit tests, including CRC values confirmed
                               # against a real SkyTrak (see crates/trakr-skytrak)
cargo clippy --workspace --all-targets
cargo fmt
```

## Status

- [x] Core model and event bus
- [x] Open Connect output
- [x] SkyTrak wire protocol documented and confirmed against real hardware (`docs/skytrak-protocol/`)
- [x] SkyTrak driver: discovery, connect handshake, arm/disarm, status — confirmed against real hardware
- [x] `trakr serve` daemon with HTTP + SSE API (`docs/api.md`, `GET /v1/openapi.json`)
- [x] AXI-conventioned CLI (`trakr devices/connect/status/arm/disarm/mode/hand/events`)
- [x] Tauri desktop app (`apps/trakr-ui`) with a live, persistent Settings panel
- [ ] Shot capture: decoding the original SkyTrak's camera images into ball speed/spin/angles (see `docs/skytrak-protocol/shot-data.md`)
- [ ] Linux/Windows builds exercised (should work; not yet tested)
- [ ] Additional drivers

## Legal

trakr is an independent project. It is not affiliated with SkyTrak, GOLFTEC,
Rapsodo, GSPro, or Muni Golf Sim. The SkyTrak driver is a clean-room
implementation for interoperability with hardware the user owns. MIT
licensed — see [LICENSE](LICENSE).
