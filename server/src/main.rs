//! LiveReplay server.
//!
//! Stores battles uploaded by desktop clients (already decoded into the compact frame format, see
//! `src/replay/wire.rs`) and serves them, the game assets the web viewer needs (uploaded by clients,
//! never part of the repository) and the web viewer itself. Plain HTTP; HTTPS is done by a reverse
//! proxy (see `docker-compose.yml`).
//!
//! Environment: `BIND` (default `0.0.0.0:8080`), `DATA_DIR` (default `data`), `WEB_DIR`
//! (default `web-dist`), `THREADS` (default 16), `ADMIN_TOKEN`, the limits below and in `guard`.
//!
//! Nothing uploaded is ever run or interpreted by the server: battle data, original replays and
//! assets are stored as opaque bytes and served back as `application/octet-stream` (with
//! `nosniff` and a sandbox CSP); only small JSON descriptions are parsed, into fixed types, and
//! every text in them is bounded and stripped of control characters.
//!
//! Made for a small shared machine: large uploads go straight to disk and large downloads are
//! streamed from it, and no large file is read or written while the shared state is locked.
//!
//! Data folder: `claims.json` (player name -> uploader key hash), `codes.json` (code hash ->
//! owner key hash + expiry), `blocked.json` (blocked key hashes), `battles/<id>.json` +
//! `battles/<id>.bin` (+ `.wotreplay`), `assets/` (see `assets`), `tmp/` (uploads in progress).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use livereplay_api::*;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

mod admin;
mod assets;
mod guard;
mod privacy;

use assets::{AssetMeta, Copy};
use guard::{Daily, Guard, Visitor};

/// A live battle not updated for this long is no longer live (the app sends every 2 s; it
/// crashed, was closed, or the battle ended without a final upload).
const STALE_SECS: u64 = 90;
const MAX_BODY: usize = 16 << 20;
/// JSON requests are small (a battle's description with 30 players is ~3 KB); parsing a big one
/// could take far more memory than its size.
const MAX_JSON: usize = 64 << 10;
const MAX_ASSET: u64 = 64 << 20;
/// Largest original replay file accepted.
const MAX_REPLAY: u64 = 64 << 20;
const MAX_BATTLE: u64 = 24 << 20;
const PER_PAGE: usize = 50;
/// Abuse limits per uploader key (overridable: `MAX_BATTLES_PER_DAY`, `MAX_KEY_MB`, `MAX_TOTAL_GB`).
/// Keys cost nothing to make, so the limits that matter are per visitor (see `guard`).
const BATTLES_PER_DAY: usize = 300;
const KEY_MB: u64 = 2048;
const TOTAL_GB: u64 = 50;
/// Player names one key may claim.
const CLAIMS_PER_KEY: usize = 10;
/// Backstops for memory (far above normal use; the per-visitor limits bound how fast they fill).
const MAX_CLAIMS: usize = 500_000;
const MAX_CODES: usize = 200_000;
const MAX_ASSETS: usize = 200_000;
/// Longest text kept from a battle's description fields.
const MAX_TEXT: usize = 100;
/// Viewing codes looked at per request.
const CODES_PER_REQUEST: usize = 20;
/// A battle that never got any data is removed after this long.
const EMPTY_BATTLE_SECS: u64 = 3600;
/// Public battle lists are answered from a cache this old at most; lists and live lists are made
/// from a copy of the battle index this old at most (made outside the lock).
const LIST_CACHE: Duration = Duration::from_secs(2);
const SNAPSHOT_AGE: Duration = Duration::from_secs(2);
/// Storage use is counted again at most this often (and kept up to date in between).
const USAGE_CACHE: Duration = Duration::from_secs(30);

fn env_num(k: &str, default: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The operator's contact (`OPERATOR_CONTACT`), empty if not set.
fn contact() -> String {
    std::env::var("OPERATOR_CONTACT").unwrap_or_default().trim().to_string()
}

/// A battle date as the game writes it ("dd.mm.yyyy hh:mm:ss", the player's local time) in
/// seconds, for comparing two dates.
fn date_secs(d: &str) -> Option<i64> {
    let n: Vec<i64> = d.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().ok()).collect::<Option<_>>()?;
    let [day, m, y, h, mi, s] = n[..] else { return None };
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || !(0..=9999).contains(&y) {
        return None;
    }
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86400 + h.min(99) * 3600 + mi.min(99) * 60 + s.min(99))
}

/// Text from a request as stored: no control characters, at most `max` characters.
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

/// A player name as the game allows it (letters, digits, `_`, `-`; up to 32 characters).
fn valid_player(p: &str) -> bool {
    let n = p.chars().count();
    (1..=32).contains(&n) && p.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// The key a player name is claimed under: realm in capitals, name in lower case (so `Foo` and
/// `foo` are one name).
fn claim_key(realm: &str, player: &str) -> String {
    format!("{}:{}", realm.trim().to_ascii_uppercase(), player.to_lowercase())
}

/// Constant-time comparison of two secrets (compares their hashes, so lengths don't leak either).
fn same_secret(a: &str, b: &str) -> bool {
    let (x, y) = (sha2::Sha256::digest(a.as_bytes()), sha2::Sha256::digest(b.as_bytes()));
    x.iter().zip(y.iter()).fold(0u8, |acc, (p, q)| acc | (p ^ q)) == 0
}

/// The operator's admin token in the request (`ADMIN_TOKEN`, at least 16 characters).
fn is_admin(rq: &Request) -> bool {
    let token = std::env::var("ADMIN_TOKEN").unwrap_or_default();
    token.len() >= 16 && header(rq, ADMIN_HEADER).is_some_and(|h| same_secret(h, &token))
}

fn sha_hex(b: &[u8]) -> String {
    hex(&sha2::Sha256::digest(b))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Random token from an unambiguous alphabet (no 0/O, 1/I/L). Unbiased: bytes that would make
/// some letters more likely are skipped.
fn token(n: usize) -> String {
    const A: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
    let limit = 256 - 256 % A.len();
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let mut buf = [0u8; 32];
        getrandom::fill(&mut buf).expect("os random");
        out.extend(buf.iter().filter(|b| (**b as usize) < limit).map(|b| A[*b as usize % A.len()] as char).take(n - out.len()));
    }
    out
}

#[derive(Serialize, Deserialize, Clone)]
struct Code {
    owner: String,
    expires: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct Stored {
    /// Shared with list snapshots (copied only when it changes while a snapshot holds it).
    info: Arc<BattleInfo>,
    /// Hash of the uploader's key.
    owner: String,
    /// Unix seconds the entry was made.
    #[serde(default)]
    created: u64,
    #[serde(default)]
    vehicle_ids: Vec<i32>,
    /// Lower-case names of everyone in the battle (removal requests).
    #[serde(default)]
    players: Vec<String>,
    /// The game's arenaUniqueID (0 = unknown): recognises the same battle.
    #[serde(default)]
    arena_id: u64,
    /// Lower-case names of the uploader's team, as its app reported them (live viewpoints of a
    /// team are shared with that team's codes, see [`may_watch`]).
    #[serde(default)]
    allies: Vec<String>,
}

/// Files that changed in memory and are written after the request.
#[derive(Default, Clone, Copy)]
struct Dirty {
    claims: bool,
    codes: bool,
    blocked: bool,
}

#[derive(Default)]
struct State {
    claims: HashMap<String, String>,
    codes: HashMap<String, Code>,
    battles: HashMap<String, Stored>,
    assets: HashMap<String, AssetMeta>,
    /// Blocked uploader keys (hashes).
    blocked: HashSet<String>,
    /// Asset uploads in progress: name -> (bytes received, bytes in total). Memory only.
    uploading: HashMap<String, (u64, u64)>,
    /// Assets an app announced it is about to upload: name -> until (Unix seconds). Memory only.
    expected: HashMap<String, u64>,
    dirty: Dirty,
    /// Changes whenever a battle goes away, gets its first data or ends (cached lists are then
    /// out of date at once).
    epoch: u64,
}

/// A copy of what lists need, made at most every [`SNAPSHOT_AGE`] (copied under the lock,
/// sorted outside it), so busy or made-up list queries never hold the shared state for long.
struct Snapshot {
    at: Instant,
    epoch: u64,
    /// Battles with data, newest first, with their uploader key.
    rows: Vec<(Arc<BattleInfo>, String)>,
    /// Keys whose asset votes may count: they own a player name and uploaded a battle with data.
    voters: HashSet<String>,
    /// Live viewpoints of grouped battles: group -> members (for team access, see [`may_watch`]).
    members: HashMap<String, Vec<Member>>,
}

/// One live upload of a grouped battle.
struct Member {
    id: String,
    owner: String,
    /// Lower-case player name of the uploader.
    player: String,
    allies: Vec<String>,
}

/// Whether the viewer (who holds codes or keys of `owners`) may watch this upload: finished
/// battles are public; a live one needs a code from its uploader, or from a teammate who
/// uploaded the same battle live, if both uploads list each other as teammates. A code from
/// the other team alone never shows it.
fn may_watch(snap: &Snapshot, i: &BattleInfo, owner: &str, owners: &[String], t: u64) -> bool {
    if !info_live(i, t) || owners.iter().any(|o| o == owner) {
        return true;
    }
    let Some(members) = snap.members.get(&i.group) else { return false };
    let Some(me) = members.iter().find(|m| m.id == i.id) else { return false };
    members.iter().any(|u| u.id != me.id && owners.contains(&u.owner) && u.allies.contains(&me.player) && me.allies.contains(&u.player))
}

/// Storage in use, per uploader key and in total.
#[derive(Default)]
struct Usage {
    per_owner: HashMap<String, u64>,
    total: u64,
}

struct StaticFile {
    body: Arc<[u8]>,
    gz: Option<Arc<[u8]>>,
    br: Option<Arc<[u8]>>,
    etag: String,
    ctype: &'static str,
}

struct App {
    data: PathBuf,
    state: Mutex<State>,
    web: HashMap<String, StaticFile>,
    guard: Guard,
    /// Held while writing the JSON files, so an older copy never overwrites a newer one.
    persist: Mutex<()>,
    /// One lock per battle for writing its data file (outside the state lock).
    io_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Public battle lists: query -> (epoch, when, JSON).
    list_cache: Mutex<HashMap<String, (u64, Instant, Arc<[u8]>)>>,
    usage: Mutex<Option<(Instant, Usage)>>,
    snapshot: Mutex<Option<Arc<Snapshot>>>,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Write a file through a temp file and a rename, so a crash never leaves half a file.
fn write_atomic(p: &Path, b: &[u8]) -> std::io::Result<()> {
    // Its own temp name: two files of one battle (`.bin`, `.json`) may be written at once.
    let tmp = p.with_file_name(format!(".{}.{}.tmp", p.file_name().unwrap_or_default().to_string_lossy(), token(8)));
    std::fs::write(&tmp, b)?;
    std::fs::rename(&tmp, p)
}

fn write_json<T: Serialize>(p: &Path, v: &T) {
    if let Ok(b) = serde_json::to_vec(v) {
        let _ = write_atomic(p, &b);
    }
}

fn gzip(b: &[u8], level: u32) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(level));
    let _ = e.write_all(b);
    e.finish().unwrap_or_default()
}

fn brotli_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let params = brotli::enc::BrotliEncoderParams { quality: 11, lgwin: 24, ..Default::default() };
    let _ = brotli::BrotliCompress(&mut &b[..], &mut out, &params);
    out
}

fn content_type(p: &str) -> &'static str {
    match p.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// An upload being received into `tmp/`; deleted unless it is moved into place.
struct TempFile(Option<PathBuf>);

impl TempFile {
    /// Move it to `dest` (replacing what is there).
    fn place(mut self, dest: &Path) -> std::io::Result<()> {
        let p = self.0.take().unwrap_or_default();
        std::fs::rename(&p, dest).inspect_err(|_| {
            let _ = std::fs::remove_file(&p);
        })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// A received upload: where it is, its size, its SHA-256 and its first bytes.
struct Received {
    file: TempFile,
    size: u64,
    hash: String,
    head: Vec<u8>,
}

impl App {
    fn load(data: PathBuf, web_dir: &Path) -> Self {
        for d in ["battles", "assets", "assets/candidates", "tmp"] {
            let _ = std::fs::create_dir_all(data.join(d));
        }
        // Uploads a restart cut off.
        for e in std::fs::read_dir(data.join("tmp")).into_iter().flatten().flatten() {
            let _ = std::fs::remove_file(e.path());
        }
        let mut st = State { codes: read_json(&data.join("codes.json")), blocked: read_json(&data.join("blocked.json")), ..Default::default() };
        // Claims in the normalised form (older files had the name as uploaded); the first one wins.
        let old: HashMap<String, String> = read_json(&data.join("claims.json"));
        let mut old: Vec<(String, String)> = old.into_iter().collect();
        old.sort();
        for (k, owner) in old {
            let key = k.split_once(':').map_or(k.clone(), |(realm, player)| claim_key(realm, player));
            st.claims.entry(key).or_insert(owner);
        }
        write_json(&data.join("claims.json"), &st.claims);
        for e in std::fs::read_dir(data.join("battles")).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Some(s) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Stored>(&b).ok()) {
                    st.battles.insert(s.info.id.clone(), s);
                }
            }
        }
        for e in std::fs::read_dir(data.join("assets")).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let (Some(name), Some(m)) = (p.file_stem(), std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<AssetMeta>(&b).ok())) {
                    st.assets.insert(name.to_string_lossy().into_owned(), m);
                }
            }
        }
        // Web viewer files, compressed once at startup.
        let mut web = HashMap::new();
        let mut stack = vec![web_dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Ok(body) = std::fs::read(&p) else { continue };
                let rel = p.strip_prefix(web_dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                let ctype = content_type(&rel);
                let compressible = !ctype.starts_with("image/png") && body.len() > 512;
                let gz = compressible.then(|| gzip(&body, 9).into());
                let br = compressible.then(|| brotli_bytes(&body).into());
                let etag = format!("\"{}\"", &sha_hex(&body)[..16]);
                web.insert(format!("/{rel}"), StaticFile { body: body.into(), gz, br, etag, ctype });
            }
        }
        let removed = remove_duplicates(&data, &mut st) + merge_same_battles(&data, &mut st);
        if removed > 0 {
            println!("removed {removed} duplicate uploads");
        }
        // Expired viewing codes are deleted.
        let t = now();
        st.codes.retain(|_, c| c.expires > t);
        write_json(&data.join("codes.json"), &st.codes);
        println!("loaded {} battles, {} assets, {} web files", st.battles.len(), st.assets.len(), web.len());
        App {
            data,
            state: Mutex::new(st),
            web,
            guard: Guard::new(),
            persist: Mutex::new(()),
            io_locks: Mutex::new(HashMap::new()),
            list_cache: Mutex::new(HashMap::new()),
            usage: Mutex::new(None),
            snapshot: Mutex::new(None),
        }
    }

    /// The shared state. A panic while it was held (a bug) must not take every later request down.
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Write the JSON files that changed (after a request, with the state unlocked while writing).
    fn flush(&self) {
        let _p = self.persist.lock().unwrap_or_else(|e| e.into_inner());
        let (claims, codes, blocked) = {
            let mut st = self.st();
            let d = std::mem::take(&mut st.dirty);
            (
                d.claims.then(|| serde_json::to_vec(&st.claims).ok()).flatten(),
                d.codes.then(|| serde_json::to_vec(&st.codes).ok()).flatten(),
                d.blocked.then(|| serde_json::to_vec(&st.blocked).ok()).flatten(),
            )
        };
        for (name, b) in [("claims.json", claims), ("codes.json", codes), ("blocked.json", blocked)] {
            if let Some(b) = b {
                let _ = write_atomic(&self.data.join(name), &b);
            }
        }
    }

    /// Delete a battle's files and entry (caller holds the state lock; deleting is quick).
    fn remove_battle(&self, st: &mut State, id: &str) -> bool {
        let dir = self.data.join("battles");
        for ext in ["bin", "json", "wotreplay"] {
            let _ = std::fs::remove_file(dir.join(format!("{id}.{ext}")));
        }
        st.epoch += 1;
        if let Some(s) = st.battles.remove(id) {
            if let Some((_, u)) = self.usage.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                let n = s.info.size + s.info.replay_size;
                u.total = u.total.saturating_sub(n);
                if let Some(m) = u.per_owner.get_mut(&s.owner) {
                    *m = m.saturating_sub(n);
                }
            }
            true
        } else {
            false
        }
    }

    fn save_battle(&self, s: &Stored) {
        write_json(&self.data.join("battles").join(format!("{}.json", s.info.id)), s);
    }

    fn asset_path(&self, name: &str) -> PathBuf {
        self.data.join("assets").join(format!("{name}.bin"))
    }

    fn candidate_path(&self, name: &str, hash: &str) -> PathBuf {
        self.data.join("assets").join("candidates").join(assets::candidate_file(name, hash))
    }

    fn save_asset(&self, name: &str, m: &AssetMeta) {
        write_json(&self.data.join("assets").join(format!("{name}.json")), m);
    }

    /// Remove an asset with every copy (caller holds the state lock).
    fn remove_asset(&self, st: &mut State, name: &str) {
        if let Some(m) = st.assets.remove(name) {
            for c in &m.others {
                let _ = std::fs::remove_file(self.candidate_path(name, &c.hash));
            }
        }
        let _ = std::fs::remove_file(self.asset_path(name));
        let _ = std::fs::remove_file(self.data.join("assets").join(format!("{name}.json")));
        *self.usage.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Lock for writing one battle's data file.
    fn io_lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.io_locks.lock().unwrap_or_else(|e| e.into_inner()).entry(id.to_string()).or_default().clone()
    }

    /// Receive a request body into `tmp/` (streamed: never all in memory), within `max` bytes and
    /// the visitor's daily upload allowance. `progress`: show it as an asset upload in progress.
    fn receive(&self, rq: &mut Request, v: Visitor, max: u64, progress: Option<&str>) -> Result<Received, Reply> {
        let total = rq.body_length().unwrap_or(0) as u64;
        if total > max {
            return Err(Reply::err(413, "too large"));
        }
        if !self.guard.fits(v, Daily::Bytes, total) {
            return Err(Reply::err(429, "upload limit reached for today"));
        }
        let path = self.data.join("tmp").join(format!("{}.part", token(16)));
        let file = TempFile(Some(path.clone()));
        let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&path).map_err(|_| Reply::err(500, "write failed"))?);
        struct Progress<'a>(&'a App, Option<&'a str>);
        impl Drop for Progress<'_> {
            fn drop(&mut self) {
                if let Some(n) = self.1 {
                    self.0.st().uploading.remove(n);
                }
            }
        }
        if let Some(n) = progress {
            self.st().uploading.insert(n.to_string(), (0, total));
        }
        let _progress = Progress(self, progress);
        let mut hasher = sha2::Sha256::new();
        let mut head = Vec::new();
        let mut size = 0u64;
        let mut buf = vec![0u8; 256 << 10];
        let mut last = Instant::now();
        let reader = rq.as_reader();
        loop {
            let n = reader.read(&mut buf).map_err(|_| Reply::err(400, "bad body"))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > max {
                return Err(Reply::err(413, "too large"));
            }
            if head.len() < 16 {
                head.extend_from_slice(&buf[..n.min(16 - head.len())]);
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|_| Reply::err(500, "write failed"))?;
            if let Some(name) = progress.filter(|_| last.elapsed() > Duration::from_millis(250)) {
                last = Instant::now();
                if let Some(u) = self.st().uploading.get_mut(name) {
                    u.0 = size;
                }
            }
        }
        out.flush().map_err(|_| Reply::err(500, "write failed"))?;
        drop(out);
        if !self.guard.take(v, Daily::Bytes, size) {
            return Err(Reply::err(429, "upload limit reached for today"));
        }
        Ok(Received { file, size, hash: hex(&hasher.finalize()), head })
    }

    /// Make sure the served copy of an asset has its hash (assets from before hashes were kept):
    /// the file is read with the state unlocked.
    fn ensure_hash(&self, name: &str) {
        if !self.st().assets.get(name).is_some_and(|m| m.current.hash.is_empty()) {
            return;
        }
        let Ok(mut f) = std::fs::File::open(self.asset_path(name)) else { return };
        let mut h = sha2::Sha256::new();
        let mut buf = vec![0u8; 256 << 10];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => h.update(&buf[..n]),
                Err(_) => return,
            }
        }
        let hash = hex(&h.finalize());
        let mut st = self.st();
        if let Some(m) = st.assets.get_mut(name).filter(|m| m.current.hash.is_empty()) {
            m.current.hash = hash;
            let m = m.clone();
            self.save_asset(name, &m);
        }
    }

    /// Serve the candidate that now has the most votes (swaps two files; state locked).
    fn promote(&self, name: &str, m: &mut AssetMeta, i: usize) {
        if m.current.hash.is_empty() {
            return;
        }
        let (cur, cand) = (self.asset_path(name), self.candidate_path(name, &m.others[i].hash));
        // The served file is replaced in one step (a second name keeps the old copy), so a
        // download at the same moment gets one copy or the other, never nothing.
        let parked = self.candidate_path(name, &m.current.hash);
        let _ = std::fs::remove_file(&parked);
        if std::fs::hard_link(&cur, &parked).is_err() {
            return;
        }
        if std::fs::rename(&cand, &cur).is_err() {
            let _ = std::fs::remove_file(&parked);
            return;
        }
        m.promote(i);
        println!("asset {name}: now serving {} ({} votes)", &m.current.hash[..12], m.current.votes.len());
    }

    /// Whether an uploader key's asset votes count: it uploaded a battle with data (so it is a
    /// real app with a player name), and few enough keys voted from this visitor today.
    fn counts(&self, v: Visitor, owner: &str) -> bool {
        self.snapshot().voters.contains(owner) && self.guard.voter(v, owner)
    }

    /// The list snapshot (made again when older than [`SNAPSHOT_AGE`] or a battle went away).
    fn snapshot(&self) -> Arc<Snapshot> {
        // One maker at a time; the others wait for its result instead of making their own.
        let mut slot = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        let epoch = self.st().epoch;
        if let Some(s) = slot.as_ref().filter(|s| s.epoch == epoch && s.at.elapsed() < SNAPSHOT_AGE) {
            return s.clone();
        }
        let (mut rows, voters, members) = {
            let st = self.st();
            let rows: Vec<(Arc<BattleInfo>, String)> = st.battles.values().filter(|s| s.info.size > 0).map(|s| (s.info.clone(), s.owner.clone())).collect();
            let claimed: HashSet<&String> = st.claims.values().collect();
            let voters: HashSet<String> = rows.iter().map(|r| &r.1).filter(|k| claimed.contains(k)).cloned().collect();
            let mut members: HashMap<String, Vec<Member>> = HashMap::new();
            for s in st.battles.values().filter(|s| s.info.live && !s.info.group.is_empty()) {
                members.entry(s.info.group.clone()).or_default().push(Member { id: s.info.id.clone(), owner: s.owner.clone(), player: s.info.player.to_lowercase(), allies: s.allies.clone() });
            }
            (rows, voters, members)
        };
        let key = |d: &str| {
            let (day, time) = d.split_once(' ').unwrap_or((d, ""));
            match day.split('.').collect::<Vec<_>>()[..] {
                [d, m, y] => format!("{y}{m:0>2}{d:0>2}{time}"),
                _ => d.to_string(),
            }
        };
        rows.sort_by_cached_key(|r| std::cmp::Reverse(key(&r.0.date)));
        let s = Arc::new(Snapshot { at: Instant::now(), epoch, rows, voters, members });
        *slot = Some(s.clone());
        s
    }

    /// Storage use (counted again every [`USAGE_CACHE`], kept up to date in between).
    fn over_limit(&self, st: &State, owner: &str, extra: u64) -> Option<Reply> {
        let mut u = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        if u.as_ref().is_none_or(|(at, _)| at.elapsed() > USAGE_CACHE) {
            *u = Some((Instant::now(), usage(st)));
        }
        let Some((_, u)) = u.as_mut() else { return None };
        let mine = u.per_owner.get(owner).copied().unwrap_or(0);
        if mine + extra > env_num("MAX_KEY_MB", KEY_MB) << 20 {
            return Some(Reply::err(507, "storage limit for this uploader reached"));
        }
        if u.total + extra > env_num("MAX_TOTAL_GB", TOTAL_GB) << 30 {
            return Some(Reply::err(507, "server storage full"));
        }
        *u.per_owner.entry(owner.to_string()).or_default() += extra;
        u.total += extra;
        None
    }
}

/// Bytes stored per uploader and for the whole server (battle data, original replays, assets).
fn usage(st: &State) -> Usage {
    let mut u = Usage::default();
    for s in st.battles.values() {
        let n = s.info.size + s.info.replay_size;
        u.total += n;
        *u.per_owner.entry(s.owner.clone()).or_default() += n;
    }
    for c in st.assets.values().flat_map(|a| a.copies()) {
        u.total += c.size;
        *u.per_owner.entry(c.owner.clone()).or_default() += c.size;
    }
    u
}

enum Body {
    Bytes(Arc<[u8]>),
    /// Streamed (a file), with its length.
    Stream(Box<dyn Read + Send>, usize),
}

struct Reply {
    status: u16,
    body: Body,
    ctype: &'static str,
    headers: Vec<(String, String)>,
    /// Allow gzip for this body (text).
    compress: bool,
}

impl Reply {
    fn json<T: Serialize>(v: &T) -> Self {
        Self::json_bytes(serde_json::to_vec(v).unwrap_or_default().into())
    }
    fn json_bytes(b: Arc<[u8]>) -> Self {
        Reply { status: 200, body: Body::Bytes(b), ctype: "application/json", headers: vec![("Cache-Control".into(), "no-store".into())], compress: true }
    }
    fn err(status: u16, msg: &str) -> Self {
        Reply { status, body: Body::Bytes(msg.as_bytes().into()), ctype: "text/plain; charset=utf-8", headers: vec![], compress: false }
    }
    fn bin(body: Vec<u8>) -> Self {
        Reply { status: 200, body: Body::Bytes(body.into()), ctype: "application/octet-stream", headers: vec![], compress: false }
    }
    fn html(body: String) -> Self {
        Reply { status: 200, body: Body::Bytes(body.into_bytes().into()), ctype: "text/html; charset=utf-8", headers: vec![], compress: true }
    }
    /// A file from `from` to `from + len`, streamed.
    fn file(f: std::fs::File, from: u64, len: u64) -> Self {
        let mut f = f;
        let body = if from == 0 || f.seek(std::io::SeekFrom::Start(from)).is_ok() { Body::Stream(Box::new(f.take(len)), len as usize) } else { Body::Bytes(Arc::from(&[][..])) };
        Reply { status: 200, body, ctype: "application/octet-stream", headers: vec![], compress: false }
    }
    fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

fn header<'a>(rq: &'a Request, name: &str) -> Option<&'a str> {
    rq.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn query(q: &str, k: &str) -> Option<String> {
    q.split('&').filter_map(|kv| kv.split_once('=').or(Some((kv, "")))).find(|(a, _)| *a == k).map(|(_, v)| url_decode(v))
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_body(rq: &mut Request, max: usize) -> Result<Vec<u8>, Reply> {
    if rq.body_length().unwrap_or(0) > max {
        return Err(Reply::err(413, "too large"));
    }
    let mut b = Vec::new();
    rq.as_reader().take(max as u64 + 1).read_to_end(&mut b).map_err(|_| Reply::err(400, "bad body"))?;
    if b.len() > max {
        return Err(Reply::err(413, "too large"));
    }
    Ok(b)
}

/// Hash of the uploader key in the request (keys are long random strings made by the client).
fn key_hash(rq: &Request) -> Option<String> {
    header(rq, KEY_HEADER).filter(|k| k.len() >= 32 && k.len() <= 128).map(|k| sha_hex(k.as_bytes()))
}

/// The uploader making the request: its key hash, unless missing or blocked.
fn uploader(app: &App, rq: &Request) -> Result<String, Reply> {
    let Some(k) = key_hash(rq) else { return Err(Reply::err(401, "key required")) };
    if app.st().blocked.contains(&k) {
        return Err(Reply::err(403, "this uploader is blocked"));
    }
    Ok(k)
}

/// Frames are `u32 len + body`; accept the upload only if it is made of whole frames.
fn whole_frames(b: &[u8]) -> bool {
    let mut at = 0usize;
    while at < b.len() {
        let Some(len) = b.get(at..at + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap()) as usize) else { return false };
        at += 4 + len;
    }
    at == b.len()
}

fn is_live(s: &Stored, t: u64) -> bool {
    s.info.live && t.saturating_sub(s.info.updated) < STALE_SECS
}

/// The uploader `owner` may act on this upload: it sent it, or it owns the player name it was
/// uploaded for (an older key of the same player, e.g. after a reinstall). Uploads by other
/// players that this player is in are never theirs.
fn owns(st: &State, owner: &str, s: &Stored) -> bool {
    s.owner == owner || st.claims.get(&claim_key(&s.info.realm, &s.info.player)).is_some_and(|k| k == owner)
}

/// The same battle uploaded more than once for the same player (also by different keys): same
/// game battle id, or the same replay file. The most complete copy stays, owned by whoever owns the
/// player name now.
fn merge_same_battles(data: &Path, st: &mut State) -> usize {
    let key = |s: &Stored| -> Option<String> {
        let who = format!("{}:{}", s.info.realm.to_ascii_uppercase(), s.info.player.to_lowercase());
        if s.arena_id != 0 {
            Some(format!("{who}#a{}", s.arena_id))
        } else if !s.info.file_name.is_empty() {
            Some(format!("{who}#f{}", s.info.file_name))
        } else {
            None
        }
    };
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for s in st.battles.values() {
        if let Some(k) = key(s) {
            groups.entry(k).or_default().push(s.info.id.clone());
        }
    }
    let mut removed = 0;
    for ids in groups.into_values().filter(|v| v.len() > 1) {
        let keep = ids.iter().max_by_key(|id| {
            let s = &st.battles[*id];
            (s.info.replay_size > 0, !s.info.live, s.info.size, s.info.updated)
        });
        let keep = keep.cloned().unwrap_or_default();
        for id in ids.iter().filter(|id| **id != keep) {
            for ext in ["bin", "json", "wotreplay"] {
                let _ = std::fs::remove_file(data.join("battles").join(format!("{id}.{ext}")));
            }
            st.battles.remove(id);
            removed += 1;
        }
        let owner = st.battles.get(&keep).and_then(|s| st.claims.get(&claim_key(&s.info.realm, &s.info.player)).cloned());
        if let (Some(owner), Some(s)) = (owner, st.battles.get_mut(&keep)) {
            if s.owner != owner {
                s.owner = owner;
                write_json(&data.join("battles").join(format!("{keep}.json")), &*s);
            }
        }
    }
    removed
}

/// A battle as shown: a live upload that stopped without being finished is interrupted.
fn shown(s: &Stored, t: u64) -> BattleInfo {
    let mut i = (*s.info).clone();
    if s.info.live && !is_live(s, t) {
        i.live = false;
        i.interrupted = true;
    }
    i
}

/// Live uploads of one battle that app restarts made into several entries (before battles were
/// recognised by their id): the most complete one stays. Same uploader, player and map, only
/// live data (no replay file name), and at least 3 tanks in common, so different battles on the
/// same map are never merged.
fn remove_duplicates(data: &Path, st: &mut State) -> usize {
    let mut ids: Vec<&Stored> = st.battles.values().filter(|s| s.arena_id == 0 && s.info.file_name.is_empty()).collect();
    ids.sort_by_key(|s| s.created);
    let mut drop = HashSet::new();
    for (i, a) in ids.iter().enumerate() {
        if drop.contains(&a.info.id) {
            continue;
        }
        let tanks: HashSet<i32> = a.vehicle_ids.iter().copied().collect();
        let mut same: Vec<&Stored> = vec![a];
        for b in ids[i + 1..].iter().filter(|b| !drop.contains(&b.info.id)) {
            if b.owner == a.owner && b.info.player == a.info.player && b.info.map == a.info.map && b.vehicle_ids.iter().filter(|v| tanks.contains(v)).count() >= 3 {
                same.push(b);
            }
        }
        if same.len() > 1 {
            let keep = same.iter().max_by_key(|s| (s.info.size, s.info.updated)).map(|s| s.info.id.clone()).unwrap_or_default();
            drop.extend(same.iter().map(|s| s.info.id.clone()).filter(|id| *id != keep));
        }
    }
    for id in &drop {
        for ext in ["bin", "json", "wotreplay"] {
            let _ = std::fs::remove_file(data.join("battles").join(format!("{id}.{ext}")));
        }
        st.battles.remove(id);
    }
    drop.len()
}

/// Owners (key hashes) whose live battles the given codes unlock, and the still-valid codes.
/// Wrong codes count against the visitor: after a few different ones in an hour, its codes
/// aren't looked at any more (no guessing).
fn unlocked(app: &App, st: &State, rq: &Request, v: Visitor) -> (Vec<String>, Vec<(String, String, u64)>) {
    let t = now();
    let mut owners = Vec::new();
    let mut valid = Vec::new();
    let codes = header(rq, CODES_HEADER).unwrap_or("");
    if !codes.is_empty() && app.guard.may_try_codes(v) {
        for c in codes.split(',').map(normalize_code).filter(|c| !c.is_empty()).take(CODES_PER_REQUEST) {
            let h = sha_hex(c.as_bytes());
            match st.codes.get(&h).filter(|x| x.expires > t) {
                Some(code) => {
                    owners.push(code.owner.clone());
                    valid.push((c, code.owner.clone(), code.expires));
                }
                None => app.guard.bad_code(v, &h),
            }
        }
    }
    if let Some(k) = key_hash(rq) {
        owners.push(k);
    }
    (owners, valid)
}

fn route(app: &App, rq: &mut Request, v: Visitor) -> Reply {
    let url = rq.url().to_string();
    let (path, q) = url.split_once('?').unwrap_or((&url, ""));
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let method = rq.method().clone();
    match (&method, parts.as_slice()) {
        (_, ["api", "admin", ..]) => admin::route(app, rq, &method, &parts[2..], q),
        (Method::Get, ["admin"]) => Reply::html(admin::PAGE.to_string()).header("Cache-Control", "no-store"),
        (Method::Get, ["admin.js"]) => Reply { status: 200, body: Body::Bytes(admin::SCRIPT.as_bytes().into()), ctype: "text/javascript", headers: vec![("Cache-Control".into(), "no-store".into())], compress: true },
        (Method::Post, ["api", "battles"]) => new_battle(app, rq, v),
        (Method::Get, ["privacy"]) => Reply::html(privacy::page()),
        (Method::Get, ["legal" | "impressum"]) => Reply::html(privacy::legal()),
        (Method::Get, ["api", "info"]) => Reply::json(&ServerInfo { contact: contact() }),
        (Method::Delete, ["api", "battles", id]) => {
            let owner = match uploader(app, rq) {
                Ok(o) => o,
                Err(e) => return e,
            };
            let mut st = app.st();
            match st.battles.get(*id) {
                Some(s) if owns(&st, &owner, s) => {
                    app.remove_battle(&mut st, id);
                    Reply::json(&())
                }
                Some(_) => Reply::err(403, "not yours"),
                None => Reply::json(&()),
            }
        }
        (Method::Get, ["api", "me"]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            if !app.guard.heavy(v) {
                return Reply::err(429, "too many requests");
            }
            let st = app.st();
            let t = now();
            // Also uploads of this player's names by an older key (same player, new install).
            let mut battles: Vec<BattleInfo> = st.battles.values().filter(|s| owns(&st, &owner, s)).map(|s| shown(s, t)).collect();
            battles.sort_by(|a, b| b.updated.cmp(&a.updated));
            Reply::json(&MyData {
                player_names: st.claims.iter().filter(|(_, k)| **k == owner).map(|(n, _)| n.clone()).collect(),
                battles,
                code_expiries: st.codes.values().filter(|c| c.owner == owner && c.expires > t).map(|c| c.expires).collect(),
            })
        }
        (Method::Delete, ["api", "me"]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            if !app.guard.heavy(v) {
                return Reply::err(429, "too many requests");
            }
            Reply::json(&forget_uploader(app, &mut app.st(), &owner))
        }
        (Method::Get, ["api", "battles"]) => list(app, rq, q, v),
        (Method::Get, ["api", "battles", id]) => {
            let found = {
                let st = app.st();
                st.battles.get(*id).map(|s| (shown(s, now()), (*s.info).clone(), s.owner.clone(), unlocked(app, &st, rq, v).0))
            };
            match found {
                Some((out, info, owner, owners)) if may_watch(&app.snapshot(), &info, &owner, &owners, now()) => Reply::json(&out),
                _ => Reply::err(404, "not found"),
            }
        }
        (Method::Post, ["api", "battles", id, "frames"]) => frames(app, rq, v, id, q),
        (Method::Put, ["api", "battles", id, "replay"]) => put_replay(app, rq, v, id),
        (Method::Get, ["api", "battles", id, "replay"]) => {
            // Only for the uploader (GDPR access); never public.
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let mine = {
                let st = app.st();
                st.battles.get(*id).is_some_and(|s| owns(&st, &owner, s))
            };
            if !mine {
                return Reply::err(404, "not found");
            }
            match std::fs::File::open(app.data.join("battles").join(format!("{id}.wotreplay"))) {
                Ok(f) => {
                    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                    Reply::file(f, 0, len).header("Content-Disposition", format!("attachment; filename=\"{id}.wotreplay\""))
                }
                Err(_) => Reply::err(404, "no replay file"),
            }
        }
        (Method::Get, ["api", "battles", id, "data"]) => data(app, rq, v, id, q),
        (Method::Get, ["api", "live"]) => {
            let (owners, valid) = unlocked(app, &app.st(), rq, v);
            let t = now();
            let snap = app.snapshot();
            // Newest first: the first row of an owner is its latest battle.
            let latest_name = |owner: &str| snap.rows.iter().filter(|r| r.1 == owner).max_by_key(|r| r.0.updated).map(|r| r.0.player.clone()).unwrap_or_default();
            let mut battles: Vec<BattleInfo> = if owners.is_empty() {
                Vec::new()
            } else {
                snap.rows.iter().filter(|(i, o)| info_live(i, t) && owners.contains(o)).map(|(i, _)| (**i).clone()).collect()
            };
            battles.sort_by(|a, b| b.updated.cmp(&a.updated));
            let battles = one_per_battle(battles);
            let valid = valid.into_iter().map(|(code, owner, expires)| CodeStatus { code, player: latest_name(&owner), expires }).collect();
            Reply::json(&LiveList { battles, valid })
        }
        (Method::Post, ["api", "codes"]) => {
            let owner = match uploader(app, rq) {
                Ok(o) => o,
                Err(e) => return e,
            };
            let Ok(body) = read_body(rq, MAX_JSON) else { return Reply::err(400, "bad body") };
            let Ok(req) = serde_json::from_slice::<NewCode>(&body) else { return Reply::err(400, "bad json") };
            let days = req.days.clamp(1, MAX_CODE_DAYS) as u64;
            let code = token(10);
            let expires = now() + days * 86400;
            let mut st = app.st();
            let t = now();
            st.codes.retain(|_, c| c.expires > t);
            if st.codes.values().filter(|c| c.owner == owner).count() >= 100 || st.codes.len() >= MAX_CODES || !app.guard.take(v, Daily::Codes, 1) {
                return Reply::err(429, "too many codes");
            }
            st.codes.insert(sha_hex(code.as_bytes()), Code { owner, expires });
            st.dirty.codes = true;
            Reply::json(&CodeInfo { code, expires })
        }
        (Method::Delete, ["api", "codes", code]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let h = sha_hex(normalize_code(code).as_bytes());
            let mut st = app.st();
            if st.codes.get(&h).is_some_and(|c| c.owner == owner) {
                st.codes.remove(&h);
                st.dirty.codes = true;
            }
            Reply::json(&())
        }
        (Method::Get | Method::Head, ["api", "assets", name]) if valid_asset_name(name) => {
            app.ensure_hash(name);
            // Description and file together (a copy promoted meanwhile can't mix them up).
            let (meta, file) = {
                let st = app.st();
                let m = st.assets.get(*name).map(|m| m.current.clone());
                let f = if m.is_some() && method == Method::Get { std::fs::File::open(app.asset_path(name)).ok() } else { None };
                (m, f)
            };
            let Some(c) = meta else { return Reply::err(404, "not found") };
            let etag = if c.hash.is_empty() { format!("\"{}\"", &sha_hex(format!("{}{}", c.version, c.size).as_bytes())[..16]) } else { format!("\"{}\"", &c.hash[..16]) };
            let tagged = |r: Reply| r.header("ETag", etag.clone()).header("Cache-Control", "no-cache").header(VERSION_HEADER, c.version.clone()).header(HASH_HEADER, c.hash.clone());
            if header(rq, "If-None-Match") == Some(etag.as_str()) {
                return tagged(Reply { status: 304, body: Body::Bytes(Arc::from(&[][..])), ctype: "application/octet-stream", headers: vec![], compress: false });
            }
            if method == Method::Head {
                return tagged(Reply::bin(vec![]));
            }
            match file {
                Some(f) => {
                    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                    tagged(Reply::file(f, 0, len))
                }
                None => Reply::err(404, "not found"),
            }
        }
        (Method::Put, ["api", "assets", name]) if valid_asset_name(name) => put_asset(app, rq, v, name, q),
        (Method::Post, ["api", "assets", name, "vote"]) if valid_asset_name(name) => {
            let owner = match uploader(app, rq) {
                Ok(o) => o,
                Err(e) => return e,
            };
            let hash = query(q, "hash").unwrap_or_default();
            if !assets::valid_hash(&hash) {
                return Reply::err(400, "bad hash");
            }
            app.ensure_hash(name);
            // Before the state is locked (the snapshot locks it itself).
            let counted = app.counts(v, &owner);
            let mut st = app.st();
            let Some(mut m) = st.assets.get(*name).cloned() else { return Reply::json(&AssetVote { upload: true, counted: false }) };
            if !m.has(&hash) {
                // Only a copy whose vote would count is kept as a candidate.
                return Reply::json(&AssetVote { upload: counted && m.room().is_ok(), counted: false });
            }
            if counted {
                if let Some(i) = m.vote(&owner, &hash, now()) {
                    app.promote(name, &mut m, i);
                }
                app.save_asset(name, &m);
                st.assets.insert(name.to_string(), m);
            }
            Reply::json(&AssetVote { upload: false, counted })
        }
        (Method::Post, ["api", "asset-expect"]) => {
            if let Err(e) = uploader(app, rq) {
                return e;
            }
            let Ok(body) = read_body(rq, MAX_JSON) else { return Reply::err(400, "bad body") };
            let Ok(names) = serde_json::from_slice::<Vec<String>>(&body) else { return Reply::err(400, "bad json") };
            let t = now();
            let mut st = app.st();
            st.expected.retain(|_, until| *until > t);
            for n in names.iter().filter(|n| valid_asset_name(n)).take(300) {
                if st.expected.len() < 20_000 && !st.assets.contains_key(n) {
                    st.expected.insert(n.clone(), t + 600);
                }
            }
            Reply::json(&())
        }
        (Method::Get, ["api", "asset-status"]) => {
            let t = now();
            let st = app.st();
            let names = query(q, "names").unwrap_or_default();
            let out: HashMap<String, AssetStatus> = names
                .split(',')
                .filter(|n| valid_asset_name(n))
                .take(300)
                .map(|n| {
                    let s = if let Some(m) = st.assets.get(n) {
                        AssetStatus { state: "ready".into(), size: m.current.size, ..Default::default() }
                    } else if let Some(&(received, total)) = st.uploading.get(n) {
                        AssetStatus { state: "uploading".into(), received, total, ..Default::default() }
                    } else if st.expected.get(n).is_some_and(|until| *until > t) {
                        AssetStatus { state: "expected".into(), ..Default::default() }
                    } else {
                        AssetStatus { state: "missing".into(), ..Default::default() }
                    };
                    (n.to_string(), s)
                })
                .collect();
            Reply::json(&out)
        }
        (Method::Get | Method::Head, _) if !path.starts_with("/api/") => static_file(app, rq, path),
        _ => Reply::err(404, "not found"),
    }
}

/// Everything of one uploader key: its uploads (also those for its player names by older keys),
/// codes, player names and asset votes. Returns how many battles went.
fn forget_uploader(app: &App, st: &mut State, owner: &str) -> usize {
    let view: &State = st;
    let ids: Vec<String> = view.battles.values().filter(|s| owns(view, owner, s)).map(|s| s.info.id.clone()).collect();
    for id in &ids {
        app.remove_battle(st, id);
    }
    let n = st.codes.len();
    st.codes.retain(|_, c| c.owner != owner);
    st.dirty.codes |= n != st.codes.len();
    let n = st.claims.len();
    st.claims.retain(|_, k| *k != owner);
    st.dirty.claims |= n != st.claims.len();
    let voted: Vec<String> = st.assets.iter().filter(|(_, m)| m.copies().any(|c| c.votes.iter().any(|x| x.key == owner))).map(|(n, _)| n.clone()).collect();
    for name in voted {
        if let Some(m) = st.assets.get_mut(&name) {
            m.forget(owner);
            let m = m.clone();
            app.save_asset(&name, &m);
        }
    }
    ids.len()
}

/// "1.30.1.2 #123" -> comparable numbers.
#[cfg(test)]
fn version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

fn put_replay(app: &App, rq: &mut Request, v: Visitor, id: &str) -> Reply {
    let owner = match uploader(app, rq) {
        Ok(o) => o,
        Err(e) => return e,
    };
    // Checked before the file is received, and again after.
    let mine = |st: &State| match st.battles.get(id) {
        None => Err(Reply::err(404, "not found")),
        Some(s) if s.owner != owner => Err(Reply::err(403, "not yours")),
        Some(s) => Ok(s.info.replay_size),
    };
    if let Err(e) = mine(&app.st()) {
        return e;
    }
    let r = match app.receive(rq, v, MAX_REPLAY, None) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // A replay file starts with the magic 0x11343212.
    if r.size < 12 || r.head[..4] != [0x12, 0x32, 0x34, 0x11] {
        return Reply::err(400, "not a replay file");
    }
    let mut st = app.st();
    let old = match mine(&st) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if let Some(e) = app.over_limit(&st, &owner, r.size.saturating_sub(old)) {
        return e;
    }
    if r.file.place(&app.data.join("battles").join(format!("{id}.wotreplay"))).is_err() {
        return Reply::err(500, "write failed");
    }
    let Some(s) = st.battles.get_mut(id) else { return Reply::err(404, "not found") };
    Arc::make_mut(&mut s.info).replay_size = r.size;
    let s = s.clone();
    app.save_battle(&s);
    Reply::json(&s.info)
}

fn put_asset(app: &App, rq: &mut Request, v: Visitor, name: &str, q: &str) -> Reply {
    let owner = match uploader(app, rq) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let version = asset_version(&query(q, "version").unwrap_or_default());
    {
        let st = app.st();
        if !st.assets.contains_key(name) && !app.guard.fits(v, Daily::NewAssets, 1) {
            return Reply::err(429, "too many new assets from here today");
        }
        if !st.assets.contains_key(name) && st.assets.len() >= MAX_ASSETS {
            return Reply::err(507, "too many assets");
        }
    }
    app.ensure_hash(name);
    // Map 3D packs can be large (big Steel Hunter maps ~30 MB).
    let r = match app.receive(rq, v, MAX_ASSET, Some(name)) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // Before the state is locked (the snapshot locks it itself).
    let counted = app.counts(v, &owner);
    let mut st = app.st();
    st.expected.remove(name);
    let t = now();
    let copy = Copy { hash: r.hash.clone(), version, size: r.size, owner: owner.clone(), votes: vec![] };
    let Some(mut m) = st.assets.get(name).cloned() else {
        // The first copy is served at once.
        if st.assets.len() >= MAX_ASSETS || !app.guard.take(v, Daily::NewAssets, 1) {
            return Reply::err(507, "too many assets");
        }
        if let Some(e) = app.over_limit(&st, &owner, r.size) {
            return e;
        }
        if r.file.place(&app.asset_path(name)).is_err() {
            return Reply::err(500, "write failed");
        }
        let mut m = AssetMeta { current: copy, others: vec![], pinned: false };
        if counted {
            m.vote(&owner, &r.hash, t);
        }
        app.save_asset(name, &m);
        st.assets.insert(name.to_string(), m);
        return Reply::json(&AssetVote { upload: false, counted });
    };
    if !m.has(&r.hash) {
        // Another copy: kept as a candidate only if its uploader's vote counts (a made-up key
        // can't fill the places or push out a real copy), and if there is room (a candidate
        // with at most one vote makes way; one with support doesn't).
        if !counted {
            return Reply::json(&AssetVote { upload: false, counted: false });
        }
        if let Some(e) = app.over_limit(&st, &owner, r.size) {
            return e;
        }
        match m.room() {
            Err(()) => return Reply::json(&AssetVote { upload: false, counted: false }),
            Ok(Some(i)) => {
                let gone = m.others.remove(i);
                let _ = std::fs::remove_file(app.candidate_path(name, &gone.hash));
                *app.usage.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            Ok(None) => {}
        }
        if r.file.place(&app.candidate_path(name, &r.hash)).is_err() {
            return Reply::err(500, "write failed");
        }
        m.others.push(copy);
    }
    if counted {
        if let Some(i) = m.vote(&owner, &r.hash, t) {
            app.promote(name, &mut m, i);
        }
    }
    app.save_asset(name, &m);
    st.assets.insert(name.to_string(), m);
    Reply::json(&AssetVote { upload: false, counted })
}

fn new_battle(app: &App, rq: &mut Request, v: Visitor) -> Reply {
    let owner = match uploader(app, rq) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let body = match read_body(rq, MAX_JSON) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if !app.guard.heavy(v) {
        return Reply::err(429, "too many requests");
    }
    let Ok(mut nb) = serde_json::from_slice::<NewBattle>(&body) else { return Reply::err(400, "bad json") };
    // Everything bounded and plain before it is used or kept.
    nb.realm = nb.realm.trim().to_ascii_uppercase();
    let map_ok = (1..=80).contains(&nb.map.len()) && nb.map.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let realm_ok = (1..=8).contains(&nb.realm.len()) && nb.realm.bytes().all(|b| b.is_ascii_alphanumeric());
    if !valid_player(&nb.player) || !map_ok || !realm_ok {
        return Reply::err(400, "bad battle");
    }
    for f in [&mut nb.map_display, &mut nb.mode, &mut nb.tank, &mut nb.version] {
        *f = clean(f, MAX_TEXT);
    }
    nb.date = clean(&nb.date, 32);
    nb.file_name = clean(&nb.file_name, 160);
    nb.vehicle_ids.truncate(200);
    nb.vehicle_ids.sort_unstable();
    nb.vehicle_ids.dedup();
    nb.players.truncate(200);
    let players: Vec<String> = nb.players.iter().filter(|p| valid_player(p)).map(|p| p.to_lowercase()).collect();
    nb.allies.truncate(100);
    let allies: Vec<String> = nb.allies.iter().filter(|p| valid_player(p)).map(|p| p.to_lowercase()).collect();
    let mut st = app.st();
    // The first key that uploads for a player name owns it.
    let claim = claim_key(&nb.realm, &nb.player);
    match st.claims.get(&claim) {
        Some(k) if *k != owner => {
            let c = contact();
            let msg = if c.is_empty() {
                "this player name belongs to another uploader".to_string()
            } else {
                format!("this player name belongs to another uploader. If it is yours, write to {c}")
            };
            return Reply::err(403, &msg);
        }
        Some(_) => {}
        None if st.claims.len() >= MAX_CLAIMS || st.claims.values().filter(|k| **k == owner).count() >= CLAIMS_PER_KEY => {
            return Reply::err(429, "too many player names for one uploader")
        }
        None if !app.guard.take(v, Daily::Claims, 1) => return Reply::err(429, "too many new player names from here today"),
        None => {
            st.claims.insert(claim, owner.clone());
            st.dirty.claims = true;
        }
    }
    // The same battle again for this player (app restart, a manual upload, the finished replay of
    // a live upload, or an older key of the same player): that entry, never a second one. This key
    // owns the player name (checked above), so it takes the entry over.
    let same_player = |s: &Stored| s.info.player.eq_ignore_ascii_case(&nb.player) && s.info.realm.eq_ignore_ascii_case(&nb.realm);
    let same = st
        .battles
        .values()
        .filter(|s| same_player(s))
        .find(|s| (nb.arena_id != 0 && s.arena_id == nb.arena_id) || (!nb.file_name.is_empty() && s.info.file_name == nb.file_name))
        .map(|s| s.info.id.clone());
    if let Some(id) = same {
        let s = st.battles.get_mut(&id).unwrap();
        let mut changed = s.owner != owner;
        s.owner = owner.clone();
        if !nb.live && !nb.file_name.is_empty() && s.info.file_name.is_empty() {
            Arc::make_mut(&mut s.info).file_name = nb.file_name.clone();
            changed = true;
        }
        if s.arena_id == 0 && nb.arena_id != 0 {
            s.arena_id = nb.arena_id;
            changed = true;
        }
        if s.allies.is_empty() && !allies.is_empty() {
            s.allies = allies.clone();
            changed = true;
        }
        let s = s.clone();
        if changed {
            app.save_battle(&s);
        }
        return Reply::json(&s.info);
    }
    if !nb.live && !nb.file_name.is_empty() {
        // The finished replay of this uploader's live upload (live entries have no file name; their
        // date is when the game started writing, a few minutes before the replay's own): that entry.
        let at = date_secs(&nb.date);
        let live = st
            .battles
            .values()
            .filter(|s| same_player(s) && s.info.file_name.is_empty() && s.info.map == nb.map)
            .filter_map(|s| Some((s.info.id.clone(), (date_secs(&s.info.date)? - at?).abs())))
            .filter(|(_, d)| *d <= 600)
            .min_by_key(|(_, d)| *d)
            .map(|(id, _)| id);
        if let Some(id) = live {
            let s = st.battles.get_mut(&id).unwrap();
            s.owner = owner.clone();
            Arc::make_mut(&mut s.info).file_name = nb.file_name.clone();
            let s = s.clone();
            app.save_battle(&s);
            return Reply::json(&s.info);
        }
    }
    let t = now();
    let today = st.battles.values().filter(|s| s.owner == owner && t.saturating_sub(s.created) < 86400).count();
    if today >= env_num("MAX_BATTLES_PER_DAY", BATTLES_PER_DAY as u64) as usize || !app.guard.take(v, Daily::Battles, 1) {
        return Reply::err(429, "upload limit reached for today");
    }
    let id = loop {
        let id = token(10).to_ascii_lowercase();
        if !st.battles.contains_key(&id) {
            break id;
        }
    };
    let mut info = BattleInfo {
        id: id.clone(),
        player: nb.player,
        realm: nb.realm,
        map: nb.map,
        map_display: nb.map_display,
        mode: nb.mode,
        tank: nb.tank,
        date: nb.date,
        version: nb.version,
        file_name: nb.file_name,
        live: nb.live,
        damage: 0,
        size: 0,
        gen: 1,
        updated: now(),
        group: String::new(),
        replay_size: 0,
        uploaders: Vec::new(),
        interrupted: false,
    };
    // Another player's upload of the same battle: the same arenaUniqueID (also while live);
    // for uploads without it, same realm, map and day and shared tanks (finished battles only).
    let day = |d: &str| d.get(..10).unwrap_or("").to_string();
    let mine: HashSet<i32> = nb.vehicle_ids.iter().copied().collect();
    let other_player = |s: &&Stored| !s.info.player.eq_ignore_ascii_case(&info.player);
    let mate = if nb.arena_id != 0 {
        st.battles.values().filter(other_player).find(|s| s.arena_id == nb.arena_id).map(|s| s.info.id.clone())
    } else {
        st.battles
            .values()
            .filter(other_player)
            .filter(|s| s.arena_id == 0 && !is_live(s, t) && s.info.realm == info.realm && s.info.map == info.map && day(&s.info.date) == day(&info.date))
            .find(|s| s.vehicle_ids.iter().filter(|v| mine.contains(v)).count() >= 3)
            .map(|s| s.info.id.clone())
    };
    if let Some(mate) = mate {
        let m = st.battles.get_mut(&mate).unwrap();
        if m.info.group.is_empty() {
            Arc::make_mut(&mut m.info).group = mate.clone();
            let m = m.clone();
            app.save_battle(&m);
        }
        info.group = st.battles[&mate].info.group.clone();
    }
    let s = Stored { info: Arc::new(info.clone()), owner, created: t, vehicle_ids: nb.vehicle_ids, players, arena_id: nb.arena_id, allies };
    app.save_battle(&s);
    st.battles.insert(id, s);
    Reply::json(&info)
}

fn frames(app: &App, rq: &mut Request, v: Visitor, id: &str, q: &str) -> Reply {
    let owner = match uploader(app, rq) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let body = match read_body(rq, MAX_BODY) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if !whole_frames(&body) {
        return Reply::err(400, "bad frames");
    }
    let reset = query(q, "reset").is_some();
    let len = body.len() as u64;
    if !app.guard.take(v, Daily::Bytes, len) {
        return Reply::err(429, "upload limit reached for today");
    }
    // One writer per battle; the file is written with the shared state unlocked.
    let io = app.io_lock(id);
    let _io = io.lock().unwrap_or_else(|e| e.into_inner());
    let base = {
        let st = app.st();
        let Some(s) = st.battles.get(id) else { return Reply::err(404, "not found") };
        if s.owner != owner {
            return Reply::err(403, "not yours");
        }
        // Finished battles are immutable (they are cached forever) unless replaced as a whole.
        if !s.info.live && s.info.size > 0 && !reset {
            return Reply::err(409, "battle is finished");
        }
        let base = if reset { 0 } else { s.info.size };
        if base + len > MAX_BATTLE {
            return Reply::err(413, "battle too large");
        }
        // Storage limits: per uploader and for the whole server.
        if let Some(e) = app.over_limit(&st, &owner, (base + len).saturating_sub(s.info.size)) {
            return e;
        }
        base
    };
    let path = app.data.join("battles").join(format!("{id}.bin"));
    let ok = if reset {
        write_atomic(&path, &body).is_ok()
    } else {
        // Anything after the known size is left from a failed earlier write: cut it off.
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .and_then(|mut f| {
                f.set_len(base)?;
                f.seek(std::io::SeekFrom::Start(base))?;
                f.write_all(&body)
            })
            .is_ok()
    };
    if !ok {
        return Reply::err(500, "write failed");
    }
    let mut st = app.st();
    let Some(s) = st.battles.get_mut(id) else {
        // Deleted meanwhile.
        let _ = std::fs::remove_file(&path);
        return Reply::err(404, "not found");
    };
    if reset {
        Arc::make_mut(&mut s.info).gen += 1;
    }
    // Lists change when a battle gets its first data or ends: made again at once.
    let listed_change = s.info.size == 0 || query(q, "finish").is_some();
    Arc::make_mut(&mut s.info).size = base + len;
    Arc::make_mut(&mut s.info).updated = now();
    if query(q, "finish").is_some() {
        Arc::make_mut(&mut s.info).live = false;
        Arc::make_mut(&mut s.info).interrupted = query(q, "interrupted").is_some();
    }
    if let Some(d) = query(q, "damage").and_then(|d| d.parse::<i64>().ok()) {
        Arc::make_mut(&mut s.info).damage = d.clamp(0, 1_000_000);
    }
    let s = s.clone();
    if listed_change {
        st.epoch += 1;
    }
    app.save_battle(&s);
    Reply::json(&s.info)
}

fn data(app: &App, rq: &mut Request, v: Visitor, id: &str, q: &str) -> Reply {
    let (info, owner, owners) = {
        let st = app.st();
        let Some(s) = st.battles.get(id) else { return Reply::err(404, "not found") };
        let owners = if is_live(s, now()) { unlocked(app, &st, rq, v).0 } else { Vec::new() };
        ((*s.info).clone(), s.owner.clone(), owners)
    };
    let allowed = !is_live_info(&info, now()) || may_watch(&app.snapshot(), &info, &owner, &owners, now());
    if !allowed {
        return Reply::err(403, "live battle: needs a viewing code from the player");
    }
    let gen_ok = query(q, "gen").and_then(|g| g.parse::<u32>().ok()) == Some(info.gen);
    let from = if gen_ok { query(q, "from").and_then(|f| f.parse::<u64>().ok()).unwrap_or(0).min(info.size) } else { 0 };
    let tag = |r: Reply| {
        let finished = !info.live;
        r.header(GEN_HEADER, info.gen.to_string())
            .header("X-Livereplay-Finished", if finished { "1" } else { "0" })
            .header("Cache-Control", if finished && from == 0 { "public, max-age=31536000, immutable" } else { "no-store" })
    };
    let f = match std::fs::File::open(app.data.join("battles").join(format!("{id}.bin"))) {
        Ok(f) => f,
        Err(_) => return Reply::bin(vec![]).header(GEN_HEADER, info.gen.to_string()),
    };
    // Never promise more than the file has (it may have been replaced meanwhile).
    let end = info.size.min(f.metadata().map(|m| m.len()).unwrap_or(0));
    tag(Reply::file(f, from.min(end), end - from.min(end)))
}

/// Several uploads of one battle (a group) shown once, with every uploader's name: the first
/// upload of it (the group is named after it), so a later upload can't take its place.
/// Works on references; returns the entries kept and each group's uploader names.
fn group_view<'a>(v: &[&'a BattleInfo]) -> (Vec<&'a BattleInfo>, HashMap<&'a str, Vec<&'a str>>) {
    let mut names: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut roots = HashSet::new();
    for b in v.iter().filter(|b| !b.group.is_empty()) {
        let n = names.entry(b.group.as_str()).or_default();
        if !n.contains(&b.player.as_str()) {
            n.push(&b.player);
        }
        if b.id == b.group {
            roots.insert(b.group.as_str());
        }
    }
    let mut seen = HashSet::new();
    let kept = v.iter().copied().filter(|b| b.group.is_empty() || if roots.contains(b.group.as_str()) { b.id == b.group } else { seen.insert(b.group.as_str()) }).collect();
    (kept, names)
}

/// The uploaders of an entry's group, when more than one.
fn with_uploaders(mut b: BattleInfo, names: &HashMap<&str, Vec<&str>>) -> BattleInfo {
    if let Some(n) = names.get(b.group.as_str()).filter(|n| n.len() > 1) {
        b.uploaders = n.iter().map(|s| s.to_string()).collect();
    }
    b
}

fn one_per_battle(v: Vec<BattleInfo>) -> Vec<BattleInfo> {
    let refs: Vec<&BattleInfo> = v.iter().collect();
    let (kept, names) = group_view(&refs);
    kept.into_iter().map(|b| with_uploaders(b.clone(), &names)).collect()
}

/// A battle in a list: still live, or interrupted (a live upload that stopped).
fn shown_info(i: &BattleInfo, t: u64) -> BattleInfo {
    let mut i = i.clone();
    if i.live && t.saturating_sub(i.updated) >= STALE_SECS {
        i.live = false;
        i.interrupted = true;
    }
    i
}

fn is_live_info(i: &BattleInfo, t: u64) -> bool {
    info_live(i, t)
}

fn info_live(i: &BattleInfo, t: u64) -> bool {
    i.live && t.saturating_sub(i.updated) < STALE_SECS
}

fn list(app: &App, rq: &Request, q: &str, v: Visitor) -> Reply {
    let player = query(q, "player").unwrap_or_default().to_lowercase();
    let group = query(q, "group").unwrap_or_default();
    let page = query(q, "page").and_then(|p| p.parse().ok()).unwrap_or(0usize).min(100_000);
    // What everyone sees (no key, no codes) is the same for everyone: answered from a cache.
    let public = group.is_empty() || (key_hash(rq).is_none() && header(rq, CODES_HEADER).is_none_or(str::is_empty));
    let cache_key = format!("{player}\n{group}\n{page}");
    let epoch = app.st().epoch;
    if public {
        if let Some((e, at, b)) = app.list_cache.lock().unwrap_or_else(|e| e.into_inner()).get(&cache_key) {
            if *e == epoch && at.elapsed() < LIST_CACHE {
                return Reply::json_bytes(b.clone());
            }
        }
    }
    // Made-up queries (each one new) can't be cached: they count as expensive.
    if !player.is_empty() && !app.guard.heavy(v) {
        return Reply::err(429, "too many searches, try again in a moment");
    }
    let t = now();
    // A group's live members too, for whoever may see them (viewing code, or the uploader).
    let owners = if group.is_empty() { Vec::new() } else { unlocked(app, &app.st(), rq, v).0 };
    let snap = app.snapshot();
    // References only; just the page's entries are copied.
    let v: Vec<&BattleInfo> = snap
        .rows
        .iter()
        .filter(|(i, _)| group.is_empty() || i.group == group)
        .filter(|(i, owner)| !info_live(i, t) || may_watch(&snap, i, owner, &owners, t))
        .filter(|(i, _)| player.is_empty() || i.player.to_lowercase().contains(&player))
        .map(|(i, _)| &**i)
        .collect();
    // The battle list: one entry per battle; a group's own list: every viewpoint.
    let (v, names) = if group.is_empty() { group_view(&v) } else { (v, HashMap::new()) };
    let total = v.len();
    let battles = v.into_iter().skip(page * PER_PAGE).take(PER_PAGE).map(|i| with_uploaders(shown_info(i, t), &names)).collect();
    let body: Arc<[u8]> = serde_json::to_vec(&BattleList { battles, total, page, per_page: PER_PAGE }).unwrap_or_default().into();
    if public {
        let mut c = app.list_cache.lock().unwrap_or_else(|e| e.into_inner());
        if c.len() > 2000 {
            c.clear();
        }
        c.insert(cache_key, (epoch, Instant::now(), body.clone()));
    }
    Reply::json_bytes(body)
}

fn static_file(app: &App, rq: &Request, path: &str) -> Reply {
    // Unknown paths get the viewer page (it routes by URL itself).
    let f = app.web.get(path).or_else(|| app.web.get("/index.html"));
    let Some(f) = f else { return Reply::err(404, "web viewer not installed") };
    if header(rq, "If-None-Match") == Some(f.etag.as_str()) {
        return Reply { status: 304, body: Body::Bytes(Arc::from(&[][..])), ctype: f.ctype, headers: vec![("ETag".into(), f.etag.clone())], compress: false };
    }
    let accept = header(rq, "Accept-Encoding").unwrap_or("");
    let mut r = Reply { status: 200, body: Body::Bytes(f.body.clone()), ctype: f.ctype, headers: vec![("ETag".into(), f.etag.clone()), ("Cache-Control".into(), "no-cache".into())], compress: false };
    match (&f.br, &f.gz) {
        (Some(b), _) if accept.contains("br") => {
            r.body = Body::Bytes(b.clone());
            r.headers.push(("Content-Encoding".into(), "br".into()));
        }
        (_, Some(z)) if accept.contains("gzip") => {
            r.body = Body::Bytes(z.clone());
            r.headers.push(("Content-Encoding".into(), "gzip".into()));
        }
        _ => {}
    }
    r.headers.push(("Vary".into(), "Accept-Encoding".into()));
    r
}

fn respond(rq: Request, r: Reply) {
    let gz_ok = matches!(&r.body, Body::Bytes(b) if r.compress && b.len() > 512) && header(&rq, "Accept-Encoding").is_some_and(|a| a.contains("gzip"));
    let (reader, len): (Box<dyn Read + Send>, usize) = match r.body {
        Body::Bytes(b) if gz_ok => {
            let z = gzip(&b, 5);
            let n = z.len();
            (Box::new(std::io::Cursor::new(z)), n)
        }
        Body::Bytes(b) => {
            let n = b.len();
            (Box::new(std::io::Cursor::new(b)), n)
        }
        Body::Stream(s, n) => (s, n),
    };
    let mut resp = Response::new(StatusCode(r.status), vec![], reader, Some(len), None);
    let mut add = |k: &str, v: &str| {
        // Never a control character (CR/LF would start a header of the sender's choosing).
        if v.bytes().any(|b| b < 0x20 || b == 0x7f) || k.bytes().any(|b| !b.is_ascii_graphic() || b == b':') {
            return;
        }
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    };
    add("Content-Type", r.ctype);
    add("X-Content-Type-Options", "nosniff");
    add("Referrer-Policy", "no-referrer");
    add("Strict-Transport-Security", "max-age=31536000");
    // Pages: own scripts and the wasm only, no framing. Everything else (data, uploads, errors) can
    // never act as a page, whatever it contains.
    add(
        "Content-Security-Policy",
        if r.ctype.starts_with("text/html") {
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
             connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
        } else {
            "default-src 'none'; sandbox; frame-ancestors 'none'"
        },
    );
    if gz_ok {
        add("Content-Encoding", "gzip");
        add("Vary", "Accept-Encoding");
    }
    for (k, v) in &r.headers {
        add(k, v);
    }
    let _ = rq.respond(resp);
}

/// Housekeeping every minute: expired codes and votes, battles that never got data, memory.
fn maintain(app: &App) {
    let t = now();
    {
        let mut st = app.st();
        let n = st.codes.len();
        st.codes.retain(|_, c| c.expires > t);
        st.dirty.codes |= n != st.codes.len();
        st.expected.retain(|_, until| *until > t);
        let empty: Vec<String> = st.battles.values().filter(|s| s.info.size == 0 && s.info.replay_size == 0 && t.saturating_sub(s.created) > EMPTY_BATTLE_SECS).map(|s| s.info.id.clone()).collect();
        for id in &empty {
            app.remove_battle(&mut st, id);
        }
        let mut changed = Vec::new();
        for (name, m) in st.assets.iter_mut() {
            if m.expire(t) {
                changed.push((name.clone(), m.clone()));
            }
        }
        for (name, m) in changed {
            app.save_asset(&name, &m);
        }
    }
    app.io_locks.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, l| Arc::strong_count(l) > 1);
    app.list_cache.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, (_, at, _)| at.elapsed() < LIST_CACHE);
    app.guard.tidy();
    app.flush();
}

fn main() {
    let bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let data = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "data".into()));
    let web = PathBuf::from(std::env::var("WEB_DIR").unwrap_or_else(|_| "web-dist".into()));
    // Many workers (threads cost little while they wait); one visitor can hold only a few.
    let threads: usize = std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(64);
    let app = Arc::new(App::load(data, &web));
    let server = Arc::new(Server::http(&bind).expect("bind"));
    println!("listening on http://{bind}");
    {
        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| maintain(&app)));
        });
    }
    let workers: Vec<_> = (0..threads.max(1))
        .map(|_| {
            let (app, server) = (app.clone(), server.clone());
            std::thread::spawn(move || {
                for mut rq in server.incoming_requests() {
                    let v = app.guard.visitor(rq.remote_addr(), header(&rq, "X-Forwarded-For"));
                    let busy = if app.guard.request(v) { app.guard.enter(v) } else { None };
                    let r = match &busy {
                        Some(_) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| route(&app, &mut rq, v))).unwrap_or_else(|_| Reply::err(500, "internal error")),
                        None => Reply::err(429, "too many requests").header("Retry-After", "5"),
                    };
                    // Still counted while the answer is sent (a slow reader holds a worker too).
                    respond(rq, r);
                    drop(busy);
                    app.flush();
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(id: &str, owner: &str, map: &str, tanks: &[i32], size: u64, created: u64) -> Stored {
        let info = Arc::new(BattleInfo { id: id.into(), player: "p".into(), map: map.into(), size, live: true, ..Default::default() });
        Stored { info, owner: owner.into(), created, vehicle_ids: tanks.to_vec(), players: vec![], arena_id: 0, allies: vec![] }
    }

    #[test]
    fn same_battle_from_two_keys() {
        let dir = std::env::temp_dir().join(format!("lr-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("battles")).unwrap();
        let mut st = State::default();
        st.claims.insert(claim_key("EU", "p"), "new".into());
        let mut a = stored("a", "old", "m", &[], 500, 1);
        Arc::make_mut(&mut a.info).file_name = "x.wotreplay".into();
        Arc::make_mut(&mut a.info).realm = "EU".into();
        Arc::make_mut(&mut a.info).live = false;
        let mut b = stored("b", "new", "m", &[], 400, 2);
        Arc::make_mut(&mut b.info).file_name = "x.wotreplay".into();
        Arc::make_mut(&mut b.info).realm = "EU".into();
        Arc::make_mut(&mut b.info).live = false;
        Arc::make_mut(&mut b.info).replay_size = 900;
        let mut c = stored("c", "old", "m", &[], 300, 3);
        Arc::make_mut(&mut c.info).file_name = "other.wotreplay".into();
        Arc::make_mut(&mut c.info).realm = "EU".into();
        for s in [a, b, c] {
            st.battles.insert(s.info.id.clone(), s);
        }
        // a and b are one battle: b (with the original replay) stays; c is another battle.
        assert_eq!(merge_same_battles(&dir, &mut st), 1);
        assert!(st.battles.contains_key("b") && st.battles.contains_key("c") && !st.battles.contains_key("a"));
        // The current owner of the name may act on the old key's upload too.
        assert!(owns(&st, "new", &st.battles["c"]));
        assert!(!owns(&st, "someone", &st.battles["c"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_live_uploads() {
        let dir = std::env::temp_dir().join(format!("lr-dup-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("battles")).unwrap();
        let mut st = State::default();
        // One battle uploaded three times (app restarts), one other battle on the same map,
        // and the same battle from another uploader: only the two extra copies go.
        for s in [
            stored("a1", "k", "m", &[1, 2, 3, 4], 100, 1),
            stored("a2", "k", "m", &[1, 2, 3, 4, 5], 900, 2),
            stored("a3", "k", "m", &[2, 3, 4], 300, 3),
            stored("b1", "k", "m", &[7, 8, 9], 500, 4),
            stored("c1", "other", "m", &[1, 2, 3, 4], 100, 5),
        ] {
            st.battles.insert(s.info.id.clone(), s);
        }
        assert_eq!(remove_duplicates(&dir, &mut st), 2);
        let mut left: Vec<&String> = st.battles.keys().collect();
        left.sort();
        assert_eq!(left, ["a2", "b1", "c1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn group_shows_first_upload() {
        let b = |id: &str, group: &str, player: &str| BattleInfo { id: id.into(), group: group.into(), player: player.into(), ..Default::default() };
        // A later upload dated first can't stand in for the battle.
        let v = one_per_battle(vec![b("fake", "orig", "mallory"), b("x", "", "p"), b("orig", "orig", "alice")]);
        assert_eq!(v.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["x", "orig"]);
        assert_eq!(v[1].uploaders, ["mallory", "alice"]);
    }

    #[test]
    fn helpers() {
        assert!(whole_frames(&[2, 0, 0, 0, 9, 9, 1, 0, 0, 0, 5]));
        assert!(!whole_frames(&[2, 0, 0, 0, 9]));
        assert_eq!(url_decode("a%20b+c"), "a b c");
        assert_eq!(query("reset&finish=1", "reset").as_deref(), Some(""));
        assert_eq!(query("a=1&finish=1", "finish").as_deref(), Some("1"));
        assert_eq!(date_secs("01.01.1970 00:00:00"), Some(0));
        assert_eq!(date_secs("21.11.2026 01:43:04").zip(date_secs("21.11.2026 01:41:26")).map(|(a, b)| a - b), Some(98));
        assert_eq!(date_secs("01.03.2024 00:00:00").zip(date_secs("29.02.2024 23:59:59")).map(|(a, b)| a - b), Some(1));
        assert_eq!(date_secs("21.11.2026"), None);
        assert_eq!(date_secs("99.99.99999999999 1:1:1"), None);
        assert!(version_key("1.31.0.2") > version_key("1.30.1"));
        assert_eq!(token(10).len(), 10);
        assert!(token(200).bytes().all(|b| b"23456789ABCDEFGHJKMNPQRSTUVWXYZ".contains(&b)));
        assert_eq!(clean("a\r\nb\u{0}c  ", 100), "abc");
        assert_eq!(clean("abcdef", 3), "abc");
        assert!(valid_player("windows_2000") && valid_player("Ab-c") && !valid_player("") && !valid_player("a b") && !valid_player("a\nb"));
        assert_eq!(claim_key(" eu", "Windows_2000"), "EU:windows_2000");
        assert!(same_secret("token-123", "token-123") && !same_secret("token-123", "token-124"));
        assert_eq!(asset_version("World\u{a0}of\u{a0}Tanks v.2.4.1.1 #993"), "World of Tanks v.2.4.1.1 #993");
        assert_eq!(asset_version("1\r\nContent-Type: text/html"), "1  Content-Type  text html");
    }
}
