//! Persisted `ChipSettings`. Loaded once at startup and rewritten on every
//! change via `POST /v1/settings`, so a value set through the UI or the API
//! survives a daemon/app restart. CLI flags / env vars only seed the value
//! the very first time this file doesn't exist yet -- after that, whatever
//! was last saved here wins, which is what "persistent settings" means.
//!
//! Platform notes (verified against the `dirs` crate's source, not assumed):
//! Linux (`$XDG_CONFIG_HOME` or `~/.config`), Windows (`%APPDATA%`), and
//! macOS (`~/Library/Application Support`) all resolve correctly. iOS shares
//! macOS's implementation and would land in the app's own sandboxed
//! Application Support directory -- the right place for it -- but this repo
//! has no iOS build target set up yet, so that's unverified, not broken.
//! Android doesn't reliably expose `HOME`, so `dirs::config_dir()` can
//! return `None` there; we fall back to the OS temp directory below so this
//! never panics, but a temp dir isn't guaranteed to survive between runs, so
//! persistence would be unreliable if an Android target is ever added.

use std::path::PathBuf;

use trakr_core::ChipSettings;

fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("trakr")
        .join("settings.json")
}

/// Returns `None` if there's no settings file yet, or it's unreadable.
pub fn load() -> Option<ChipSettings> {
    let path = settings_path();
    let data = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&data) {
        Ok(settings) => {
            tracing::info!(?path, "loaded persisted settings");
            Some(settings)
        }
        Err(e) => {
            tracing::warn!(?path, error = %e, "settings file is unreadable, ignoring it");
            None
        }
    }
}

/// Best-effort: a failed save is logged, not fatal -- the setting still
/// takes effect for the running process, it just won't survive a restart.
pub fn save(settings: &ChipSettings) {
    let path = settings_path();
    let result: std::io::Result<()> = (|| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(settings)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    match result {
        Ok(()) => tracing::debug!(?path, "saved settings"),
        Err(e) => tracing::warn!(?path, error = %e, "could not save settings"),
    }
}
