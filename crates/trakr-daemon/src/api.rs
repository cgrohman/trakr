use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use trakr_core::{Command, Handedness, ShotMode};

use crate::state::{AppState, ConnectRequest};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/openapi.json", get(openapi))
        .route("/v1/devices", get(list_devices))
        .route(
            "/v1/session",
            get(get_session).post(post_session).delete(delete_session),
        )
        .route("/v1/session/arm", post(arm))
        .route("/v1/session/disarm", post(disarm))
        .route("/v1/session/mode", post(set_mode))
        .route("/v1/session/hand", post(set_hand))
        .route("/v1/events", get(events))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

async fn openapi() -> impl IntoResponse {
    (
        [("content-type", "application/json")],
        include_str!("openapi.json"),
    )
}

#[derive(serde::Deserialize)]
struct DevicesQuery {
    window_ms: Option<u64>,
    /// Comma-separated directed broadcast addresses, e.g. "192.168.7.255".
    broadcast: Option<String>,
}

async fn list_devices(Query(q): Query<DevicesQuery>) -> impl IntoResponse {
    let window = Duration::from_millis(q.window_ms.unwrap_or(3000));
    let broadcast: Vec<Ipv4Addr> = q
        .broadcast
        .as_deref()
        .unwrap_or("")
        .split(",")
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    match trakr_skytrak::discover(window, &broadcast).await {
        Ok(boxes) => {
            let devices: Vec<_> = boxes
                .into_iter()
                .map(|b| json!({ "kind": "skytrak", "name": b.name, "address": b.address.to_string(), "direct_mode": b.direct_mode }))
                .collect();
            Json(json!({ "devices": devices, "count": devices.len() })).into_response()
        }
        Err(e) => err(500, "discover_failed", &e.to_string()),
    }
}

async fn get_session(State(state): State<AppState>) -> impl IntoResponse {
    match state.snapshot().await {
        Some((device, status, last_shot)) => {
            Json(json!({ "device": device, "status": status, "last_shot": last_shot }))
                .into_response()
        }
        None => err(
            404,
            "no_session",
            "no launch monitor is connected. POST /v1/session to connect",
        ),
    }
}

async fn post_session(
    State(state): State<AppState>,
    Json(req): Json<ConnectRequest>,
) -> impl IntoResponse {
    let (name, address) = (req.name.clone(), req.address);
    if state
        .is_connected(name.as_deref().unwrap_or(""), address)
        .await
    {
        return Json(json!({ "already_connected": true })).into_response();
    }
    match state.connect(req).await {
        Ok(()) => (axum::http::StatusCode::ACCEPTED, Json(json!({ "connecting": true, "help": "GET /v1/session for status, or GET /v1/events to stream progress" }))).into_response(),
        Err("session_active") => err(409, "session_active", "a session is already active. DELETE /v1/session first, or this is a no-op if you meant the same device"),
        Err("unsupported_kind") => err(422, "unsupported_kind", "only \"skytrak\" is supported right now"),
        Err("name_and_address_required_together") => err(422, "invalid_request", "provide both name and address, or neither (to discover)"),
        Err(other) => err(500, "connect_failed", other),
    }
}

async fn delete_session(State(state): State<AppState>) -> impl IntoResponse {
    if state.disconnect().await {
        Json(json!({ "disconnected": true })).into_response()
    } else {
        Json(json!({ "disconnected": true, "note": "no-op: no session was active" }))
            .into_response()
    }
}

async fn arm(State(state): State<AppState>) -> impl IntoResponse {
    command_response(&state, Command::Arm).await
}

async fn disarm(State(state): State<AppState>) -> impl IntoResponse {
    command_response(&state, Command::Disarm).await
}

#[derive(serde::Deserialize)]
struct ModeBody {
    mode: String,
}

async fn set_mode(State(state): State<AppState>, Json(body): Json<ModeBody>) -> impl IntoResponse {
    let mode = match body.mode.as_str() {
        "normal" => ShotMode::Normal,
        "putting" => ShotMode::Putting,
        "chipping" => ShotMode::Chipping,
        _ => {
            return err(
                422,
                "invalid_mode",
                "mode must be \"normal\", \"putting\", or \"chipping\"",
            )
        }
    };
    command_response(&state, Command::SetShotMode(mode)).await
}

#[derive(serde::Deserialize)]
struct HandBody {
    hand: String,
}

async fn set_hand(State(state): State<AppState>, Json(body): Json<HandBody>) -> impl IntoResponse {
    let hand = match body.hand.as_str() {
        "right" => Handedness::Right,
        "left" => Handedness::Left,
        _ => return err(422, "invalid_hand", "hand must be \"right\" or \"left\""),
    };
    command_response(&state, Command::SetHandedness(hand)).await
}

async fn command_response(state: &AppState, cmd: Command) -> axum::response::Response {
    match state.send_command(cmd).await {
        Ok(()) => (axum::http::StatusCode::ACCEPTED, Json(json!({ "accepted": true, "help": "GET /v1/session or /v1/events to see the resulting state" }))).into_response(),
        Err("no_session") => err(404, "no_session", "no launch monitor is connected. POST /v1/session to connect"),
        Err(other) => err(500, "command_failed", other),
    }
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.0.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| {
        let ev = item.ok()?;
        let data = serde_json::to_string(&ev).ok()?;
        Some(Ok(SseEvent::default().event(event_type(&ev)).data(data)))
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn event_type(ev: &trakr_core::Event) -> &'static str {
    match ev {
        trakr_core::Event::Discovered(_) => "discovered",
        trakr_core::Event::Connected(_) => "connected",
        trakr_core::Event::Disconnected { .. } => "disconnected",
        trakr_core::Event::Status(_) => "status",
        trakr_core::Event::Ready => "ready",
        trakr_core::Event::ShotStarted => "shot_started",
        trakr_core::Event::Shot(_) => "shot",
        trakr_core::Event::Misread { .. } => "misread",
        trakr_core::Event::Error { .. } => "error",
    }
}

fn err(status: u16, error: &str, help: &str) -> axum::response::Response {
    let code = axum::http::StatusCode::from_u16(status)
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(json!({ "error": error, "help": help }))).into_response()
}
