//! The LiveReplay HTTP API: types shared by the desktop client, the server and the web viewer.
//!
//! Endpoints (JSON unless noted; `K` = needs the uploader's key in [`KEY_HEADER`]):
//! - `POST /api/battles` K: [`NewBattle`] -> [`BattleInfo`]
//! - `POST /api/battles/{id}/frames?reset=1&finish=1&damage=N` K: body = battle frames (binary),
//!   appended (or replacing everything with `reset`) -> [`BattleInfo`]
//! - `GET /api/battles?player=&page=&group=` -> [`BattleList`] (finished battles, newest first;
//!   `group`: every upload of one battle)
//! - `GET /api/battles/{id}` -> [`BattleInfo`]
//! - `GET /api/battles/{id}/data?gen=G&from=N` -> frames from byte N (binary). Live battles need a
//!   viewing code ([`CODES_HEADER`]) or the owner's key. If `gen` is not current, everything is
//!   sent again; the current generation is in [`GEN_HEADER`].
//! - `GET /api/live` with [`CODES_HEADER`] -> [`LiveList`]
//! - `POST /api/codes` K: [`NewCode`] -> [`CodeInfo`];  `DELETE /api/codes/{code}` K
//!   A code shows the live battles uploaded with the key that created it. The server stores only
//!   a hash of each code, so the code itself says nothing about whose it is.
//! - `PUT /api/battles/{id}/replay` K: the original .wotreplay file (kept private: never served to
//!   others; `GET` with the owner's key downloads it)
//! - `GET /api/me` K -> [`MyData`];  `DELETE /api/me` K: delete everything of this uploader;
//!   `DELETE /api/battles/{id}` K: delete one battle (GDPR access / erasure)
//! - `DELETE /api/admin/players/{name}` with [`ADMIN_HEADER`]: remove every battle a player is in
//!   (removal requests from players who didn't upload)
//! - `GET /privacy`: privacy policy
//! - `GET /api/assets/{name}` / `PUT /api/assets/{name}?version=V` K: game data the web viewer
//!   needs (map packs, icons), uploaded by clients — never part of the repository.

use serde::{Deserialize, Serialize};

/// The uploader's secret key (created by the desktop client, first use claims the player name).
pub const KEY_HEADER: &str = "X-Livereplay-Key";
/// Comma-separated live viewing codes the browser has saved.
pub const CODES_HEADER: &str = "X-Livereplay-Codes";
/// Generation of a battle's data (changes when the uploader replaced it).
pub const GEN_HEADER: &str = "X-Livereplay-Gen";
/// Version of an asset (client version it came from).
pub const VERSION_HEADER: &str = "X-Livereplay-Version";

/// Longest validity of a live viewing code (days).
pub const MAX_CODE_DAYS: u32 = 31;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NewBattle {
    pub player: String,
    /// Server cluster, e.g. "EU".
    pub realm: String,
    /// Map id (e.g. `10_hills`) and display name.
    pub map: String,
    pub map_display: String,
    pub mode: String,
    pub tank: String,
    /// As written by the game: "dd.mm.yyyy hh:mm:ss".
    pub date: String,
    /// Game client version (map assets are keyed by it).
    pub version: String,
    /// In progress (live) or a finished replay.
    pub live: bool,
    /// Original replay file name (finished replays), to avoid duplicates per player.
    pub file_name: String,
    /// Tank (entity) ids known at upload: recordings of the same battle share them, so the
    /// server can group several players' uploads of one battle.
    pub vehicle_ids: Vec<i32>,
    /// Names of all players in the battle, so a player can have every battle with their name removed.
    pub players: Vec<String>,
}

/// Everything the server keeps about one uploader (`GET /api/me`, GDPR access / portability).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MyData {
    /// Player names this uploader key has claimed.
    pub player_names: Vec<String>,
    pub battles: Vec<BattleInfo>,
    /// Expiry (Unix seconds) of each active viewing code (codes themselves are only stored hashed).
    pub code_expiries: Vec<u64>,
}

/// Header with the operator's admin token (`ADMIN_TOKEN` on the server) for removal requests.
pub const ADMIN_HEADER: &str = "X-Livereplay-Admin";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BattleInfo {
    pub id: String,
    pub player: String,
    pub realm: String,
    pub map: String,
    pub map_display: String,
    pub mode: String,
    pub tank: String,
    pub date: String,
    pub version: String,
    pub file_name: String,
    /// Still being played (only visible with a viewing code until finished).
    pub live: bool,
    pub damage: i64,
    /// Bytes of battle data.
    pub size: u64,
    pub gen: u32,
    /// Unix seconds of the last upload.
    pub updated: u64,
    /// Size of the stored original replay file (0 = none).
    pub replay_size: u64,
    /// Same battle uploaded by several players: all of them share this group id (empty = alone).
    pub group: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BattleList {
    pub battles: Vec<BattleInfo>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveList {
    /// Live battles of the players whose codes were given.
    pub battles: Vec<BattleInfo>,
    /// Which of the given codes are still valid (the browser forgets the others), with the player
    /// name of the code owner's latest battle.
    pub valid: Vec<CodeStatus>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeStatus {
    pub code: String,
    pub player: String,
    pub expires: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NewCode {
    pub days: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeInfo {
    pub code: String,
    /// Unix seconds.
    pub expires: u64,
}

/// Canonical form of a typed code: upper case, only the code alphabet (dashes/spaces dropped).
pub fn normalize_code(c: &str) -> String {
    c.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Asset names: `map-<mapid>` (map pack) and `icons`.
pub fn map_asset(map: &str) -> String {
    format!("map-{map}")
}

pub fn valid_asset_name(n: &str) -> bool {
    !n.is_empty() && n.len() < 80 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
