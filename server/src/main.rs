//! LiveReplay server.
//!
//! Stores battles uploaded by desktop clients (already decoded into the compact frame format, see
//! `src/replay/wire.rs`) and serves them, the game assets the web viewer needs (uploaded by clients,
//! never part of the repository) and the web viewer itself. Plain HTTP; HTTPS is done by a reverse
//! proxy (see `docker-compose.yml`).
//!
//! Environment: `BIND` (default `0.0.0.0:8080`), `DATA_DIR` (default `data`), `WEB_DIR`
//! (default `web-dist`), `THREADS` (default 4).
//!
//! Data folder: `claims.json` (player name -> uploader key hash), `codes.json` (code hash ->
//! owner key hash + expiry), `battles/<id>.json` + `battles/<id>.bin`, `assets/<name>.bin` + `.json`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use livereplay_api::*;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tiny_http::{Header, Method, Request, Response, Server};

mod privacy;

/// A live battle not updated for this long counts as finished (client crashed / quit).
const STALE_SECS: u64 = 30 * 60;
const MAX_BODY: usize = 16 << 20;
const MAX_ASSET: usize = 64 << 20;
/// Largest original replay file accepted.
const MAX_REPLAY: usize = 64 << 20;
const MAX_BATTLE: u64 = 24 << 20;
const PER_PAGE: usize = 50;
/// Abuse limits per uploader key (overridable: `MAX_BATTLES_PER_DAY`, `MAX_KEY_MB`, `MAX_TOTAL_GB`).
const BATTLES_PER_DAY: usize = 300;
const KEY_MB: u64 = 2048;
const TOTAL_GB: u64 = 50;
/// Player names one key may claim.
const CLAIMS_PER_KEY: usize = 10;

fn env_num(k: &str, default: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A battle date as the game writes it ("dd.mm.yyyy hh:mm:ss", the player's local time) in
/// seconds, for comparing two dates.
fn date_secs(d: &str) -> Option<i64> {
    let n: Vec<i64> = d.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().ok()).collect::<Option<_>>()?;
    let [day, m, y, h, mi, s] = n[..] else { return None };
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86400 + h * 3600 + mi * 60 + s)
}

fn sha_hex(b: &[u8]) -> String {
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Random token from an unambiguous alphabet (no 0/O, 1/I/L).
fn token(n: usize) -> String {
    const A: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).expect("os random");
    buf.iter().map(|b| A[*b as usize % A.len()] as char).collect()
}

#[derive(Serialize, Deserialize, Clone)]
struct Code {
    owner: String,
    expires: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct Stored {
    info: BattleInfo,
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
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct AssetMeta {
    version: String,
    size: u64,
}

#[derive(Default)]
struct State {
    claims: HashMap<String, String>,
    codes: HashMap<String, Code>,
    battles: HashMap<String, Stored>,
    assets: HashMap<String, AssetMeta>,
}

struct StaticFile {
    body: Vec<u8>,
    gz: Option<Vec<u8>>,
    br: Option<Vec<u8>>,
    etag: String,
    ctype: &'static str,
}

struct App {
    data: PathBuf,
    state: Mutex<State>,
    web: HashMap<String, StaticFile>,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_json<T: Serialize>(p: &Path, v: &T) {
    // Write to a temp file and rename, so a crash never leaves half a file.
    let tmp = p.with_extension("tmp");
    if let Ok(b) = serde_json::to_vec(v) {
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }
}

fn gzip(b: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
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

impl App {
    fn load(data: PathBuf, web_dir: &Path) -> Self {
        for d in ["battles", "assets"] {
            let _ = std::fs::create_dir_all(data.join(d));
        }
        let mut st = State { claims: read_json(&data.join("claims.json")), codes: read_json(&data.join("codes.json")), ..Default::default() };
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
                let gz = compressible.then(|| gzip(&body));
                let br = compressible.then(|| brotli_bytes(&body));
                let etag = format!("\"{}\"", &sha_hex(&body)[..16]);
                web.insert(format!("/{rel}"), StaticFile { body, gz, br, etag, ctype });
            }
        }
        // Expired viewing codes are deleted.
        let t = now();
        st.codes.retain(|_, c| c.expires > t);
        write_json(&data.join("codes.json"), &st.codes);
        println!("loaded {} battles, {} assets, {} web files", st.battles.len(), st.assets.len(), web.len());
        App { data, state: Mutex::new(st), web }
    }

    /// Delete a battle's files and entry (caller holds the state lock).
    fn remove_battle(&self, st: &mut State, id: &str) -> bool {
        let dir = self.data.join("battles");
        let _ = std::fs::remove_file(dir.join(format!("{id}.bin")));
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
        let _ = std::fs::remove_file(dir.join(format!("{id}.wotreplay")));
        st.battles.remove(id).is_some()
    }

    fn save_battle(&self, s: &Stored) {
        write_json(&self.data.join("battles").join(format!("{}.json", s.info.id)), s);
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    ctype: &'static str,
    headers: Vec<(String, String)>,
    /// Allow gzip for this body (text).
    compress: bool,
}

impl Reply {
    fn json<T: Serialize>(v: &T) -> Self {
        Reply { status: 200, body: serde_json::to_vec(v).unwrap_or_default(), ctype: "application/json", headers: vec![("Cache-Control".into(), "no-store".into())], compress: true }
    }
    fn err(status: u16, msg: &str) -> Self {
        Reply { status, body: msg.as_bytes().to_vec(), ctype: "text/plain; charset=utf-8", headers: vec![], compress: false }
    }
    fn bin(body: Vec<u8>) -> Self {
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
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| url_decode(v))
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

fn read_body(rq: &mut Request) -> Result<Vec<u8>, Reply> {
    read_body_max(rq, MAX_BODY)
}

fn read_body_max(rq: &mut Request, max: usize) -> Result<Vec<u8>, Reply> {
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

/// Owners (key hashes) whose live battles the given codes unlock, and the still-valid codes.
fn unlocked(st: &State, rq: &Request) -> (Vec<String>, Vec<(String, String, u64)>) {
    let t = now();
    let mut owners = Vec::new();
    let mut valid = Vec::new();
    for c in header(rq, CODES_HEADER).unwrap_or("").split(',').map(normalize_code).filter(|c| !c.is_empty()).take(50) {
        if let Some(code) = st.codes.get(&sha_hex(c.as_bytes())).filter(|x| x.expires > t) {
            owners.push(code.owner.clone());
            valid.push((c, code.owner.clone(), code.expires));
        }
    }
    if let Some(k) = key_hash(rq) {
        owners.push(k);
    }
    (owners, valid)
}

fn route(app: &App, rq: &mut Request) -> Reply {
    let url = rq.url().to_string();
    let (path, q) = url.split_once('?').unwrap_or((&url, ""));
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let method = rq.method().clone();
    match (&method, parts.as_slice()) {
        (Method::Post, ["api", "battles"]) => new_battle(app, rq),
        (Method::Get, ["privacy"]) => Reply { status: 200, body: privacy::page().into_bytes(), ctype: "text/html; charset=utf-8", headers: vec![], compress: true },
        (Method::Delete, ["api", "battles", id]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let mut st = app.state.lock().unwrap();
            match st.battles.get(*id) {
                Some(s) if s.owner == owner => {
                    app.remove_battle(&mut st, id);
                    Reply::json(&())
                }
                Some(_) => Reply::err(403, "not yours"),
                None => Reply::json(&()),
            }
        }
        (Method::Get, ["api", "me"]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let st = app.state.lock().unwrap();
            let t = now();
            let mut battles: Vec<BattleInfo> = st.battles.values().filter(|s| s.owner == owner).map(|s| s.info.clone()).collect();
            battles.sort_by(|a, b| b.updated.cmp(&a.updated));
            Reply::json(&MyData {
                player_names: st.claims.iter().filter(|(_, k)| **k == owner).map(|(n, _)| n.clone()).collect(),
                battles,
                code_expiries: st.codes.values().filter(|c| c.owner == owner && c.expires > t).map(|c| c.expires).collect(),
            })
        }
        (Method::Delete, ["api", "me"]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let mut st = app.state.lock().unwrap();
            let ids: Vec<String> = st.battles.values().filter(|s| s.owner == owner).map(|s| s.info.id.clone()).collect();
            for id in &ids {
                app.remove_battle(&mut st, id);
            }
            st.codes.retain(|_, c| c.owner != owner);
            st.claims.retain(|_, k| *k != owner);
            write_json(&app.data.join("codes.json"), &st.codes);
            write_json(&app.data.join("claims.json"), &st.claims);
            Reply::json(&ids.len())
        }
        (Method::Delete, ["api", "admin", "players", name]) => {
            let token = std::env::var("ADMIN_TOKEN").unwrap_or_default();
            if token.len() < 16 || header(rq, ADMIN_HEADER) != Some(token.as_str()) {
                return Reply::err(403, "admin token required");
            }
            let name = url_decode(name).to_lowercase();
            let mut st = app.state.lock().unwrap();
            let ids: Vec<String> = st
                .battles
                .values()
                .filter(|s| s.players.contains(&name) || s.info.player.to_lowercase() == name)
                .map(|s| s.info.id.clone())
                .collect();
            for id in &ids {
                app.remove_battle(&mut st, id);
            }
            // The name is free again: the next uploader of it claims it.
            st.claims.retain(|claim, _| claim.split_once(':').map_or(true, |(_, p)| p.to_lowercase() != name));
            write_json(&app.data.join("claims.json"), &st.claims);
            Reply::json(&ids)
        }
        (Method::Get, ["api", "battles"]) => list(app, q),
        (Method::Get, ["api", "battles", id]) => {
            let st = app.state.lock().unwrap();
            match st.battles.get(*id) {
                Some(s) if !is_live(s, now()) || unlocked(&st, rq).0.contains(&s.owner) => Reply::json(&s.info),
                _ => Reply::err(404, "not found"),
            }
        }
        (Method::Post, ["api", "battles", id, "frames"]) => frames(app, rq, id, q),
        (Method::Put, ["api", "battles", id, "replay"]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let body = match read_body_max(rq, MAX_REPLAY) {
                Ok(b) => b,
                Err(e) => return e,
            };
            // A replay file starts with the magic 0x11343212.
            if body.len() < 12 || body[..4] != [0x12, 0x32, 0x34, 0x11] {
                return Reply::err(400, "not a replay file");
            }
            let mut st = app.state.lock().unwrap();
            let used: u64 = st.battles.values().filter(|s| s.owner == owner).map(|s| s.info.size + s.info.replay_size).sum();
            let total: u64 = st.battles.values().map(|s| s.info.size + s.info.replay_size).sum();
            let Some(s) = st.battles.get_mut(*id) else { return Reply::err(404, "not found") };
            if s.owner != owner {
                return Reply::err(403, "not yours");
            }
            let extra = (body.len() as u64).saturating_sub(s.info.replay_size);
            if used + extra > env_num("MAX_KEY_MB", KEY_MB) << 20 || total + extra > env_num("MAX_TOTAL_GB", TOTAL_GB) << 30 {
                return Reply::err(507, "storage limit reached");
            }
            let path = app.data.join("battles").join(format!("{id}.wotreplay"));
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, &body).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
                return Reply::err(500, "write failed");
            }
            s.info.replay_size = body.len() as u64;
            let s = s.clone();
            app.save_battle(&s);
            Reply::json(&s.info)
        }
        (Method::Get, ["api", "battles", id, "replay"]) => {
            // Only for the uploader (GDPR access); never public.
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let mine = app.state.lock().unwrap().battles.get(*id).is_some_and(|s| s.owner == owner);
            if !mine {
                return Reply::err(404, "not found");
            }
            match std::fs::read(app.data.join("battles").join(format!("{id}.wotreplay"))) {
                Ok(b) => Reply::bin(b).header("Content-Disposition", format!("attachment; filename=\"{id}.wotreplay\"")),
                Err(_) => Reply::err(404, "no replay file"),
            }
        }
        (Method::Get, ["api", "battles", id, "data"]) => data(app, rq, id, q),
        (Method::Get, ["api", "live"]) => {
            let st = app.state.lock().unwrap();
            let (owners, valid) = unlocked(&st, rq);
            let t = now();
            let latest_name = |owner: &str| {
                st.battles.values().filter(|s| s.owner == owner).max_by_key(|s| s.info.updated).map(|s| s.info.player.clone()).unwrap_or_default()
            };
            let mut battles: Vec<BattleInfo> = st.battles.values().filter(|s| is_live(s, t) && owners.contains(&s.owner)).map(|s| s.info.clone()).collect();
            battles.sort_by(|a, b| b.updated.cmp(&a.updated));
            let valid = valid.into_iter().map(|(code, owner, expires)| CodeStatus { code, player: latest_name(&owner), expires }).collect();
            Reply::json(&LiveList { battles, valid })
        }
        (Method::Post, ["api", "codes"]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let Ok(body) = read_body(rq) else { return Reply::err(400, "bad body") };
            let Ok(req) = serde_json::from_slice::<NewCode>(&body) else { return Reply::err(400, "bad json") };
            let days = req.days.clamp(1, MAX_CODE_DAYS) as u64;
            let code = token(10);
            let expires = now() + days * 86400;
            let mut st = app.state.lock().unwrap();
            let t = now();
            st.codes.retain(|_, c| c.expires > t);
            if st.codes.values().filter(|c| c.owner == owner).count() >= 100 {
                return Reply::err(429, "too many codes");
            }
            st.codes.insert(sha_hex(code.as_bytes()), Code { owner, expires });
            write_json(&app.data.join("codes.json"), &st.codes);
            Reply::json(&CodeInfo { code, expires })
        }
        (Method::Delete, ["api", "codes", code]) => {
            let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
            let h = sha_hex(normalize_code(code).as_bytes());
            let mut st = app.state.lock().unwrap();
            if st.codes.get(&h).is_some_and(|c| c.owner == owner) {
                st.codes.remove(&h);
                write_json(&app.data.join("codes.json"), &st.codes);
            }
            Reply::json(&())
        }
        (Method::Get, ["api", "assets", name]) if valid_asset_name(name) => {
            let meta = app.state.lock().unwrap().assets.get(*name).cloned();
            let Some(meta) = meta else { return Reply::err(404, "not found") };
            let etag = format!("\"{}\"", sha_hex(format!("{}{}", meta.version, meta.size).as_bytes())[..16].to_string());
            if header(rq, "If-None-Match") == Some(etag.as_str()) {
                return Reply { status: 304, body: vec![], ctype: "application/octet-stream", headers: vec![("ETag".into(), etag)], compress: false };
            }
            match std::fs::read(app.data.join("assets").join(format!("{name}.bin"))) {
                Ok(b) => Reply::bin(b).header("ETag", etag).header("Cache-Control", "no-cache").header(VERSION_HEADER, meta.version),
                Err(_) => Reply::err(404, "not found"),
            }
        }
        (Method::Head, ["api", "assets", name]) if valid_asset_name(name) => {
            let meta = app.state.lock().unwrap().assets.get(*name).cloned();
            match meta {
                Some(m) => Reply::bin(vec![]).header(VERSION_HEADER, m.version),
                None => Reply::err(404, ""),
            }
        }
        (Method::Put, ["api", "assets", name]) if valid_asset_name(name) => {
            if key_hash(rq).is_none() {
                return Reply::err(401, "key required");
            }
            let version = query(q, "version").unwrap_or_default();
            // Map 3D packs can be large (big Steel Hunter maps ~30 MB).
            let body = match read_body_max(rq, MAX_ASSET) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let mut st = app.state.lock().unwrap();
            // Only replace with data from the same or a newer client version.
            if st.assets.get(*name).is_some_and(|m| version_key(&m.version) > version_key(&version)) {
                return Reply::json(&());
            }
            let dir = app.data.join("assets");
            if std::fs::write(dir.join(format!("{name}.bin")), &body).is_err() {
                return Reply::err(500, "write failed");
            }
            let meta = AssetMeta { version, size: body.len() as u64 };
            write_json(&dir.join(format!("{name}.json")), &meta);
            st.assets.insert(name.to_string(), meta);
            Reply::json(&())
        }
        (Method::Get | Method::Head, _) if !path.starts_with("/api/") => static_file(app, rq, path),
        _ => Reply::err(404, "not found"),
    }
}

/// "1.30.1.2 #123" -> comparable numbers.
fn version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

fn new_battle(app: &App, rq: &mut Request) -> Reply {
    let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
    let body = match read_body(rq) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Ok(nb) = serde_json::from_slice::<NewBattle>(&body) else { return Reply::err(400, "bad json") };
    if nb.player.is_empty() || nb.player.len() > 40 || nb.map.len() > 80 {
        return Reply::err(400, "bad battle");
    }
    let mut st = app.state.lock().unwrap();
    // The first key that uploads for a player name owns it.
    let claim = format!("{}:{}", nb.realm, nb.player);
    match st.claims.get(&claim) {
        Some(k) if *k != owner => return Reply::err(403, "this player name belongs to another uploader"),
        Some(_) => {}
        None if st.claims.values().filter(|k| **k == owner).count() >= CLAIMS_PER_KEY => {
            return Reply::err(429, "too many player names for one uploader")
        }
        None => {
            st.claims.insert(claim, owner.clone());
            write_json(&app.data.join("claims.json"), &st.claims);
        }
    }
    // A finished replay uploaded again: same battle entry.
    if !nb.live && !nb.file_name.is_empty() {
        if let Some(s) = st.battles.values().find(|s| s.owner == owner && s.info.file_name == nb.file_name && s.info.player == nb.player) {
            return Reply::json(&s.info);
        }
        // The finished replay of this uploader's live upload (live entries have no file name; their
        // date is when the game started writing, a few minutes before the replay's own): that entry.
        let at = date_secs(&nb.date);
        let live = st
            .battles
            .values()
            .filter(|s| s.owner == owner && s.info.file_name.is_empty() && s.info.player == nb.player && s.info.map == nb.map)
            .filter_map(|s| Some((s.info.id.clone(), (date_secs(&s.info.date)? - at?).abs())))
            .filter(|(_, d)| *d <= 600)
            .min_by_key(|(_, d)| *d)
            .map(|(id, _)| id);
        if let Some(id) = live {
            let s = st.battles.get_mut(&id).unwrap();
            s.info.file_name = nb.file_name.clone();
            let s = s.clone();
            app.save_battle(&s);
            return Reply::json(&s.info);
        }
    }
    let t = now();
    let today = st.battles.values().filter(|s| s.owner == owner && t.saturating_sub(s.created) < 86400).count();
    if today >= env_num("MAX_BATTLES_PER_DAY", BATTLES_PER_DAY as u64) as usize {
        return Reply::err(429, "upload limit reached for today");
    }
    let id = loop {
        let id = token(10).to_ascii_lowercase();
        if !st.battles.contains_key(&id) {
            break id;
        }
    };
    let info = BattleInfo {
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
    };
    let mut info = info;
    // Another player's upload of the same battle: same realm, map and day, shared tanks.
    let day = |d: &str| d.get(..10).unwrap_or("").to_string();
    let mate = st
        .battles
        .values()
        .filter(|s| s.info.realm == info.realm && s.info.map == info.map && day(&s.info.date) == day(&info.date) && s.info.player != info.player)
        .find(|s| s.vehicle_ids.iter().filter(|v| nb.vehicle_ids.contains(v)).count() >= 3)
        .map(|s| s.info.id.clone());
    if let Some(mate) = mate {
        let m = st.battles.get_mut(&mate).unwrap();
        if m.info.group.is_empty() {
            m.info.group = mate.clone();
            let m = m.clone();
            app.save_battle(&m);
        }
        info.group = st.battles[&mate].info.group.clone();
    }
    let mut ids = nb.vehicle_ids;
    ids.truncate(200);
    let mut players: Vec<String> = nb.players.iter().map(|p| p.to_lowercase()).collect();
    players.truncate(200);
    let s = Stored { info: info.clone(), owner, created: t, vehicle_ids: ids, players };
    app.save_battle(&s);
    st.battles.insert(id, s);
    Reply::json(&info)
}

fn frames(app: &App, rq: &mut Request, id: &str, q: &str) -> Reply {
    let Some(owner) = key_hash(rq) else { return Reply::err(401, "key required") };
    let body = match read_body(rq) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if !whole_frames(&body) {
        return Reply::err(400, "bad frames");
    }
    let reset = query(q, "reset").is_some();
    let mut st = app.state.lock().unwrap();
    // Storage limits: per uploader and for the whole server.
    let total: u64 = st.battles.values().map(|s| s.info.size + s.info.replay_size).sum();
    let mine: u64 = st.battles.values().filter(|s| s.owner == owner).map(|s| s.info.size + s.info.replay_size).sum();
    if mine + body.len() as u64 > env_num("MAX_KEY_MB", KEY_MB) << 20 {
        return Reply::err(507, "storage limit for this uploader reached");
    }
    if total + body.len() as u64 > env_num("MAX_TOTAL_GB", TOTAL_GB) << 30 {
        return Reply::err(507, "server storage full");
    }
    let Some(s) = st.battles.get_mut(id) else { return Reply::err(404, "not found") };
    if s.owner != owner {
        return Reply::err(403, "not yours");
    }
    // Finished battles are immutable (they are cached forever) unless replaced as a whole.
    if !s.info.live && s.info.size > 0 && !reset {
        return Reply::err(409, "battle is finished");
    }
    if (if reset { 0 } else { s.info.size }) + body.len() as u64 > MAX_BATTLE {
        return Reply::err(413, "battle too large");
    }
    let path = app.data.join("battles").join(format!("{id}.bin"));
    let ok = if reset {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &body).and_then(|_| std::fs::rename(&tmp, &path)).is_ok()
    } else {
        std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(&body)).is_ok()
    };
    if !ok {
        return Reply::err(500, "write failed");
    }
    if reset {
        s.info.size = 0;
        s.info.gen += 1;
    }
    s.info.size += body.len() as u64;
    s.info.updated = now();
    if query(q, "finish").is_some() {
        s.info.live = false;
    }
    if let Some(d) = query(q, "damage").and_then(|d| d.parse().ok()) {
        s.info.damage = d;
    }
    let s = s.clone();
    app.save_battle(&s);
    Reply::json(&s.info)
}

fn data(app: &App, rq: &mut Request, id: &str, q: &str) -> Reply {
    let (info, allowed) = {
        let st = app.state.lock().unwrap();
        let Some(s) = st.battles.get(id) else { return Reply::err(404, "not found") };
        let live = is_live(s, now());
        (s.info.clone(), !live || unlocked(&st, rq).0.contains(&s.owner))
    };
    if !allowed {
        return Reply::err(403, "live battle: needs a viewing code from the player");
    }
    let gen_ok = query(q, "gen").and_then(|g| g.parse::<u32>().ok()) == Some(info.gen);
    let from = if gen_ok { query(q, "from").and_then(|f| f.parse::<u64>().ok()).unwrap_or(0).min(info.size) } else { 0 };
    let mut f = match std::fs::File::open(app.data.join("battles").join(format!("{id}.bin"))) {
        Ok(f) => f,
        Err(_) => return Reply::bin(vec![]).header(GEN_HEADER, info.gen.to_string()),
    };
    use std::io::Seek;
    let mut buf = Vec::with_capacity((info.size - from) as usize);
    if f.seek(std::io::SeekFrom::Start(from)).is_err() || f.take(info.size - from).read_to_end(&mut buf).is_err() {
        return Reply::err(500, "read failed");
    }
    let finished = !info.live;
    Reply::bin(buf)
        .header(GEN_HEADER, info.gen.to_string())
        .header("X-Livereplay-Finished", if finished { "1" } else { "0" })
        .header("Cache-Control", if finished && from == 0 { "public, max-age=31536000, immutable" } else { "no-store" })
}

fn list(app: &App, q: &str) -> Reply {
    let player = query(q, "player").unwrap_or_default().to_lowercase();
    let group = query(q, "group").unwrap_or_default();
    let page = query(q, "page").and_then(|p| p.parse().ok()).unwrap_or(0usize);
    let st = app.state.lock().unwrap();
    let t = now();
    let mut v: Vec<&BattleInfo> = st
        .battles
        .values()
        .filter(|s| !is_live(s, t) && s.info.size > 0)
        .map(|s| &s.info)
        .filter(|i| player.is_empty() || i.player.to_lowercase().contains(&player))
        .filter(|i| group.is_empty() || i.group == group)
        .collect();
    let key = |d: &str| {
        let (day, time) = d.split_once(' ').unwrap_or((d, ""));
        match day.split('.').collect::<Vec<_>>()[..] {
            [d, m, y] => format!("{y}{m:0>2}{d:0>2}{time}"),
            _ => d.to_string(),
        }
    };
    v.sort_by(|a, b| key(&b.date).cmp(&key(&a.date)));
    let total = v.len();
    let battles = v.into_iter().skip(page * PER_PAGE).take(PER_PAGE).map(|i| {
        let mut i = i.clone();
        i.live = false;
        i
    });
    Reply::json(&BattleList { battles: battles.collect(), total, page, per_page: PER_PAGE })
}

fn static_file(app: &App, rq: &Request, path: &str) -> Reply {
    // Unknown paths get the viewer page (it routes by URL itself).
    let f = app.web.get(path).or_else(|| app.web.get("/index.html"));
    let Some(f) = f else { return Reply::err(404, "web viewer not installed") };
    if header(rq, "If-None-Match") == Some(f.etag.as_str()) {
        return Reply { status: 304, body: vec![], ctype: f.ctype, headers: vec![("ETag".into(), f.etag.clone())], compress: false };
    }
    let accept = header(rq, "Accept-Encoding").unwrap_or("");
    let mut r = Reply { status: 200, body: vec![], ctype: f.ctype, headers: vec![("ETag".into(), f.etag.clone()), ("Cache-Control".into(), "no-cache".into())], compress: false };
    match (&f.br, &f.gz) {
        (Some(b), _) if accept.contains("br") => {
            r.body = b.clone();
            r.headers.push(("Content-Encoding".into(), "br".into()));
        }
        (_, Some(z)) if accept.contains("gzip") => {
            r.body = z.clone();
            r.headers.push(("Content-Encoding".into(), "gzip".into()));
        }
        _ => r.body = f.body.clone(),
    }
    r.headers.push(("Vary".into(), "Accept-Encoding".into()));
    r
}

fn respond(rq: Request, r: Reply) {
    let gz_ok = r.compress && r.body.len() > 512 && header(&rq, "Accept-Encoding").is_some_and(|a| a.contains("gzip"));
    let body = if gz_ok { gzip(&r.body) } else { r.body };
    let mut resp = Response::from_data(body).with_status_code(r.status);
    let mut add = |k: &str, v: &str| {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    };
    add("Content-Type", r.ctype);
    add("X-Content-Type-Options", "nosniff");
    if gz_ok {
        add("Content-Encoding", "gzip");
        add("Vary", "Accept-Encoding");
    }
    for (k, v) in &r.headers {
        add(k, v);
    }
    let _ = rq.respond(resp);
}

fn main() {
    let bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let data = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "data".into()));
    let web = PathBuf::from(std::env::var("WEB_DIR").unwrap_or_else(|_| "web-dist".into()));
    let threads: usize = std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(4);
    let app = Arc::new(App::load(data, &web));
    let server = Arc::new(Server::http(&bind).expect("bind"));
    println!("listening on http://{bind}");
    let workers: Vec<_> = (0..threads.max(1))
        .map(|_| {
            let (app, server) = (app.clone(), server.clone());
            std::thread::spawn(move || {
                for mut rq in server.incoming_requests() {
                    let r = route(&app, &mut rq);
                    respond(rq, r);
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

    #[test]
    fn helpers() {
        assert!(whole_frames(&[2, 0, 0, 0, 9, 9, 1, 0, 0, 0, 5]));
        assert!(!whole_frames(&[2, 0, 0, 0, 9]));
        assert_eq!(url_decode("a%20b+c"), "a b c");
        assert_eq!(date_secs("01.01.1970 00:00:00"), Some(0));
        assert_eq!(date_secs("21.11.2026 01:43:04").zip(date_secs("21.11.2026 01:41:26")).map(|(a, b)| a - b), Some(98));
        assert_eq!(date_secs("01.03.2024 00:00:00").zip(date_secs("29.02.2024 23:59:59")).map(|(a, b)| a - b), Some(1));
        assert_eq!(date_secs("21.11.2026"), None);
        assert!(version_key("1.31.0.2") > version_key("1.30.1"));
        assert_eq!(token(10).len(), 10);
    }
}
