//! trakr-daemon: the long-running process that owns a launch monitor session
//! and exposes it over HTTP + Server-Sent Events. This is the single source
//! of truth both the `trakr` CLI and the future Tauri UI talk to — anything
//! either of them can do, an AI agent can do with the same HTTP calls. The
//! API is documented at `GET /v1/openapi.json` (also in `docs/api.md`).

mod api;
mod settings;
mod state;

pub use state::{AppState, ConnectRequest};

use std::net::SocketAddr;

use trakr_core::ChipSettings;

/// Run the daemon until the process is killed. `sim` configures the GSPro
/// Open Connect output (host/port of the simulator, e.g. Muni Golf Sim).
/// `initial_chip_settings` only seeds the very first run on a machine --
/// after that, settings saved via `POST /v1/settings` (or the UI) persist
/// across restarts and take precedence.
pub async fn serve(
    bind: SocketAddr,
    sim: trakr_openconnect::Config,
    initial_chip_settings: ChipSettings,
) -> anyhow::Result<()> {
    let state = AppState::new(sim, initial_chip_settings);
    let app = api::router(state);
    tracing::info!(%bind, "trakr daemon listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
