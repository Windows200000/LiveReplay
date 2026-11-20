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
const LIVE_EVERY: Duration = Duration::from_secs(4);
const LIVE_OVERLAP: f32 = 2.0;
/// After a live battle the game writes the final replay; wait this long for it.
const FINAL_WAIT: Duration = Duration::from_secs(120);

pub fn server_url() -> String {
    std::env::var("LIVEREPLAY_SERVER").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| crate::net::SERVER_URL.to_string()).trim_end_matches('/').to_string()
}

pub fn battle_link(id: &str) -> String {
    format!("{}/b/{id}", server_url())
}

/// New random uploader key (stays on this PC; whoever has it can upload as you).
pub fn new_key() -> String {
    let mut b = [0u8; 32];
    let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(60)).build()
}

fn err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => format!("server: {code} {}", r.into_string().unwrap_or_default()),
        e => format!("network: {e}"),
    }
}

fn post_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(key: &str, path: &str, body: &T) -> Result<R, String> {
    let r = agent().post(&format!("{}{path}", server_url())).set(api::KEY_HEADER, key).send_json(body).map_err(err)?;
    r.into_json().map_err(|e| e.to_string())
}

fn post_bytes(key: &str, path: &str, body: &[u8]) -> Result<BattleInfo, String> {
    let r = agent()
        .post(&format!("{}{path}", server_url()))
        .set(api::KEY_HEADER, key)
        .set("Content-Type", "application/octet-stream")
        .send_bytes(body)
        .map_err(err)?;
    r.into_json().map_err(|e| e.to_string())
}

/// Everything the server keeps about this PC's uploads (GDPR access), as pretty JSON.
pub fn my_data(key: &str) -> Result<String, String> {
    let r = agent().get(&format!("{}/api/me", server_url())).set(api::KEY_HEADER, key).call().map_err(err)?;
    let v: serde_json::Value = r.into_json().map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

/// Delete everything this PC uploaded (battles, codes, claimed names); returns how many battles.
pub fn delete_my_data(key: &str) -> Result<usize, String> {
    let r = agent().delete(&format!("{}/api/me", server_url())).set(api::KEY_HEADER, key).call().map_err(err)?;
    let n = r.into_json().map_err(|e| e.to_string())?;
    if let Some(p) = uploads_path() {
        let _ = std::fs::remove_file(p);
    }
    Ok(n)
}

/// Delete one uploaded replay from the server.
pub fn delete_upload(key: &str, file_name: &str) -> Result<(), String> {
    let mut m = uploaded();
    let Some(id) = m.remove(file_name) else { return Err("not uploaded".into()) };
    agent().delete(&format!("{}/api/battles/{id}", server_url())).set(api::KEY_HEADER, key).call().map_err(err)?;
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
    agent().delete(&format!("{}/api/codes/{}", server_url(), api::normalize_code(code))).set(api::KEY_HEADER, key).call().map_err(err)?;
    Ok(())
}

/// Replay file name -> battle id on the server.
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
        std::thread::spawn(move || Worker { cfg, status: st, live: lv, checked: HashSet::new(), current: None, last_archive: Instant::now() }.run(rx));
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
    let mut w = Worker { cfg: cfg.clone(), status: Default::default(), live: Default::default(), checked: HashSet::new(), current: None, last_archive: Instant::now() };
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
}

struct Worker {
    cfg: Config,
    status: Arc<Mutex<Status>>,
    live: Arc<AtomicBool>,
    /// Assets already checked this session.
    checked: HashSet<String>,
    current: Option<LiveUpload>,
    last_archive: Instant,
}

impl Worker {
    fn say(&self, s: impl Into<String>) {
        self.status.lock().unwrap().line = s.into();
    }

    fn run(mut self, rx: mpsc::Receiver<Job>) {
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

    fn ensure_assets(&mut self, engine: &Engine, map: &str, version: &str) -> Result<(), String> {
        for name in [api::map_asset(map), "icons".to_string()] {
            if !self.checked.insert(name.clone()) {
                continue;
            }
            let url = format!("{}/api/assets/{name}", server_url());
            let have = match agent().head(&url).call() {
                Ok(r) => r.header(api::VERSION_HEADER).map(|v| v.to_string()),
                Err(ureq::Error::Status(404, _)) => None,
                Err(e) => {
                    self.checked.remove(&name);
                    return Err(err(e));
                }
            };
            if have.as_deref() == Some(version) {
                continue;
            }
            let body = if name == "icons" {
                crate::net::encode_pack(&crate::net::icon_pack(&engine.game))
            } else {
                match crate::net::map_pack(&engine.game, map) {
                    Some(p) => crate::net::encode_pack(&p),
                    None => continue,
                }
            };
            agent()
                .put(&url)
                .query("version", version)
                .set(api::KEY_HEADER, &self.cfg.upload_key)
                .send_bytes(&body)
                .map_err(|e| {
                    self.checked.remove(&name);
                    err(e)
                })?;
        }
        Ok(())
    }

    fn new_battle(&self, engine: &Engine, path: &Path, b: &Battle, live: bool) -> Result<BattleInfo, String> {
        let e = super::library::summarize(path, Some(&engine.game)).ok_or("not a replay")?;
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
            vehicle_ids: b.vehicles.keys().copied().collect(),
            players: b.vehicles.values().map(|v| v.name.clone()).filter(|n| !n.is_empty()).collect(),
        };
        post_json(&self.cfg.upload_key, "/api/battles", &nb)
    }

    /// Send a finished replay (complete, replacing whatever the server had for `id`).
    fn upload_replay(&mut self, path: &Path, id: Option<String>) -> Result<String, String> {
        let engine = self.engine()?;
        let battle = Battle::load(path, &engine.game, &engine.defs).map_err(|e| e.to_string())?;
        let id = match id {
            Some(id) => id,
            None => {
                let info = self.new_battle(&engine, path, &battle, false)?;
                if info.size > 0 && !info.live {
                    remember(&file_name(path), &info.id);
                    let version = super::library::summarize(path, Some(&engine.game)).map(|e| e.version).unwrap_or_default();
                    let _ = self.ensure_3d(&engine, &battle, &version);
                    return Ok(info.id);
                }
                info.id
            }
        };
        let version = super::library::summarize(path, Some(&engine.game)).map(|e| e.version).unwrap_or_default();
        self.ensure_assets(&engine, &battle.map, &version)?;
        let damage = super::library::summarize(path, Some(&engine.game)).map(|e| e.damage).unwrap_or(0);
        post_bytes(&self.cfg.upload_key, &format!("/api/battles/{id}/frames?reset=1&finish=1&damage={damage}"), &wire::encode_frame(&battle, 0.0))?;
        remember(&file_name(path), &id);
        // 3D data for the web viewer (after the battle only: building it is heavy).
        if let Err(e) = self.ensure_3d(&engine, &battle, &version) {
            self.say(format!("3D data upload: {e}"));
        }
        Ok(id)
    }

    /// Upload the web 3D packs (terrain, map objects for this mode, tanks, map-event prefabs)
    /// the server doesn't have yet for this client version.
    fn ensure_3d(&mut self, engine: &Engine, b: &Battle, version: &str) -> Result<(), String> {
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
        for (name, make) in wanted {
            if self.checked.contains(&name) {
                continue;
            }
            let url = format!("{}/api/assets/{name}", server_url());
            let have = match agent().head(&url).call() {
                Ok(r) => r.header(api::VERSION_HEADER).map(|v| v.to_string()),
                Err(ureq::Error::Status(404, _)) => None,
                Err(e) => return Err(err(e)),
            };
            if have.as_deref() != Some(version) {
                if let Some(body) = make() {
                    agent().put(&url).query("version", version).set(api::KEY_HEADER, &self.cfg.upload_key).send_bytes(&body).map_err(err)?;
                }
            }
            self.checked.insert(name);
        }
        Ok(())
    }

    fn live_tick(&mut self) -> Result<(), String> {
        let dir = PathBuf::from(&self.cfg.replays_dir);
        let temp = dir.join("temp.wotreplay");
        match std::fs::metadata(&temp) {
            Ok(meta) => {
                let len = meta.len();
                if self.current.as_ref().is_some_and(|c| c.last_len == len && c.ended.is_none()) {
                    return Ok(());
                }
                let engine = self.engine()?;
                let Ok(battle) = Battle::load(&temp, &engine.game, &engine.defs) else { return Ok(()) };
                let key = (battle.map.clone(), battle.recorder_vehicle.unwrap_or(0));
                if self.current.as_ref().map_or(true, |c| c.key != key || len < c.last_len) {
                    self.finish_previous();
                    self.current = Some(LiveUpload { id: None, key, sent_until: 0.0, last_len: 0, started: SystemTime::now(), ended: None });
                }
                if self.current.as_ref().unwrap().id.is_none() {
                    let info = self.new_battle(&engine, &temp, &battle, true)?;
                    let version = super::library::summarize(&temp, Some(&engine.game)).map(|e| e.version).unwrap_or_default();
                    self.ensure_assets(&engine, &battle.map, &version)?;
                    self.current.as_mut().unwrap().id = Some(info.id);
                }
                let c = self.current.as_mut().unwrap();
                let from = if c.sent_until > 0.0 { (c.sent_until - LIVE_OVERLAP).max(0.001) } else { 0.0 };
                let id = c.id.clone().unwrap();
                post_bytes(&self.cfg.upload_key, &format!("/api/battles/{id}/frames"), &wire::encode_frame(&battle, from))?;
                c.sent_until = battle.duration;
                c.last_len = len;
                c.ended = None;
                self.say(format!("Live battle on {} is being uploaded", battle.map_display));
            }
            Err(_) => {
                let Some(c) = self.current.as_mut() else { return Ok(()) };
                let ended = *c.ended.get_or_insert_with(Instant::now);
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
                        if let Some(id) = id {
                            self.say("Uploading the finished battle");
                            self.upload_replay(&p, Some(id))?;
                            self.say("Battle uploaded");
                        }
                    }
                } else if ended.elapsed() > FINAL_WAIT {
                    self.finish_previous();
                }
            }
        }
        Ok(())
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
