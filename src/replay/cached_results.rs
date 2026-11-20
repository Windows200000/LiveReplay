//! Battle results for replays that were left before the battle ended (they have no results block).
//!
//! The game keeps the results of recent battles in `%APPDATA%\Wargaming.net\WorldOfTanks\battle_results`
//! as pickled positional lists: no field names, only a layout checksum as the first entry. We
//! 1. archive those files (the game deletes old ones),
//! 2. learn the field names per checksum by pairing files with replays that do have a results block
//!    for the same battle (same values in the same places), and
//! 3. rebuild a results block (`common`, `players`, `vehicles`) for replays without one.
//! Nothing is hard-coded, so new game versions are learned the same way.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::pickle::{self, Py};

/// The game's results cache folders (one per account).
fn game_dirs() -> Vec<PathBuf> {
    let Some(appdata) = std::env::var_os("APPDATA") else { return Vec::new() };
    let root = PathBuf::from(appdata).join("Wargaming.net").join("WorldOfTanks").join("battle_results");
    std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
}

fn archive_dir() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("battle_results"))
}

fn fields_path() -> Option<PathBuf> {
    crate::dirs::config_dir().map(|d| d.join("results-fields.json"))
}

/// Copy result files the archive doesn't have yet (named `<arenaUniqueID>.dat`); returns how many.
pub fn archive_new() -> usize {
    let mut n = 0;
    let Some(dst) = archive_dir() else { return 0 };
    let _ = std::fs::create_dir_all(&dst);
    for dir in game_dirs() {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "dat") {
                if let Some(name) = p.file_name() {
                    let to = dst.join(name);
                    if !to.exists() && std::fs::copy(&p, &to).is_ok() {
                        n += 1;
                    }
                }
            }
        }
    }
    n
}

/// Learn from the replays in `dir` that may belong to archived results not learned from yet
/// (replays written around the same time).
pub fn learn_from_folder(dir: &Path) {
    let Some(archive) = archive_dir() else { return };
    let l = load_learned();
    let pending: Vec<std::time::SystemTime> = std::fs::read_dir(&archive)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<i64>().ok()).is_some_and(|a| !l.used.contains(&a)))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .collect();
    let Some(oldest) = pending.iter().min().copied() else { return };
    let since = oldest - std::time::Duration::from_secs(6 * 3600);
    let results: Vec<Value> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "wotreplay"))
        .filter(|e| e.metadata().ok().and_then(|m| m.modified().ok()).is_some_and(|m| m >= since))
        .filter_map(|e| super::file::ReplayFile::open(&e.path()).ok()?.results()?.get(0).cloned())
        .collect();
    learn(&results);
}

/// One cached results file, split into its positional sections.
struct Dat {
    arena: i64,
    common: Vec<Py>,
    /// account id -> list
    players: Vec<(i64, Vec<Py>)>,
    /// vehicle id -> list
    vehicles: Vec<(i64, Vec<Py>)>,
}

fn inflate(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(b).read_to_end(&mut out).ok()?;
    Some(out)
}

fn list(p: &Py) -> Vec<Py> {
    match p {
        Py::List(l) => l.clone(),
        _ => Vec::new(),
    }
}

fn parse_dat(bytes: &[u8]) -> Option<Dat> {
    // (version, (arenaUniqueID, zlib(personal common), zlib(personal), zlib((common, players, vehicles, avatars))))
    let outer = pickle::loads(bytes)?;
    let inner = outer.items().get(1)?;
    let arena = inner.items().first()?.int();
    let raw = match inner.items().get(3)? {
        Py::Bytes(b) => b.clone(),
        Py::Str(s) => s.as_bytes().to_vec(),
        _ => return None,
    };
    let main = pickle::loads(&inflate(&raw)?)?;
    let parts = main.items();
    let common = list(parts.first()?);
    let dict = |p: &Py| match p {
        Py::Dict(d) => d.clone(),
        _ => Vec::new(),
    };
    let players = dict(parts.get(1)?).into_iter().map(|(k, v)| (k.int(), list(&v))).collect();
    let vehicles = dict(parts.get(2)?)
        .into_iter()
        .filter_map(|(k, v)| {
            // vehicle id -> {typeCompDescr: list}
            let first = dict(&v).into_iter().next()?.1;
            Some((k.int(), list(&first)))
        })
        .collect();
    Some(Dat { arena, common, players, vehicles })
}

fn read_dat(p: &Path) -> Option<Dat> {
    parse_dat(&std::fs::read(p).ok()?)
}

/// checksum -> field name per position (None = not known).
type Fields = HashMap<String, Vec<Option<String>>>;

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Learned {
    fields: Fields,
    /// layout -> field -> value an empty (None) entry stands for (e.g. deathReason -1 = alive).
    defaults: HashMap<String, HashMap<String, Value>>,
    /// Battles already used for learning.
    used: HashSet<i64>,
}

fn load_learned() -> Learned {
    fields_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_learned(l: &Learned) {
    if let (Some(p), Ok(b)) = (fields_path(), serde_json::to_vec(l)) {
        let _ = std::fs::write(p, b);
    }
}

fn is_default(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// Could the (non-empty) pickled value `p` be the JSON value `v`?
/// Returns Some(informative) or None for a mismatch.
fn matches(p: &Py, v: &Value) -> Option<bool> {
    match p {
        Py::None => is_default(v).then_some(false),
        Py::Bool(b) => (v.as_bool() == Some(*b) || v.as_i64() == Some(*b as i64)).then_some(true),
        Py::Int(i) => (v.as_i64() == Some(*i) || v.as_bool().map(|b| b as i64) == Some(*i) || v.as_f64() == Some(*i as f64)).then_some(*i != 0),
        Py::Float(f) => v.as_f64().filter(|x| (x - f).abs() < 1e-6 * f.abs().max(1.0)).map(|_| *f != 0.0),
        Py::Str(s) => (v.as_str() == Some(s.as_str())).then_some(!s.is_empty()),
        Py::List(l) => (v.as_array().map(|a| a.len()) == Some(l.len()) || v.as_object().map(|o| o.len()) == Some(l.len())).then_some(false),
        Py::Dict(d) => (v.as_object().map(|o| o.len()) == Some(d.len())).then_some(false),
        Py::Opaque | Py::Bytes(_) => Some(false),
    }
}

/// Name each position from (list, object) samples of one layout.
fn learn_layout(samples: &[(&[Py], &Map<String, Value>)], known: &mut Vec<Option<String>>, defaults: &mut HashMap<String, Value>) {
    let Some(len) = samples.iter().map(|s| s.0.len()).max() else { return };
    known.resize(len, None);
    let keys: Vec<&String> = samples[0].1.keys().collect();
    let mut cands: Vec<Vec<&String>> = vec![Vec::new(); len];
    for i in 1..len {
        if known[i].is_some() {
            continue;
        }
        cands[i] = keys
            .iter()
            .copied()
            .filter(|k| {
                let mut informative = false;
                // An empty entry is the field's default: whatever value, but always the same one.
                let mut default: Option<&Value> = None;
                for (l, o) in samples {
                    match (l.get(i), o.get(k.as_str())) {
                        (Some(Py::None), Some(v)) => match default {
                            Some(d) if d != v => return false,
                            _ => default = Some(v),
                        },
                        (Some(p), Some(v)) => match matches(p, v) {
                            Some(inf) => informative |= inf || default.is_some_and(|d| d != v),
                            None => return false,
                        },
                        (Some(Py::None), None) | (None, _) => {}
                        _ => return false,
                    }
                }
                informative
            })
            .collect();
    }
    let taken: HashSet<String> = known.iter().flatten().cloned().collect();
    for c in cands.iter_mut() {
        c.retain(|k| !taken.contains(*k));
    }
    // Positions with exactly one candidate fix that name; repeat until nothing changes.
    loop {
        let mut changed = false;
        for i in 0..len {
            if cands[i].len() == 1 {
                let k = cands[i][0].clone();
                // Remember what an empty entry means for this field.
                if let Some(v) = samples.iter().find(|(l, _)| matches!(l.get(i), Some(Py::None))).and_then(|(_, o)| o.get(k.as_str())) {
                    defaults.insert(k.clone(), v.clone());
                }
                known[i] = Some(k.clone());
                cands[i].clear();
                for c in cands.iter_mut() {
                    c.retain(|x| **x != k);
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn checksum(l: &[Py]) -> String {
    l.first().map(|c| c.int()).unwrap_or(0).to_string()
}

/// Learn field names from replays that have a results block and an archived results file.
/// `results` are the replays' results blocks (first element of block 2).
pub fn learn(results: &[Value]) {
    let Some(dir) = archive_dir() else { return };
    let mut l = load_learned();
    let mut groups: BTreeMap<(&'static str, String), Vec<(Vec<Py>, Map<String, Value>)>> = BTreeMap::new();
    let mut used = Vec::new();
    for r in results {
        let Some(arena) = r.get("arenaUniqueID").and_then(|a| a.as_i64()) else { continue };
        if l.used.contains(&arena) {
            continue;
        }
        let Some(d) = read_dat(&dir.join(format!("{arena}.dat"))) else { continue };
        used.push(arena);
        if let Some(c) = r.get("common").and_then(|c| c.as_object()) {
            groups.entry(("common", checksum(&d.common))).or_default().push((d.common.clone(), c.clone()));
        }
        for (acc, pl) in &d.players {
            if let Some(o) = r.get("players").and_then(|p| p.get(acc.to_string())).and_then(|o| o.as_object()) {
                groups.entry(("players", checksum(pl))).or_default().push((pl.clone(), o.clone()));
            }
        }
        for (vid, vl) in &d.vehicles {
            if let Some(o) = r.get("vehicles").and_then(|v| v.get(vid.to_string())).and_then(|a| a.get(0)).and_then(|o| o.as_object()) {
                groups.entry(("vehicles", checksum(vl))).or_default().push((vl.clone(), o.clone()));
            }
        }
    }
    if used.is_empty() {
        return;
    }
    for ((section, sum), samples) in &groups {
        let key = format!("{section}:{sum}");
        let s: Vec<(&[Py], &Map<String, Value>)> = samples.iter().map(|(a, b)| (a.as_slice(), b)).collect();
        let known = l.fields.entry(key.clone()).or_default();
        learn_layout(&s, known, l.defaults.entry(key).or_default());
    }
    l.used.extend(used);
    save_learned(&l);
}

fn to_json(p: &Py) -> Value {
    match p {
        Py::None | Py::Opaque | Py::Bytes(_) => Value::Null,
        Py::Bool(b) => Value::Bool(*b),
        Py::Int(i) => Value::from(*i),
        Py::Float(f) => serde_json::Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
        Py::Str(s) => Value::String(s.clone()),
        Py::List(l) => Value::Array(l.iter().map(to_json).collect()),
        Py::Dict(d) => Value::Object(d.iter().map(|(k, v)| (match k { Py::Str(s) => s.clone(), k => k.int().to_string() }, to_json(v))).collect()),
    }
}

fn named(section: &str, l: &[Py], learned: &Learned) -> Option<Map<String, Value>> {
    let key = format!("{section}:{}", checksum(l));
    let names = learned.fields.get(&key)?;
    let defaults = learned.defaults.get(&key);
    Some(
        l.iter()
            .zip(names)
            .skip(1)
            .filter_map(|(p, n)| {
                let n = n.clone()?;
                let v = match p {
                    Py::None => defaults.and_then(|d| d.get(&n)).cloned().unwrap_or(Value::Null),
                    p => to_json(p),
                };
                Some((n, v))
            })
            .filter(|(_, v)| !v.is_null())
            .collect(),
    )
}

/// A results block (`{common, players, vehicles, arenaUniqueID}`) for a battle with these
/// vehicle ids, rebuilt from an archived results file, if one matches and its layout is known.
pub fn results_for(vehicle_ids: &[i32]) -> Option<Value> {
    if vehicle_ids.len() < 2 {
        return None;
    }
    archive_new();
    let l = load_learned();
    let dir = archive_dir()?;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let Some(d) = read_dat(&e.path()) else { continue };
        let shared = d.vehicles.iter().filter(|(v, _)| vehicle_ids.contains(&(*v as i32))).count();
        if shared * 2 < vehicle_ids.len().min(d.vehicles.len()) || shared < 2 {
            continue;
        }
        let vehicles: Map<String, Value> = d
            .vehicles
            .iter()
            .filter_map(|(vid, vl)| Some((vid.to_string(), Value::Array(vec![Value::Object(named("vehicles", vl, &l)?)]))))
            .collect();
        if vehicles.is_empty() {
            return None;
        }
        let players: Map<String, Value> = d.players.iter().filter_map(|(a, pl)| Some((a.to_string(), Value::Object(named("players", pl, &l)?)))).collect();
        let mut r = Map::new();
        r.insert("arenaUniqueID".into(), Value::from(d.arena));
        r.insert("common".into(), Value::Object(named("common", &d.common, &l).unwrap_or_default()));
        r.insert("players".into(), Value::Object(players));
        r.insert("vehicles".into(), Value::Object(vehicles));
        return Some(Value::Array(vec![Value::Object(r)]));
    }
    None
}
