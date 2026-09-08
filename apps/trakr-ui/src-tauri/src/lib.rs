//! trakr-ui: the desktop shell. It embeds the trakr daemon (same one `trakr
//! serve` runs) and starts it in the background; the frontend is a plain
//! HTML/JS page that talks to it exactly the way the CLI or an agent would --
//! over the documented HTTP + SSE API at http://127.0.0.1:7011. See
//! docs/api.md. There is no Tauri IPC command surface here on purpose: the
//! UI is a thin client over the API, not a second implementation of it.

use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|_app| {
            tauri::async_runtime::spawn(async {
                let bind: SocketAddr = std::env::var("TRAKR_BIND")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| "127.0.0.1:7011".parse().unwrap());
                let sim_host = std::env::var("TRAKR_SIM_HOST").unwrap_or_else(|_| "127.0.0.1".into());
                let sim_port: u16 = std::env::var("TRAKR_SIM_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(921);
                let sim = trakr_openconnect::Config { host: sim_host, port: sim_port, ..Default::default() };
                tracing::info!(%bind, sim_host = %sim.host, sim_port = sim.port, "starting embedded trakr daemon");
                if let Err(e) = trakr_daemon::serve(bind, sim).await {
                    tracing::error!(error = %e, "embedded daemon failed to start -- is `trakr serve` already running elsewhere on this port?");
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
