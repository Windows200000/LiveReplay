//! Desktop app settings: where the game and its replays are.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// World of Tanks install folder.
    pub game_dir: String,
    /// Folder with the .wotreplay files.
    pub replays_dir: String,
    /// Linux only: Bottles bottle the game runs in (for "Watch in game").
    pub bottle: String,
    /// Upload the battle being played to the LiveReplay server (off unless the user turns it on).
    pub upload_live: bool,
    /// The user agreed to what uploading means (shown once, before the first upload).
    pub upload_consent: bool,
    /// Secret that identifies this PC to the server (the first upload for a player name claims it).
    pub upload_key: String,
    /// Live viewing codes made here (code, expiry in Unix seconds), so they can be shown and revoked.
    pub codes: Vec<(String, u64)>,
}

fn path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("config.json"))
}

impl Config {
    /// Saved settings, with empty paths filled in from the detected game install.
    pub fn load() -> Self {
        let mut c: Config = path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(Self::from_old_ini);
        if let Some(dir) = crate::gamedata::find_game_dir(&c.game_dir) {
            if c.game_dir.is_empty() {
                c.game_dir = dir.to_string_lossy().into_owned();
            }
            if c.replays_dir.is_empty() {
                c.replays_dir = dir.join("replays").to_string_lossy().into_owned();
            }
        }
        if c.upload_key.is_empty() {
            c.upload_key = super::upload::new_key();
            c.save();
        }
        c
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }

    /// Settings written by versions before the rename (`config.ini`, `key=value` lines).
    fn from_old_ini() -> Self {
        let mut c = Config::default();
        let Some(text) = crate::dirs::config_dir().and_then(|d| std::fs::read_to_string(d.join("config.ini")).ok()) else { return c };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim().to_string();
            match k.trim() {
                // The old setting pointed at the game's exe; we keep its folder.
                "executable_path" => c.game_dir = std::path::Path::new(&v).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                "replays_path" => c.replays_dir = v,
                "bottle_name" => c.bottle = v,
                _ => {}
            }
        }
        c
    }
}
