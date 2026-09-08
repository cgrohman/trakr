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

## What's not here yet

The original SkyTrak sends encrypted camera images rather than finished shot
numbers (see `docs/skytrak-protocol/shot-data.md`); decoding those into a
`shot` event is not implemented. A real ball strike currently surfaces as a
`misread` event so you know something happened. Discovery, connect, arm,
disarm, and status are confirmed against real hardware.
