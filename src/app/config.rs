//! Desktop app settings: where the game and its replays are.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// World of Tanks install folder.
    pub game_dir: String,
    /// Folder with the .wotreplay files.
    pub replays_dir: String,
    /// Linux only: Bottles bottle the game runs in (for "Watch in game").
    pub bottle: String,
    /// Upload the battles played to the LiveReplay server (off unless the user turns it on).
    pub upload_live: bool,
    /// Also the map's 3D data while the battle is played (several MB); off: after the battle.
    /// (The battle itself is always uploaded live.)
    pub upload_3d_during_battle: bool,
    /// The user agreed to what uploading means (shown once, before the first upload).
    pub upload_consent: bool,
    /// The first-start question about uploading was answered (either way).
    pub upload_asked: bool,
    /// Secret that identifies this PC to the server (the first upload for a player name claims it).
    pub upload_key: String,
    /// Live viewing codes made here (code, expiry in Unix seconds), so they can be shown and revoked.
    pub codes: Vec<(String, u64)>,
    /// Start with the computer, into the tray. Off by default: an unsigned program that puts
    /// itself into autostart unasked is flagged by Windows Defender (Bearfoos.A!ml).
    pub autostart: bool,
    /// Windows: the window's close button keeps LiveReplay running in the tray.
    pub close_to_tray: bool,
    /// Look for new versions and say so.
    pub update_notify: bool,
    /// Windows: install new versions by itself (used from the next start).
    pub auto_update: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            game_dir: String::new(),
            replays_dir: String::new(),
            bottle: String::new(),
            upload_live: false,
            upload_3d_during_battle: false,
            upload_consent: false,
            upload_asked: false,
            upload_key: String::new(),
            codes: Vec::new(),
            autostart: false,
            close_to_tray: true,
            update_notify: true,
            auto_update: true,
        }
    }
}

fn path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("config.json"))
}

impl Config {
    /// Saved settings, with empty paths filled in from the detected game install.
    pub fn load() -> Self {
        let mut c: Config = Self::read_saved().unwrap_or_else(Self::from_old_ini);
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

    /// The saved settings. Several app windows share the file: a read can meet another one's write,
    /// so failures are retried. A file that stays unreadable is kept aside (`config.json.broken`)
    /// rather than overwritten, so its upload key isn't lost without notice.
    fn read_saved() -> Option<Self> {
        let p = path()?;
        if !p.exists() {
            return None;
        }
        for _ in 0..20 {
            if let Some(c) = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str(&s).ok()) {
                return Some(c);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = std::fs::rename(&p, p.with_extension("json.broken"));
        None
    }

    /// Written to a temporary file and renamed over the old one: readers never see half a file.
    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let tmp = p.with_extension(format!("json.{}.tmp", std::process::id()));
            if std::fs::write(&tmp, s).is_ok() && std::fs::rename(&tmp, &p).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
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
