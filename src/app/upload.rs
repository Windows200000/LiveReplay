//! Uploads to the LiveReplay server: the battle being played (live, a few seconds at a time, only
//! new data), finished replays on request, and the game data the web viewer needs for a map
//! (once per map and client version). Everything is decoded here; the server only stores it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::config::Config;
use crate::net::api::{self, BattleInfo, CodeInfo, NewBattle, NewCode};
use crate::replay::battle::Battle;
use crate::replay::wire;
use crate::viewer::player::native::{get_engine, Engine};

/// How often the live battle is sent, and how much of the previous part is sent again
/// (shots and hits are completed a little after they start).
const LIVE_EVERY: Duration = Duration::from_secs(2);
const LIVE_OVERLAP: f32 = 2.0;
/// After a live battle the game writes the final replay; wait this long for it.
const FINAL_WAIT: Duration = Duration::from_secs(120);
/// `temp.wotreplay` not growing for this long: the battle was interrupted (game crash,
/// disconnect); what was recorded is uploaded as such.
const INTERRUPTED_AFTER: Duration = Duration::from_secs(300);

/// `INTERRUPTED_AFTER`, shorter for tests (`LIVEREPLAY_INTERRUPTED_SECS`).
fn interrupted_after() -> Duration {
    std::env::var("LIVEREPLAY_INTERRUPTED_SECS").ok().and_then(|s| s.parse().ok()).map(Duration::from_secs).unwrap_or(INTERRUPTED_AFTER)
}
/// Uploads without battle results are sent again once the game's results show up, for this long.
const RESULTS_WAIT_SECS: u64 = 24 * 3600;

/// An uploaded battle still without results.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Pending {
    path: PathBuf,
    id: String,
    ids: Vec<i32>,
    interrupted: bool,
    since: u64,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn pending_path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("pending_results.json"))
}

fn load_pending() -> Vec<Pending> {
    pending_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_pending(v: &[Pending]) {
    if let (Some(p), Ok(b)) = (pending_path(), serde_json::to_vec(v)) {
        let _ = std::fs::write(p, b);
    }
}

/// A battle being played when the app was last running: (Unix seconds it started).
fn marker_path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("battle_in_progress.json"))
}

fn set_marker(started: SystemTime) {
    let secs = started.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Some(p) = marker_path() {
        let _ = std::fs::write(p, secs.to_string());
    }
}

fn clear_marker() {
    if let Some(p) = marker_path() {
        let _ = std::fs::remove_file(p);
    }
}

fn read_marker() -> Option<u64> {
    std::fs::read_to_string(marker_path()?).ok()?.trim().parse().ok()
}

/// Where recordings of interrupted battles are kept (the game reuses its temp file).
fn interrupted_dir() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("interrupted"))
}

/// A complete .wotreplay (with header), not a recording in progress.
fn full_replay(raw: &[u8]) -> bool {
    raw.len() > 12 && raw[..4] == [0x12, 0x32, 0x34, 0x11]
}

pub fn server_url() -> String {
    std::env::var("LIVEREPLAY_SERVER").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| crate::net::SERVER_URL.to_string()).trim_end_matches('/').to_string()
}

pub fn battle_link(id: &str) -> String {
    format!("{}/b/{id}", server_url())
}

/// New random uploader key (stays on this PC; whoever has it can upload as you).
pub fn new_key() -> String {
    let mut b = [0u8; 32];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A failed request: the server answered with an error status (and its message), or it could
/// not be reached.
enum NetErr {
    Status(u16, String),
    Transport,
}

type Resp = ureq::http::Response<ureq::Body>;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(60)))
        // Error statuses come back as responses, so the server's message can be shown.
        .http_status_as_error(false)
        .build()
        .into()
}

/// A response with an error status as [`NetErr::Status`].
fn checked(r: Result<Resp, ureq::Error>) -> Result<Resp, NetErr> {
    let mut r = r.map_err(|_| NetErr::Transport)?;
    let code = r.status().as_u16();
    if code >= 400 {
        let msg = r.body_mut().with_config().limit(4096).read_to_string().unwrap_or_default();
        return Err(NetErr::Status(code, msg));
    }
    Ok(r)
}

fn err(e: NetErr) -> String {
    match e {
        NetErr::Status(code, msg) => format!("server error {code}{}", if msg.is_empty() || msg.len() > 300 { String::new() } else { format!(": {msg}") }),
        // Full details in the hover text of the status line; keep the message short.
        NetErr::Transport => "can't reach wot.arne.my (offline, or the server is down)".to_string(),
    }
}

fn json<R: serde::de::DeserializeOwned>(mut r: Resp) -> Result<R, String> {
    r.body_mut().with_config().limit(64 << 20).read_json().map_err(|e| e.to_string())
}

fn header(r: &Resp, name: &str) -> Option<String> {
    r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

fn post_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(key: &str, path: &str, body: &T) -> Result<R, String> {
    let r = checked(agent().post(&format!("{}{path}", server_url())).header(api::KEY_HEADER, key).send_json(body)).map_err(err)?;
    json(r)
}

fn post_bytes(key: &str, path: &str, body: &[u8]) -> Result<BattleInfo, String> {
    let r = checked(agent().post(&format!("{}{path}", server_url())).header(api::KEY_HEADER, key).header("Content-Type", "application/octet-stream").send(body))
        .map_err(err)?;
    json(r)
}

/// Everything the server keeps about this PC's uploads (GDPR access), as pretty JSON.
pub fn my_data(key: &str) -> Result<String, String> {
    let r = checked(agent().get(&format!("{}/api/me", server_url())).header(api::KEY_HEADER, key).call()).map_err(err)?;
    let v: serde_json::Value = json(r)?;
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

/// Delete everything this PC uploaded (battles, codes, claimed names); returns how many battles.
pub fn delete_my_data(key: &str) -> Result<usize, String> {
    let r = checked(agent().delete(&format!("{}/api/me", server_url())).header(api::KEY_HEADER, key).call()).map_err(err)?;
    let n = json(r)?;
    if let Some(p) = uploads_path() {
        let _ = std::fs::remove_file(p);
    }
    Ok(n)
}

/// The battles on the server that belong to this PC's player names (also those an older key of
/// the same player uploaded), newest first.
pub fn my_battles(key: &str) -> Result<Vec<BattleInfo>, String> {
    let r = checked(agent().get(&format!("{}/api/me", server_url())).header(api::KEY_HEADER, key).call()).map_err(err)?;
    Ok(json::<api::MyData>(r)?.battles)
}

/// Delete one battle from the server by its id.
pub fn delete_battle(key: &str, id: &str) -> Result<(), String> {
    checked(agent().delete(&format!("{}/api/battles/{id}", server_url())).header(api::KEY_HEADER, key).call()).map_err(err)?;
    let mut m = uploaded();
    m.retain(|_, v| v != id);
    if let (Some(p), Ok(b)) = (uploads_path(), serde_json::to_vec(&m)) {
        let _ = std::fs::write(p, b);
    }
    Ok(())
}

/// The server operator's contact (for "this player name belongs to another uploader").
pub fn server_contact() -> Result<String, String> {
    let r = checked(agent().get(&format!("{}/api/info", server_url())).call()).map_err(err)?;
    Ok(json::<api::ServerInfo>(r)?.contact)
}

/// Delete one uploaded replay from the server.
pub fn delete_upload(key: &str, file_name: &str) -> Result<(), String> {
    let mut m = uploaded();
    let Some(id) = m.remove(file_name) else { return Err("not uploaded".into()) };
    checked(agent().delete(&format!("{}/api/battles/{id}", server_url())).header(api::KEY_HEADER, key).call()).map_err(err)?;
    if let (Some(p), Ok(b)) = (uploads_path(), serde_json::to_vec(&m)) {
        let _ = std::fs::write(p, b);
    }
    Ok(())
}

pub fn privacy_url() -> String {
    format!("{}/privacy", server_url())
}

pub fn create_code(key: &str, days: u32) -> Result<CodeInfo, String> {
    post_json(key, "/api/codes", &NewCode { days })
}

pub fn revoke_code(key: &str, code: &str) -> Result<(), String> {
    checked(agent().delete(&format!("{}/api/codes/{}", server_url(), api::normalize_code(code))).header(api::KEY_HEADER, key).call()).map_err(err)?;
    Ok(())
}

/// Replay file name -> battle id on the server.
/// Shared game data this PC voted for, with a vote that counted: name -> (version, hash). Such
/// an asset isn't looked at again until the game version changes.
fn votes_path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("asset-votes.json"))
}

fn load_votes() -> HashMap<String, (String, String)> {
    votes_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn uploads_path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("uploads.json"))
}

pub fn uploaded() -> HashMap<String, String> {
    uploads_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn remember(file_name: &str, id: &str) {
    let mut m = uploaded();
    m.insert(file_name.to_string(), id.to_string());
    if let (Some(p), Ok(b)) = (uploads_path(), serde_json::to_vec(&m)) {
        let _ = std::fs::write(p, b);
    }
}

#[derive(Default, Clone)]
pub struct Status {
    /// One line for the replay list's status area.
    pub line: String,
    /// Queued past replays still to send.
    pub queued: usize,
}

enum Job {
    Replays(Vec<PathBuf>),
    Config(Config),
}

pub struct Uploader {
    jobs: mpsc::Sender<Job>,
    pub status: Arc<Mutex<Status>>,
    live: Arc<AtomicBool>,
}

impl Uploader {
    pub fn start(cfg: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let live = Arc::new(AtomicBool::new(cfg.upload_live));
        let (st, lv) = (status.clone(), live.clone());
        std::thread::spawn(move || Worker { cfg, status: st, live: lv, checked: HashSet::new(), votes: load_votes(), current: None, last_archive: Instant::now(), pending: load_pending() }.run(rx));
        Uploader { jobs: tx, status, live }
    }

    pub fn upload(&self, paths: Vec<PathBuf>) {
        self.status.lock().unwrap().queued += paths.len();
        let _ = self.jobs.send(Job::Replays(paths));
    }

    pub fn set_config(&self, cfg: Config) {
        self.live.store(cfg.upload_live, Ordering::Relaxed);
        let _ = self.jobs.send(Job::Config(cfg));
    }
}

/// Upload replays right now on this thread (command line `--upload`); the web link of each.
pub fn upload_files(cfg: &Config, paths: &[PathBuf]) -> Vec<Result<String, String>> {
    let mut w = Worker { cfg: cfg.clone(), status: Default::default(), live: Default::default(), checked: HashSet::new(), votes: load_votes(), current: None, last_archive: Instant::now(), pending: load_pending() };
    paths.iter().map(|p| w.upload_replay(p, None).map(|id| battle_link(&id))).collect()
}

struct LiveUpload {
    id: Option<String>,
    key: (String, i32),
    sent_until: f32,
    last_len: u64,
    started: SystemTime,
    /// Set when `temp.wotreplay` went away: waiting for the final replay.
    ended: Option<Instant>,
    /// When `temp.wotreplay` last grew.
    changed: Instant,
    /// Already uploaded as interrupted (it stopped growing).
    interrupted: bool,
    /// The battle is over and the server was told (no longer live there).
    finished: bool,
}

struct Worker {
    cfg: Config,
    status: Arc<Mutex<Status>>,
    live: Arc<AtomicBool>,
    /// Assets already checked this session.
    checked: HashSet<String>,
    /// Assets voted for with a vote that counted (see [`votes_path`]).
    votes: HashMap<String, (String, String)>,
    current: Option<LiveUpload>,
    last_archive: Instant,
    /// Uploads waiting for their battle results.
    pending: Vec<Pending>,
}

impl Worker {
    fn say(&self, s: impl Into<String>) {
        self.status.lock().unwrap().line = s.into();
    }

    fn run(mut self, rx: mpsc::Receiver<Job>) {
        if self.live.load(Ordering::Relaxed) {
            if let Err(e) = self.catch_up() {
                self.say(format!("Upload: {e}"));
            }
        }
        loop {
            match rx.recv_timeout(LIVE_EVERY) {
                Ok(Job::Config(c)) => self.cfg = c,
                Ok(Job::Replays(paths)) => {
                    let n = paths.len();
                    let (mut ok, mut failed) = (0, 0);
                    for (i, p) in paths.iter().enumerate() {
                        self.say(format!("Uploading {}/{n}", i + 1));
                        match self.upload_replay(p, None) {
                            Ok(_) => ok += 1,
                            Err(e) => {
                                failed += 1;
                                self.say(format!("Upload failed: {e}"));
                            }
                        }
                        let mut s = self.status.lock().unwrap();
                        s.queued = s.queued.saturating_sub(1);
                    }
                    if failed == 0 {
                        self.say(format!("Uploaded {ok} replay{}", if ok == 1 { "" } else { "s" }));
                    } else {
                        let last = self.status.lock().unwrap().line.clone();
                        self.say(format!("Uploaded {ok}, {failed} failed ({last})"));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            // Archive the game's cached battle results before it deletes them (once a minute).
            if self.last_archive.elapsed() > Duration::from_secs(60) {
                self.last_archive = Instant::now();
                crate::replay::cached_results::archive_new();
                self.retry_pending();
            }
            if self.live.load(Ordering::Relaxed) {
                if let Err(e) = self.live_tick() {
                    self.say(format!("Live upload: {e}"));
                }
            }
        }
    }

    fn engine(&self) -> Result<Arc<Engine>, String> {
        get_engine(None, &self.cfg.game_dir)
    }

    /// What the server serves for an asset: `None` if it has none, else (version, hash).
    fn asset_head(name: &str) -> Result<Option<(String, String)>, NetErr> {
        match checked(agent().head(&format!("{}/api/assets/{name}", server_url())).call()) {
            Ok(r) => Ok(Some((header(&r, api::VERSION_HEADER).unwrap_or_default(), header(&r, api::HASH_HEADER).unwrap_or_default()))),
            Err(NetErr::Status(404, _)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether this PC already voted for this asset with this game version.
    fn voted(&self, name: &str, version: &str) -> bool {
        self.votes.get(name).is_some_and(|(v, _)| v == version)
    }

    /// Give the server our copy of an asset (made from this PC's game files): uploaded if the
    /// server lacks it, otherwise just a vote for it. The server serves the copy most apps agree
    /// on, so garbage someone uploaded first doesn't stay.
    fn sync_asset(&mut self, name: &str, version: &str, head: Option<(String, String)>, body: Vec<u8>) -> Result<(), String> {
        // Data from an older game version than the server's: no vote (it would be out of date).
        if let Some((v, _)) = &head {
            if crate::app::update::newer(v, version) {
                return Ok(());
            }
        }
        let hash = crate::net::sha256_hex(&body);
        let url = format!("{}/api/assets/{name}", server_url());
        let put = |url: &str| checked(agent().put(url).query("version", version).header(api::KEY_HEADER, &self.cfg.upload_key).send(&body[..])).map_err(err);
        let mut res: api::AssetVote = if head.is_none() {
            json(put(&url)?)?
        } else {
            let r = checked(agent().post(&format!("{url}/vote")).query("version", version).query("hash", &hash).header(api::KEY_HEADER, &self.cfg.upload_key).send_empty());
            match r {
                Ok(r) => json(r)?,
                // An older server without votes: it takes the upload (or keeps its copy).
                Err(NetErr::Status(404 | 405, _)) => {
                    let _ = put(&url);
                    api::AssetVote::default()
                }
                Err(e) => return Err(err(e)),
            }
        };
        if res.upload {
            res = json(put(&url)?)?;
        }
        if res.counted {
            self.votes.insert(name.to_string(), (version.to_string(), hash));
            if let (Some(p), Ok(b)) = (votes_path(), serde_json::to_vec(&self.votes)) {
                if let Some(d) = p.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(p, b);
            }
        }
        Ok(())
    }

    fn ensure_assets(&mut self, engine: &Engine, map: &str, version: &str) -> Result<(), String> {
        let version = api::asset_version(version);
        let version = version.as_str();
        for name in [api::map_asset(map), crate::net::ICONS_ASSET.to_string()] {
            if self.voted(&name, version) || !self.checked.insert(name.clone()) {
                continue;
            }
            let head = match Self::asset_head(&name) {
                Ok(h) => h,
                Err(e) => {
                    self.checked.remove(&name);
                    return Err(err(e));
                }
            };
            let body = if name == crate::net::ICONS_ASSET {
                crate::net::encode_pack(&crate::net::icon_pack(&engine.game))
            } else {
                match crate::net::map_pack(&engine.game, map) {
                    Some(p) => crate::net::encode_pack(&p),
                    None => continue,
                }
            };
            if let Err(e) = self.sync_asset(&name, version, head, body) {
                self.checked.remove(&name);
                return Err(e);
            }
        }
        Ok(())
    }

    fn new_battle(&self, engine: &Engine, path: &Path, b: &Battle, live: bool) -> Result<BattleInfo, String> {
        let e = entry(engine, path, b).ok_or("not a replay")?;
        let nb = NewBattle {
            player: e.player_name,
            realm: b.server_realm.clone(),
            map: b.map.clone(),
            map_display: e.map,
            mode: e.mode,
            tank: e.tank,
            date: e.date,
            version: e.version,
            live,
            file_name: if live { String::new() } else { path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() },
            arena_id: b.arena_id,
            vehicle_ids: b.vehicles.keys().copied().collect(),
            players: b.vehicles.values().map(|v| v.name.clone()).filter(|n| !n.is_empty()).collect(),
        };
        post_json(&self.cfg.upload_key, "/api/battles", &nb)
    }

    /// Send a finished replay (complete, replacing whatever the server had for `id`).
    fn upload_replay(&mut self, path: &Path, id: Option<String>) -> Result<String, String> {
        self.upload_replay_as(path, id, false)
    }

    /// `interrupted`: a recording that stopped before the battle ended (shown as such).
    fn upload_replay_as(&mut self, path: &Path, id: Option<String>, interrupted: bool) -> Result<String, String> {
        let engine = self.engine()?;
        let battle = Battle::load(path, &engine.game, &engine.defs).map_err(|e| e.to_string())?;
        let id = match id {
            Some(id) => id,
            None => {
                let info = self.new_battle(&engine, path, &battle, false)?;
                // Complete on the server (the original replay is there): only the 3D data may be missing.
                if info.size > 0 && !info.live && info.replay_size > 0 {
                    remember(&file_name(path), &info.id);
                    let version = super::library::summarize(path, Some(&engine.game)).map(|e| e.version).unwrap_or_default();
                    let _ = self.ensure_assets(&engine, &battle.map, &version);
                    let _ = self.ensure_3d(&engine, &battle, &version);
                    return Ok(info.id);
                }
                info.id
            }
        };
        let version = super::library::summarize(path, Some(&engine.game)).map(|e| e.version).unwrap_or_default();
        self.ensure_assets(&engine, &battle.map, &version)?;
        // The official result; without one yet (left early, interrupted), the damage the recording
        // shows, until the results come.
        let damage = super::library::summarize(path, Some(&engine.game)).map(|e| e.damage).filter(|&d| d > 0).unwrap_or_else(|| {
            battle.recorder_vehicle.map_or(0, |id| crate::viewer::scene::stats_at(&battle, id, battle.duration).damage_dealt as i64)
        });
        let flag = if interrupted { "&interrupted=1" } else { "" };
        post_bytes(&self.cfg.upload_key, &format!("/api/battles/{id}/frames?reset=1&finish=1&damage={damage}{flag}"), &wire::encode_frame(&battle, 0.0))?;
        // The original replay file too (kept private on the server, for features that need more of it later).
        if let Ok(raw) = std::fs::read(path) {
            if full_replay(&raw) {
                checked(agent().put(&format!("{}/api/battles/{id}/replay", server_url())).header(api::KEY_HEADER, &self.cfg.upload_key).send(&raw[..])).map_err(err)?;
            }
        }
        remember(&file_name(path), &id);
        // No battle results yet (left early, interrupted): sent again once the game has them.
        self.pending.retain(|p| p.id != id);
        if battle.vehicles.values().all(|v| v.final_stats.is_none()) {
            self.pending.push(Pending { path: path.to_path_buf(), id: id.clone(), ids: battle.vehicles.keys().copied().collect(), interrupted, since: now_secs() });
        }
        save_pending(&self.pending);
        // 3D data for the web viewer (after the battle only: building it is heavy).
        if let Err(e) = self.ensure_3d(&engine, &battle, &version) {
            self.say(format!("3D data upload: {e}"));
        }
        Ok(id)
    }

    /// Upload the web 3D packs (terrain, map objects for this mode, tanks, map-event prefabs)
    /// the server doesn't have yet for this client version.
    fn ensure_3d(&mut self, engine: &Engine, b: &Battle, version: &str) -> Result<(), String> {
        let version = api::asset_version(version);
        let version = version.as_str();
        use crate::gamedata::webpack as wp;
        let Some(cache) = engine.cache3d.as_ref() else { return Ok(()) };
        let gd = &engine.game;
        let mut wanted: Vec<(String, Box<dyn Fn() -> Option<Vec<u8>> + '_>)> = vec![
            (wp::terrain_asset(&b.map), Box::new(|| cache.terrain(gd, &b.map).map(|t| wp::terrain_pack(&t)))),
            (wp::scene_asset(&b.map, b.gameplay_id), Box::new(|| cache.scene(gd, &b.map, b.gameplay_id).map(|s| wp::scene_pack(&s)))),
        ];
        let mut types: Vec<&String> = b.vehicles.values().map(|v| &v.vehicle_type).filter(|t| !t.is_empty()).collect();
        types.sort();
        types.dedup();
        for vt in types {
            wanted.push((wp::tank_asset(vt), Box::new(move || cache.tank(gd, vt).map(|t| wp::tank_pack(&t)))));
        }
        let mut prefabs: Vec<&String> = b.scenario.iter().map(|s| &s.prefab).collect();
        prefabs.sort();
        prefabs.dedup();
        for pf in prefabs {
            wanted.push((wp::prefab_asset(pf), Box::new(move || cache.prefab(gd, pf).map(|m| wp::mesh_pack(&m)))));
        }
        // What the server lacks; announced first, so web viewers waiting for it show progress.
        let mut todo = Vec::new();
        for (name, make) in wanted {
            if self.checked.contains(&name) || self.voted(&name, version) {
                continue;
            }
            let head = Self::asset_head(&name).map_err(err)?;
            todo.push((name, make, head));
        }
        // The ones the server lacks are announced first (web viewers waiting for them show it).
        let missing: Vec<&String> = todo.iter().filter(|t| t.2.is_none()).map(|t| &t.0).collect();
        if !missing.is_empty() {
            let _ = agent().post(&format!("{}/api/asset-expect", server_url())).header(api::KEY_HEADER, &self.cfg.upload_key).send_json(&missing);
        }
        // Missing ones first: viewers may be waiting for them.
        todo.sort_by_key(|t| t.2.is_some());
        for (name, make, head) in todo {
            if let Some(body) = make() {
                self.sync_asset(&name, version, head, body)?;
            }
            self.checked.insert(name);
        }
        Ok(())
    }

    fn live_tick(&mut self) -> Result<(), String> {
        let dir = PathBuf::from(&self.cfg.replays_dir);
        let temp = dir.join("temp.wotreplay");
        // After a battle the game leaves the file in place, empty: that counts as no battle.
        match std::fs::metadata(&temp).ok().map(|m| m.len()).filter(|&l| l > 0) {
            Some(len) => {
                if let Some(c) = self.current.as_mut().filter(|c| c.last_len == len && c.ended.is_none()) {
                    // Not growing any more: the game crashed or lost the connection.
                    if !c.interrupted && c.changed.elapsed() > interrupted_after() {
                        c.interrupted = true;
                        let id = c.id.clone();
                        return self.upload_interrupted(&temp, id);
                    }
                    return Ok(());
                }
                let engine = self.engine()?;
                let Ok(battle) = Battle::load(&temp, &engine.game, &engine.defs) else { return Ok(()) };
                let key = (battle.map.clone(), battle.recorder_vehicle.unwrap_or(0));
                if self.current.as_ref().map_or(true, |c| c.key != key || len < c.last_len) {
                    self.finish_previous();
                    self.current = Some(LiveUpload { id: None, key, sent_until: 0.0, last_len: 0, started: SystemTime::now(), ended: None, changed: Instant::now(), interrupted: false, finished: false });
                    set_marker(SystemTime::now());
                }
                if self.current.as_ref().unwrap().id.is_none() {
                    let info = self.new_battle(&engine, &temp, &battle, true)?;
                    let version = entry(&engine, &temp, &battle).map(|e| e.version).unwrap_or_default();
                    self.ensure_assets(&engine, &battle.map, &version)?;
                    self.current.as_mut().unwrap().id = Some(info.id);
                    // The map's 3D data now, if wanted (heavy: its own thread, live updates go on);
                    // otherwise after the battle, with the final upload.
                    if self.cfg.upload_3d_during_battle {
                        let mut w = Worker {
                            cfg: self.cfg.clone(),
                            status: self.status.clone(),
                            live: self.live.clone(),
                            checked: HashSet::new(),
                            votes: load_votes(),
                            current: None,
                            last_archive: Instant::now(),
                            pending: Vec::new(),
                        };
                        let (engine, battle) = (engine.clone(), battle.clone());
                        std::thread::spawn(move || {
                            if let Err(e) = w.ensure_3d(&engine, &battle, &version) {
                                w.say(format!("3D data upload: {e}"));
                            }
                        });
                    }
                }
                let c = self.current.as_mut().unwrap();
                if c.finished {
                    // Over: the full replay follows when the game writes it.
                    c.last_len = len;
                    c.changed = Instant::now();
                    return Ok(());
                }
                let from = if c.sent_until > 0.0 { (c.sent_until - LIVE_OVERLAP).max(0.001) } else { 0.0 };
                let id = c.id.clone().unwrap();
                // The first upload (also after an app restart, onto the same entry) replaces its data.
                let reset = if from == 0.0 { "?reset=1" } else { "" };
                // The battle is over in the recording: no longer live on the website, right away.
                let over = battle.periods.iter().any(|p| p.1 == crate::replay::battle::Period::AfterBattle);
                let finish = if !over { "" } else if reset.is_empty() { "?finish=1" } else { "&finish=1" };
                post_bytes(&self.cfg.upload_key, &format!("/api/battles/{id}/frames{reset}{finish}"), &wire::encode_frame(&battle, from))?;
                c.finished = over;
                c.sent_until = battle.duration;
                c.last_len = len;
                c.changed = Instant::now();
                c.ended = None;
                self.say(format!("Live battle on {} is being uploaded", battle.map_display));
            }
            None => {
                let Some(c) = self.current.as_mut() else { return Ok(()) };
                let ended = *c.ended.get_or_insert_with(Instant::now);
                // The game is back in the garage: the battle is not live any more.
                if let (false, Some(id)) = (c.finished, c.id.clone()) {
                    c.finished = true;
                    let _ = post_bytes(&self.cfg.upload_key, &format!("/api/battles/{id}/frames?finish=1"), &[]);
                }
                // The game writes the replay under its final name when the battle is left.
                let started = c.started;
                let newest = std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "wotreplay"))
                    .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                    .filter(|(m, _)| *m >= started)
                    .max_by_key(|(m, _)| *m)
                    .map(|(_, p)| p);
                if let Some(p) = newest {
                    // Give the game a moment to finish writing it.
                    if ended.elapsed() > Duration::from_secs(5) {
                        let id = c.id.clone();
                        self.current = None;
                        self.say("Uploading the finished battle");
                        self.upload_replay(&p, id)?;
                        clear_marker();
                        self.say("Battle uploaded");
                    }
                } else if ended.elapsed() > FINAL_WAIT {
                    self.finish_previous();
                }
            }
        }
        Ok(())
    }

    /// The battle stopped without its replay (game crash, disconnect): keep a copy of what the
    /// game recorded (it reuses its temp file) and upload that, marked as interrupted.
    fn upload_interrupted(&mut self, temp: &Path, id: Option<String>) -> Result<(), String> {
        let dir = interrupted_dir().ok_or("no app folder")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let copy = dir.join(format!("interrupted_{}.wotreplay", now_secs()));
        std::fs::copy(temp, &copy).map_err(|e| e.to_string())?;
        self.say("The battle stopped (game closed or connection lost): uploading what was recorded");
        self.upload_replay_as(&copy, id, true)?;
        clear_marker();
        self.say("Interrupted battle uploaded");
        Ok(())
    }

    /// The app was closed during a battle: upload that battle now (its replay, or what the game
    /// recorded if the game stopped too).
    fn catch_up(&mut self) -> Result<(), String> {
        let Some(started) = read_marker() else { return Ok(()) };
        let dir = PathBuf::from(&self.cfg.replays_dir);
        let since = std::time::UNIX_EPOCH + Duration::from_secs(started.saturating_sub(60));
        let replay = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "wotreplay") && e.file_name() != "temp.wotreplay")
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .filter(|(m, _)| *m >= since)
            .max_by_key(|(m, _)| *m)
            .map(|(_, p)| p);
        if let Some(p) = replay {
            if !uploaded().contains_key(&file_name(&p)) {
                self.say("Uploading the battle played while LiveReplay was closed");
                self.upload_replay(&p, None)?;
            }
            clear_marker();
            return Ok(());
        }
        let temp = dir.join("temp.wotreplay");
        let stale = std::fs::metadata(&temp)
            .ok()
            .filter(|m| m.len() > 0)
            .and_then(|m| m.modified().ok())
            .is_some_and(|m| m >= since && m.elapsed().is_ok_and(|e| e > interrupted_after()));
        if stale {
            self.upload_interrupted(&temp, None)?;
        } else if std::fs::metadata(&temp).map_or(true, |m| m.len() == 0) {
            // Nothing left of it.
            clear_marker();
        }
        // Otherwise the battle is still running: the live tick takes it from here.
        Ok(())
    }

    /// Battles uploaded without results: once the game has them (it writes them a while after
    /// the battle, also when it was left early), send the battle again with them.
    fn retry_pending(&mut self) {
        let t = now_secs();
        self.pending.retain(|p| t.saturating_sub(p.since) < RESULTS_WAIT_SECS && p.path.exists());
        let ready: Vec<Pending> = self.pending.iter().filter(|p| crate::replay::cached_results::results_for(&p.ids).is_some()).cloned().collect();
        for p in ready {
            if self.upload_replay_as(&p.path, Some(p.id.clone()), p.interrupted).is_ok() {
                self.say("Battle results added to an uploaded battle");
            }
        }
        save_pending(&self.pending);
    }

    /// Mark the current live upload finished as it is (no final replay came).
    fn finish_previous(&mut self) {
        if let Some(id) = self.current.take().and_then(|c| c.id) {
            let _ = post_bytes(&self.cfg.upload_key, &format!("/api/battles/{id}/frames?finish=1"), &[]);
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The list summary of a replay, or of the battle in progress (no header yet).
fn entry(engine: &Engine, path: &Path, b: &Battle) -> Option<super::library::ReplayEntry> {
    super::library::summarize(path, Some(&engine.game)).or_else(|| b.live.then(|| super::library::live_entry(path, b, &engine.game)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a running server (`LIVEREPLAY_SERVER`, with `LIVEREPLAY_HOME` set to a scratch
    /// folder): garbage uploaded first is replaced once a real app votes for its own copy, and
    /// its counted vote is remembered.
    #[test]
    #[ignore]
    fn asset_votes_against_server() {
        let worker = |key: &str| Worker {
            cfg: Config { upload_key: key.into(), ..Default::default() },
            status: Default::default(),
            live: Default::default(),
            checked: HashSet::new(),
            votes: HashMap::new(),
            current: None,
            last_archive: Instant::now(),
            pending: Vec::new(),
        };
        let (a, c) = ("a".repeat(40), "c".repeat(40));
        // A has a real battle with data (so its vote counts); C has none.
        let nb = NewBattle { player: "Alice".into(), realm: "EU".into(), map: "m".into(), arena_id: 5, live: true, ..Default::default() };
        let info: BattleInfo = post_json(&a, "/api/battles", &nb).unwrap();
        post_bytes(&a, &format!("/api/battles/{}/frames", info.id), &[1, 0, 0, 0, 7]).unwrap();
        let (garbage, good) = (b"garbage".to_vec(), b"the real map".to_vec());
        let mut wc = worker(&c);
        wc.sync_asset("map-m", "1.0", Worker::asset_head("map-m").unwrap_or(None), garbage).unwrap();
        assert!(wc.votes.is_empty(), "C's vote doesn't count");
        let mut wa = worker(&a);
        let head = Worker::asset_head("map-m").unwrap_or(None);
        assert!(head.is_some());
        wa.sync_asset("map-m", "1.0", head, good.clone()).unwrap();
        assert!(wa.voted("map-m", "1.0"));
        let (_, hash) = Worker::asset_head("map-m").unwrap_or(None).unwrap();
        assert_eq!(hash, crate::net::sha256_hex(&good), "A's copy is served now");
        assert!(load_votes().contains_key("map-m"));
        // Single battles: listed and deleted by id.
        assert_eq!(my_battles(&a).unwrap().len(), 1);
        delete_battle(&a, &info.id).unwrap();
        assert!(my_battles(&a).unwrap().is_empty());
        assert!(server_contact().is_ok());
    }
}
