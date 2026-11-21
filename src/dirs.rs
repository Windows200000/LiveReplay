//! Where LiveReplay keeps its files (desktop only).

use std::path::PathBuf;

fn project() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "LiveReplay")
}

/// Settings and the replay index (`LIVEREPLAY_HOME`: somewhere else, for tests).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("LIVEREPLAY_HOME") {
        return Some(PathBuf::from(home).join("config"));
    }
    project().map(|p| p.config_dir().to_path_buf())
}

/// Rebuildable data (3D caches).
pub fn cache_dir() -> Option<PathBuf> {
    project().map(|p| p.cache_dir().to_path_buf())
}

/// One-time move of the folders used before the rename, so settings and caches carry over.
pub fn migrate_old_folders() {
    let (Some(old), Some(new)) = (directories::ProjectDirs::from("com", "wot", "wot-replay-manager"), project()) else { return };
    for (from, to) in [(old.config_dir(), new.config_dir()), (old.cache_dir(), new.cache_dir())] {
        if from.exists() && !to.exists() {
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::rename(from, to);
        }
    }
}
