use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use trakr_core::{BallData, Command, Confidence, Handedness, Shot, ShotMode};

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
        .route("/v1/session/shot", post(fire_shot))
        .route("/v1/players", get(list_players).post(create_player))
        .route("/v1/players/{name}", get(get_player).delete(delete_player))
        .route(
            "/v1/players/{name}/clubs/{club}",
            put(set_club).delete(remove_club),
        )
        .route("/v1/clubs", get(list_clubs))
        .route("/v1/settings", get(get_settings).post(post_settings))
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

#[derive(serde::Deserialize)]
struct ShotBody {
    speed_mph: Option<f32>,
    vla_deg: Option<f32>,
    hla_deg: Option<f32>,
    spin_rpm: Option<f32>,
    axis_deg: Option<f32>,
    /// Generate plausible ball data for this club (see `GET /v1/clubs`)
    /// instead of raw numbers above. `speed_mph`/`vla_deg`/`spin_rpm` are
    /// ignored when this is set; `hla_deg`/`axis_deg` still apply as shot
    /// shape.
    club: Option<String>,
    /// Target carry in yards for `club`. Defaults to `player`'s bag entry
    /// for that club, or the club's typical carry if neither is given.
    carry_yd: Option<f32>,
    /// Look up `club`'s carry distance from this player's bag (see
    /// `GET /v1/players/{name}`). Ignored if `carry_yd` is also given, or
    /// falls back to the club's reference carry if the player has no entry
    /// for it.
    player: Option<String>,
}

/// Fires a synthetic shot into the current session, as if a device had just
/// measured one. Only works on a `kind: "simulated"` session -- real
/// hardware reports its own shots, it can't be told to fake one.
async fn fire_shot(State(state): State<AppState>, Json(body): Json<ShotBody>) -> impl IntoResponse {
    match state.snapshot().await {
        None => {
            return err(
                404,
                "no_session",
                "no launch monitor is connected. POST /v1/session to connect",
            )
        }
        Some((device, ..)) if device.kind != "simulated" => {
            return err(
                422,
                "not_simulated",
                "the connected device is real hardware and reports its own shots; POST /v1/session with kind: \"simulated\" first to fire synthetic ones",
            )
        }
        Some(_) => {}
    }

    let (speed_mph, vla_deg, spin_rpm) = if let Some(club) = &body.club {
        let Some(profile) = trakr_core::club_profile(club) else {
            let known: Vec<&str> = trakr_core::CLUB_PROFILES.iter().map(|p| p.club).collect();
            return err(
                422,
                "unknown_club",
                &format!("unknown club \"{club}\"; known clubs: {}", known.join(", ")),
            );
        };
        let carry_yd = match body.carry_yd {
            Some(c) => c,
            None => {
                let from_player = match &body.player {
                    Some(name) => state.get_player(name).await.and_then(|p| p.carry_for(club)),
                    None => None,
                };
                from_player.unwrap_or(profile.carry_yd)
            }
        };
        let speed = profile.ball_speed_mph * (carry_yd / profile.carry_yd).sqrt();
        (speed, profile.vla_deg, profile.spin_rpm)
    } else {
        (
            body.speed_mph.unwrap_or(150.0),
            body.vla_deg.unwrap_or(12.5),
            body.spin_rpm.unwrap_or(2800.0),
        )
    };
    let hla_deg = body.hla_deg.unwrap_or(1.0);
    let axis_deg = body.axis_deg.unwrap_or(-3.0);

    let shot = Shot {
        sequence: 0, // the driver assigns the real one
        ball: BallData {
            speed_mps: speed_mph / 2.236_936,
            speed_conf: Confidence::CERTAIN,
            launch_angle_deg: vla_deg,
            launch_angle_conf: Confidence::CERTAIN,
            horizontal_angle_deg: hla_deg,
            horizontal_angle_conf: Confidence::CERTAIN,
            total_spin_rpm: Some(spin_rpm),
            back_spin_rpm: None,
            side_spin_rpm: None,
            spin_axis_deg: Some(axis_deg),
            spin_conf: Confidence::CERTAIN,
        },
        club: None,
        flight: None,
        valid: true,
    };
    command_response(&state, Command::FireShot(shot)).await
}

async fn list_clubs() -> impl IntoResponse {
    Json(json!({ "clubs": trakr_core::CLUB_PROFILES })).into_response()
}

async fn list_players(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({ "players": state.list_players().await })).into_response()
}

#[derive(serde::Deserialize)]
struct CreatePlayerBody {
    name: String,
    #[serde(default = "default_true")]
    right_handed: bool,
}
fn default_true() -> bool {
    true
}

async fn create_player(
    State(state): State<AppState>,
    Json(body): Json<CreatePlayerBody>,
) -> impl IntoResponse {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return err(422, "invalid_name", "name must not be empty");
    }
    match state.create_player(name, body.right_handed).await {
        Ok(player) => (axum::http::StatusCode::CREATED, Json(player)).into_response(),
        Err("player_exists") => err(
            409,
            "player_exists",
            "a player with this name already exists (case-insensitive). PUT /v1/players/{name}/clubs/{club} to update their bag",
        ),
        Err(other) => err(500, "create_failed", other),
    }
}

async fn get_player(State(state): State<AppState>, Path(name): Path<String>) -> impl IntoResponse {
    match state.get_player(&name).await {
        Some(player) => Json(player).into_response(),
        None => err(
            404,
            "no_such_player",
            "no player with this name. GET /v1/players to list known players",
        ),
    }
}

async fn delete_player(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let removed = state.delete_player(&name).await;
    Json(json!({ "deleted": removed })).into_response()
}

#[derive(serde::Deserialize)]
struct ClubCarryBody {
    carry_yd: f32,
}

async fn set_club(
    State(state): State<AppState>,
    Path((name, club)): Path<(String, String)>,
    Json(body): Json<ClubCarryBody>,
) -> impl IntoResponse {
    match state.set_club_carry(&name, &club, body.carry_yd).await {
        Ok(player) => Json(player).into_response(),
        Err("no_such_player") => err(
            404,
            "no_such_player",
            "no player with this name. POST /v1/players to create one",
        ),
        Err(other) => err(500, "update_failed", other),
    }
}

async fn remove_club(
    State(state): State<AppState>,
    Path((name, club)): Path<(String, String)>,
) -> impl IntoResponse {
    match state.remove_club(&name, &club).await {
        Ok(player) => Json(player).into_response(),
        Err("no_such_player") => err(404, "no_such_player", "no player with this name"),
        Err(other) => err(500, "update_failed", other),
    }
}

async fn get_settings(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.chip_settings()).into_response()
}

async fn post_settings(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(obj) = body.as_object() else {
        return err(422, "invalid_body", "expected a JSON object");
    };
    let chip_on_lob_wedge = match obj.get("chip_on_lob_wedge") {
        None => None,
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(_) => return err(422, "invalid_chip_on_lob_wedge", "must be a boolean"),
    };
    // Present-and-null means "clear it"; absent means "leave unchanged" --
    // that distinction is why this isn't just Option<f32>.
    let force_chip_distance_yd = match obj.get("force_chip_distance_yd") {
        None => None,
        Some(serde_json::Value::Null) => Some(None),
        Some(v) => match v.as_f64() {
            Some(n) => Some(Some(n as f32)),
            None => {
                return err(
                    422,
                    "invalid_force_chip_distance_yd",
                    "must be a number of yards, or null to disable",
                )
            }
        },
    };
    let chip_via_putting = match obj.get("chip_via_putting") {
        None => None,
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(_) => return err(422, "invalid_chip_via_putting", "must be a boolean"),
    };
    let updated = state
        .update_chip_settings(chip_on_lob_wedge, force_chip_distance_yd, chip_via_putting)
        .await;
    Json(updated).into_response()
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
