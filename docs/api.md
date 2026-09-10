# trakr HTTP API

The `trakr` daemon (`trakr serve`) is the single source of truth for a launch
monitor session. The `trakr` CLI, an AI agent, and the future Tauri UI all
drive the same API — nothing the CLI can do is CLI-only.

Base URL: `http://127.0.0.1:7011` (configurable via `trakr serve --bind`).
Full machine-readable spec: `GET /v1/openapi.json` (OpenAPI 3.0).

## Model

There is one session at a time in v1: connect to a launch monitor, drive it,
disconnect. All endpoints are idempotent where it matters — connecting to the
device you're already connected to, or disconnecting when nothing is
connected, both return `200` rather than erroring.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/v1/health` | Liveness check |
| GET | `/v1/devices` | Broadcast-discover launch monitors (no session required) |
| GET | `/v1/session` | Current device + status, or `404` if none |
| POST | `/v1/session` | Connect (by name+address, or auto-discover if omitted) |
| DELETE | `/v1/session` | Disconnect |
| POST | `/v1/session/arm` | Arm (activates lasers/cameras) |
| POST | `/v1/session/disarm` | Disarm |
| POST | `/v1/session/mode` | `{"mode": "normal"\|"putting"\|"chipping"}` (see `docs/skytrak-protocol/chipping-mode.md`) |
| POST | `/v1/session/hand` | `{"hand": "right"\|"left"}` |
| POST | `/v1/session/shot` | Fire a synthetic shot (only on a `kind: "simulated"` session — see below) |
| GET | `/v1/clubs` | Reference club launch/spin table, used by `/v1/session/shot`'s `club` field |
| GET/POST | `/v1/players` | List / create player profiles (name, handedness, bag of club carry distances) |
| GET/DELETE | `/v1/players/{name}` | Get / remove one player |
| PUT/DELETE | `/v1/players/{name}/clubs/{club}` | Set / remove one club's carry distance in a player's bag |
| GET | `/v1/settings` | Current chipping settings (persisted to disk) |
| POST | `/v1/settings` | Partial update; applies live, no reconnect needed |
| GET | `/v1/events` | Server-Sent Events stream of everything the session does |

## Errors

Every non-2xx response is `{"error": "<code>", "help": "<what to do>"}`.
`error` codes are stable and meant to be matched on; `help` is prose for a
human or an agent that doesn't already know the fix.

## Events

`GET /v1/events` is a standard `text/event-stream`. Each event's `event:`
line is one of `discovered`, `connected`, `disconnected`, `status`, `ready`,
`shot_started`, `shot`, `misread`, `error`; `data:` is the JSON body with a
matching `"type"` field, straight off `trakr_core::Event`. Consume it with
any SSE/EventSource client, or from a shell:

```sh
curl -N http://127.0.0.1:7011/v1/events
```

## Driving it as an agent

A full session from a cold start:

```sh
curl -s http://127.0.0.1:7011/v1/devices | jq
curl -s -X POST http://127.0.0.1:7011/v1/session \
  -H 'content-type: application/json' \
  -d '{"name":"SKYTRAK_C47F51902EE3","address":"192.168.4.61"}'
curl -s http://127.0.0.1:7011/v1/session | jq
curl -s -X POST http://127.0.0.1:7011/v1/session/arm
curl -N http://127.0.0.1:7011/v1/events   # watch for "ready", then shots
curl -s -X DELETE http://127.0.0.1:7011/v1/session
```

Or drive the same session through the CLI, which is a thin wrapper over this
API and prints [TOON](https://toonformat.dev/) instead of raw JSON:

```sh
trakr serve &
trakr devices
trakr connect --name SKYTRAK_C47F51902EE3 --address 192.168.4.61
trakr arm
trakr events
```

## Testing without hardware

`POST /v1/session {"kind": "simulated"}` connects a fake device instead of a
real SkyTrak — same session, status, arm/disarm, mode, and hand endpoints,
plus one extra: `POST /v1/session/shot` fires a synthetic shot through the
whole event pipeline (`shot_started` + `shot` on `/v1/events`, forwarded to
the sim via Open Connect exactly like a real one). Useful for driving several
shots through a round in Muni Golf Sim (or any GSPro Open Connect sim) and
watching the CLI/UI react, without needing hardware connected. Every field is
optional and defaults to the same values as `trakr test-shot`:

```sh
curl -s -X POST http://127.0.0.1:7011/v1/session -d '{"kind":"simulated"}' -H 'content-type: application/json'
curl -s -X POST http://127.0.0.1:7011/v1/session/arm
curl -s -X POST http://127.0.0.1:7011/v1/session/shot \
  -H 'content-type: application/json' \
  -d '{"speed_mph": 150, "vla_deg": 12.5, "hla_deg": 1, "spin_rpm": 2800, "axis_deg": -3}'
```

Unlike `trakr test-shot` (which bypasses the daemon entirely, for a quick
one-off link check), this goes through a real session — so it's what the
Tauri UI uses for its "Simulated" connect option.

### Players and clubs

`POST /v1/session/shot` also accepts a `club` (see `GET /v1/clubs` for the
reference launch/spin table) instead of raw numbers, and optionally a
`player` whose bag (`GET/POST /v1/players`, `PUT/DELETE
/v1/players/{name}/clubs/{club}`) supplies that club's real carry distance:

```sh
curl -s -X POST http://127.0.0.1:7011/v1/players -d '{"name":"Cori"}' -H 'content-type: application/json'
curl -s -X PUT http://127.0.0.1:7011/v1/players/Cori/clubs/7I -d '{"carry_yd":145}' -H 'content-type: application/json'
curl -s -X POST http://127.0.0.1:7011/v1/session/shot -d '{"club":"7I","player":"Cori"}' -H 'content-type: application/json'
```

Players are reference data (persisted the same way as chip settings): the
only place they drive behavior is this carry-distance lookup.

## What's not here yet

The original SkyTrak sends encrypted camera images rather than finished shot
numbers (see `docs/skytrak-protocol/shot-data.md`); decoding those into a
`shot` event is not implemented. A real ball strike currently surfaces as a
`misread` event so you know something happened. Discovery, connect, arm,
disarm, and status are confirmed against real hardware.
