//! Persisted player roster (name, handedness, bag of club carry distances).
//! Same pattern as `settings.rs`: loaded once at startup, rewritten on every
//! change via the players API, so it survives a daemon/app restart.

use std::path::PathBuf;

use trakr_core::Player;

fn players_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("trakr")
        .join("players.json")
}

/// Empty if there's no file yet, or it's unreadable.
pub fn load() -> Vec<Player> {
    let path = players_path();
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match serde_json::from_str(&data) {
        Ok(players) => {
            tracing::info!(?path, "loaded persisted players");
            players
        }
        Err(e) => {
            tracing::warn!(?path, error = %e, "players file is unreadable, ignoring it");
            Vec::new()
        }
    }
}

/// Best-effort: a failed save is logged, not fatal -- the roster still
/// reflects the change for the running process, it just won't survive a
/// restart.
pub fn save(players: &[Player]) {
    let path = players_path();
    let result: std::io::Result<()> = (|| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(players)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    match result {
        Ok(()) => tracing::debug!(?path, "saved players"),
        Err(e) => tracing::warn!(?path, error = %e, "could not save players"),
    }
}
