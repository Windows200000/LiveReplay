//! The replay library: one summary line per .wotreplay in the replays folder, kept in a small
//! JSON index (keyed by file size + modification time) so only new or changed files are read.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::gamedata::GameData;
use crate::replay::file::ReplayFile;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayEntry {
    pub path: String,
    pub player_name: String,
    pub tank: String,
    pub map: String,
    /// Battle type and game mode, e.g. "Random · Standard".
    pub mode: String,
    /// As written by the game: "dd.mm.yyyy hh:mm:ss".
    pub date: String,
    pub damage: i64,
    pub server: String,
    pub version: String,
}

impl ReplayEntry {
    /// Sortable form of the date ("yyyymmddhhmmss").
    pub fn date_key(&self) -> String {
        let (day, time) = self.date.split_once(' ').unwrap_or((&self.date, ""));
        match day.split('.').collect::<Vec<_>>()[..] {
            [d, m, y] => format!("{y}{m:0>2}{d:0>2}{}", time.replace(':', "")),
            _ => self.date.clone(),
        }
    }
}

/// Arena bonus type + gameplay id as shown in the Mode column. Names come from the client's own
/// texts ("Skirmish", "Encounter Battle", ...) when the game is available, else a built-in table.
pub fn mode_label(bonus_type: i64, gameplay: &str, game: Option<&GameData>) -> String {
    let client = |key: String| game.and_then(|g| g.tr(&key)).filter(|v| !v.is_empty() && !v.starts_with('#'));
    if let Some(kind) = client(format!("#menu:bonusType/{bonus_type}")) {
        let kind = kind.trim_end_matches(" Battle").to_string();
        return match (gameplay, client(format!("#arenas:type/{gameplay}/name"))) {
            ("", _) => kind,
            (_, Some(mode)) => format!("{kind} · {}", mode.trim_end_matches(" Battle")),
            (g, None) => format!("{kind} · {g}"),
        };
    }
    let kind: String = match bonus_type {
        1 => "Random".into(),
        2 => "Training".into(),
        3 => "Tank Company".into(),
        4 | 14 => "Tournament".into(),
        5 => "Clan Wars".into(),
        7 => "Team Battle".into(),
        10 | 32 => "Event".into(),
        13 => "Global Map".into(),
        18 => "Skirmish".into(),
        19 => "Advance".into(),
        20 => "Detachments".into(),
        22 => "Ranked".into(),
        24 => "Grand Battle".into(),
        27 => "Frontline".into(),
        29 | 30 => "Steel Hunter".into(),
        43 => "Onslaught".into(),
        n => format!("Type {n}"),
    };
    let mode = match gameplay {
        "" => return kind,
        g if g.starts_with("ctf") => "Standard",
        g if g.starts_with("domination") => "Encounter",
        g if g.starts_with("assault") => "Assault",
        g => g,
    };
    format!("{kind} · {mode}")
}

/// Read the summary of one replay (start block, plus the results block for damage if present).
/// Summary of the battle being played (`temp.wotreplay` has no JSON header yet): taken from the
/// parsed packets; the date is when the game created the file (the battle's start).
pub fn live_entry(path: &Path, b: &crate::replay::battle::Battle, game: &GameData) -> ReplayEntry {
    let me = b.recorder_vehicle.and_then(|id| b.vehicles.get(&id));
    // The battle id's low 32 bits are its start (Unix seconds); else when the file was made.
    let from_id = b.arena_id & 0xffff_ffff;
    let started = if (1_500_000_000..4_000_000_000).contains(&from_id) {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(from_id)
    } else {
        std::fs::metadata(path).and_then(|m| m.created().or_else(|_| m.modified())).unwrap_or_else(|_| std::time::SystemTime::now())
    };
    ReplayEntry {
        path: path.to_string_lossy().into_owned(),
        player_name: b.player_name.clone(),
        tank: me.map(|v| v.tank_name.clone()).unwrap_or_default(),
        map: b.map_display.clone(),
        mode: mode_label(b.battle_type, &b.gameplay, Some(game)),
        date: local_date(started),
        damage: 0,
        server: b.server_realm.clone(),
        version: game.client_version(),
    }
}

/// "dd.mm.yyyy hh:mm:ss" in local time, like the game writes it.
fn local_date(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + local_offset_secs();
    let (days, s) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{d:02}.{m:02}.{y} {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(windows)]
fn local_offset_secs() -> i64 {
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: [u16; 8],
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: [u16; 8],
        daylight_bias: i32,
    }
    extern "system" {
        fn GetTimeZoneInformation(info: *mut TimeZoneInformation) -> u32;
    }
    // SAFETY: plain C struct filled in by kernel32.
    let mut tz: TimeZoneInformation = unsafe { std::mem::zeroed() };
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = tz.bias + if r == 2 { tz.daylight_bias } else { tz.standard_bias };
    -(bias as i64) * 60
}

#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

pub fn summarize(path: &Path, game: Option<&GameData>) -> Option<ReplayEntry> {
    let file = ReplayFile::open(path).ok()?;
    let info = file.start_info();
    let text = |k: &str| info.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let player_name = text("playerName");
    if player_name.is_empty() {
        return None;
    }
    // "germany-G42_Maus" -> the client's display name when the game files are available.
    let vehicle = text("playerVehicle");
    let tank = vehicle
        .split_once('-')
        .and_then(|(nation, tag)| game?.vehicle(&format!("{nation}:{tag}")))
        .map(|v| v.name)
        .unwrap_or(vehicle);
    let damage = info.get("damageDealt").and_then(|v| v.as_i64()).filter(|&d| d > 0).or_else(|| {
        let personal = file.results()?.get(0)?.get("personal")?.as_object()?.values().next()?.get("damageDealt")?.as_i64();
        personal
    });
    // Left before the end (no results block): the game's cached results, if archived.
    let damage = damage.or_else(|| {
        if file.results().is_some() {
            return None;
        }
        let vehicles = info.get("vehicles")?.as_object()?;
        let ids: Vec<i32> = vehicles.keys().filter_map(|k| k.parse().ok()).collect();
        let me = vehicles.iter().find(|(_, v)| v.get("name").and_then(|n| n.as_str()) == Some(player_name.as_str()))?.0;
        crate::replay::cached_results::results_for(&ids)?.get(0)?.get("vehicles")?.get(me)?.get(0)?.get("damageDealt")?.as_i64()
    });
    Some(ReplayEntry {
        path: path.to_string_lossy().into_owned(),
        player_name,
        tank,
        map: text("mapDisplayName"),
        mode: mode_label(info.get("battleType").and_then(|v| v.as_i64()).unwrap_or(0), &text("gameplayID"), game),
        date: text("dateTime"),
        damage: damage.unwrap_or(0),
        server: text("serverName"),
        version: text("clientVersionFromXml"),
    })
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    /// file name -> (size, modified seconds, summary)
    files: HashMap<String, (u64, u64, ReplayEntry)>,
}

const INDEX_VERSION: u32 = 2;

fn index_path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join(format!("library-v{INDEX_VERSION}.json")))
}

fn load_index() -> Index {
    index_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Entries known from the last scan (instant, before the folder is re-checked).
pub fn cached() -> Vec<ReplayEntry> {
    load_index().files.into_values().map(|(_, _, e)| e).collect()
}

pub enum ScanEvent {
    Progress { done: usize, total: usize },
    Finished(Vec<ReplayEntry>),
}

/// Re-check the replays folder in the background: read new/changed files, forget deleted ones.
pub fn scan(dir: PathBuf, game_dir: String) -> mpsc::Receiver<ScanEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let game = crate::gamedata::find_game_dir(&game_dir).map(GameData::new);
        // Keep the game's cached battle results (it deletes old ones) and learn their layout.
        let new_results = crate::replay::cached_results::archive_new();
        crate::replay::cached_results::learn_from_folder(&dir);
        let mut old = load_index();
        if new_results > 0 {
            // Replays left early may now have results: read those again.
            old.files.retain(|_, (_, _, e)| e.damage > 0);
        }
        let mut files: Vec<(String, PathBuf, u64, u64)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "wotreplay"))
            // The battle being played (shown separately at the top of the list).
            .filter(|e| e.file_name() != "temp.wotreplay")
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
                Some((e.file_name().to_string_lossy().into_owned(), e.path(), meta.len(), modified))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let total = files.len();
        let mut index = Index::default();
        for (i, (name, path, size, modified)) in files.into_iter().enumerate() {
            let known = old.files.remove(&name).filter(|(s, m, _)| *s == size && *m == modified);
            let entry = match known {
                Some((_, _, e)) => Some(e),
                None => {
                    let _ = tx.send(ScanEvent::Progress { done: i, total });
                    summarize(&path, game.as_ref())
                }
            };
            if let Some(e) = entry {
                index.files.insert(name, (size, modified, e));
            }
        }
        if let Some(p) = index_path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Ok(b) = serde_json::to_vec(&index) {
                let _ = std::fs::write(p, b);
            }
        }
        let _ = tx.send(ScanEvent::Finished(index.files.into_values().map(|(_, _, e)| e).collect()));
    });
    rx
}
