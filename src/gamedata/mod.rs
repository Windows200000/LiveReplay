//! Read-only access to the installed game client: packages, packed XML, localization,
//! map (arena) definitions and vehicle metadata. Nothing from the game is redistributed;
//! everything is loaded at runtime from the user's own installation.

pub mod cache3d;
pub mod dds;
pub mod ground;
pub mod models;
pub mod pkg;
pub mod pxml;
pub mod space;
pub mod webpack;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pkg::Pkg;
use pxml::Node;

pub struct GameData {
    pub root: PathBuf,
    pkgs: Mutex<HashMap<String, Option<std::sync::Arc<Pkg>>>>,
    mo: Mutex<HashMap<String, std::sync::Arc<HashMap<String, String>>>>,
    vehicles: Mutex<HashMap<String, VehicleInfo>>,
    vehicle_ids: Mutex<HashMap<(String, u16), String>>,
    nations_loaded: Mutex<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VehicleClass {
    Light,
    Medium,
    Heavy,
    TankDestroyer,
    Artillery,
    Unknown,
}

impl VehicleClass {
    pub fn from_tags(tags: &str) -> Self {
        for t in tags.split_whitespace() {
            match t {
                "lightTank" => return Self::Light,
                "mediumTank" => return Self::Medium,
                "heavyTank" => return Self::Heavy,
                "AT-SPG" => return Self::TankDestroyer,
                "SPG" => return Self::Artillery,
                _ => {}
            }
        }
        Self::Unknown
    }
    pub fn short(&self) -> &'static str {
        match self {
            Self::Light => "LT",
            Self::Medium => "MT",
            Self::Heavy => "HT",
            Self::TankDestroyer => "TD",
            Self::Artillery => "SPG",
            Self::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VehicleInfo {
    pub tag: String,
    pub class: VehicleClass,
    pub tier: u8,
    pub name: String,
    pub short_name: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GameplayDef {
    pub bases: [Vec<[f32; 2]>; 2],
    pub spawns: [Vec<[f32; 2]>; 2],
    pub control_points: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ArenaDef {
    pub map: String,
    pub display_name: String,
    /// (min x, min z), (max x, max z) in world coordinates.
    pub bbox_min: [f32; 2],
    pub bbox_max: [f32; 2],
    pub round_length: f32,
    pub minimap_path: String,
    /// Random-event minimap overlays (e.g. `mmap_bigCityBombing_destroyed.dds`).
    #[serde(default)]
    pub minimap_layers: Vec<String>,
    pub gameplay: HashMap<String, GameplayDef>,
}

impl ArenaDef {
    pub fn gameplay(&self, id: &str) -> GameplayDef {
        self.gameplay
            .get(id)
            .or_else(|| self.gameplay.get("ctf"))
            .cloned()
            .unwrap_or_default()
    }
}

/// Try to locate the World of Tanks install (Wargaming Game Center or Steam).
pub fn find_game_dir(exe_hint: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !exe_hint.is_empty() {
        let p = PathBuf::from(exe_hint);
        candidates.push(if p.is_file() { p.parent().map(Path::to_path_buf).unwrap_or(p) } else { p });
    }
    if let Ok(xml) = std::fs::read_to_string(r"C:\ProgramData\Wargaming.net\GameCenter\preferences.xml") {
        for part in xml.split("<working_dir>").skip(1) {
            if let Some(end) = part.find("</working_dir>") {
                candidates.push(PathBuf::from(part[..end].trim()));
            }
        }
    }
    for drive in ["C", "D", "E", "F"] {
        for p in [r"Games\World_of_Tanks_EU", r"Games\World_of_Tanks_NA", r"Games\World_of_Tanks_ASIA", r"Games\World_of_Tanks"] {
            candidates.push(PathBuf::from(format!(r"{drive}:\{p}")));
        }
    }
    candidates.into_iter().find(|p| p.join("res").join("packages").join("scripts.pkg").is_file())
}

fn parse_mo(bytes: &[u8]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let rd = |o: usize| -> Option<usize> { Some(u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?) as usize) };
    let (Some(n), Some(ko), Some(vo)) = (rd(8), rd(12), rd(16)) else { return out };
    for i in 0..n {
        let (Some(kl), Some(kp), Some(vl), Some(vp)) = (rd(ko + 8 * i), rd(ko + 8 * i + 4), rd(vo + 8 * i), rd(vo + 8 * i + 4)) else {
            break;
        };
        if let (Some(k), Some(v)) = (bytes.get(kp..kp + kl), bytes.get(vp..vp + vl)) {
            out.insert(String::from_utf8_lossy(k).into_owned(), String::from_utf8_lossy(v).into_owned());
        }
    }
    out
}

impl GameData {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            pkgs: Mutex::new(HashMap::new()),
            mo: Mutex::new(HashMap::new()),
            vehicles: Mutex::new(HashMap::new()),
            vehicle_ids: Mutex::new(HashMap::new()),
            nations_loaded: Mutex::new(Vec::new()),
        }
    }

    fn pkg(&self, name: &str) -> Option<std::sync::Arc<Pkg>> {
        let mut cache = self.pkgs.lock().unwrap();
        cache
            .entry(name.to_string())
            .or_insert_with(|| Pkg::open(&self.root.join("res").join("packages").join(name)).ok().map(std::sync::Arc::new))
            .clone()
    }

    /// Read a file from a named package, falling back to `res_mods`-style loose files under `res/`.
    pub fn read(&self, pkg: &str, path: &str) -> Option<Vec<u8>> {
        if let Some(p) = self.pkg(pkg) {
            if let Some(b) = p.read(path) {
                return Some(b);
            }
        }
        std::fs::read(self.root.join("res").join(path)).ok()
    }

    pub fn read_xml(&self, pkg: &str, path: &str) -> Option<Node> {
        pxml::parse(&self.read(pkg, path)?)
    }

    pub fn script_xml(&self, path: &str) -> Option<Node> {
        self.read_xml("scripts.pkg", path)
    }

    /// Localized string from `res/text/lc_messages/<domain>.mo`, e.g. `#arenas:01_karelia/name`.
    pub fn tr(&self, key: &str) -> Option<String> {
        let key = key.trim().trim_start_matches('#');
        let (domain, id) = key.split_once(':')?;
        let table = {
            let mut cache = self.mo.lock().unwrap();
            cache
                .entry(domain.to_string())
                .or_insert_with(|| {
                    let p = self.root.join("res").join("text").join("lc_messages").join(format!("{domain}.mo"));
                    std::sync::Arc::new(std::fs::read(p).map(|b| parse_mo(&b)).unwrap_or_default())
                })
                .clone()
        };
        table.get(id).cloned()
    }

    /// Realm code from `version.xml` (EU, NA, ASIA, ...).
    pub fn realm(&self) -> String {
        std::fs::read_to_string(self.root.join("version.xml"))
            .ok()
            .and_then(|x| x.split("<realm>").nth(1).and_then(|r| r.split("</realm>").next()).map(|r| r.trim().to_string()))
            .unwrap_or_else(|| "EU".into())
    }

    pub fn arena(&self, map: &str) -> Option<ArenaDef> {
        let root = self.script_xml(&format!("scripts/arena_defs/{map}.xml"))?;
        let bb = root.child("boundingBox")?;
        let bl = bb.child("bottomLeft")?.floats();
        let ur = bb.child("upperRight")?.floats();
        let mut gameplay = HashMap::new();
        if let Some(g) = root.child("gameplayTypes") {
            for gt in &g.children {
                let mut def = GameplayDef::default();
                let collect = |n: Option<&Node>, out: &mut [Vec<[f32; 2]>; 2]| {
                    let Some(n) = n else { return };
                    for (i, team) in ["team1", "team2"].iter().enumerate() {
                        if let Some(t) = n.child(team) {
                            for p in &t.children {
                                let f = p.floats();
                                if f.len() >= 2 {
                                    out[i].push([f[0], f[1]]);
                                }
                            }
                        }
                    }
                };
                collect(gt.child("teamBasePositions"), &mut def.bases);
                collect(gt.child("teamSpawnPoints"), &mut def.spawns);
                for cp in gt.children_named("controlPoint") {
                    let f = cp.floats();
                    if f.len() >= 2 {
                        def.control_points.push([f[0], f[1]]);
                    }
                }
                gameplay.insert(gt.name.clone(), def);
            }
        }
        let display_name = root.child_text("name").and_then(|k| self.tr(k)).unwrap_or_else(|| map.to_string());
        Some(ArenaDef {
            map: map.to_string(),
            display_name,
            bbox_min: [bl[0], bl[1]],
            bbox_max: [ur[0], ur[1]],
            round_length: root.child_text("roundLength").and_then(|s| s.parse().ok()).unwrap_or(900.0),
            minimap_path: root.child_text("minimap").unwrap_or("").to_string(),
            minimap_layers: root
                .child("minimapLayers")
                .map(|l| l.children_named("layer").filter_map(|x| x.child_text("path").map(str::to_string)).collect())
                .unwrap_or_default(),
            gameplay,
        })
    }

    /// A file from the GUI packages (`gui-part1.pkg`, `gui-part2.pkg`, ...).
    pub fn gui_file(&self, path: &str) -> Option<Vec<u8>> {
        (1..=8).find_map(|i| self.pkg(&format!("gui-part{i}.pkg")).and_then(|p| p.read(path)))
            .or_else(|| self.read("gui.pkg", path))
    }

    /// A GUI icon from the client, cropped to its visible pixels (icons sit in large canvases).
    pub fn gui_icon(&self, path: &str) -> Option<dds::Image> {
        let img = models::png_image(&self.gui_file(path)?)?;
        let (w, h) = (img.width, img.height);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if img.rgba[(y * w + x) * 4 + 3] > 24 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return Some(img);
        }
        // Square crop around the glyph with a little padding.
        let side = (x1 - x0).max(y1 - y0) + 4;
        let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
        let mut out = vec![0u8; side * side * 4];
        for y in 0..side {
            for x in 0..side {
                let (sx, sy) = (cx as i64 - side as i64 / 2 + x as i64, cy as i64 - side as i64 / 2 + y as i64);
                if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                    let s = (sy as usize * w + sx as usize) * 4;
                    out[(y * side + x) * 4..][..4].copy_from_slice(&img.rgba[s..s + 4]);
                }
            }
        }
        Some(dds::Image { width: side, height: side, rgba: out })
    }

    /// Minimap texture for a map, decoded to RGBA.
    pub fn minimap(&self, arena: &ArenaDef) -> Option<dds::Image> {
        self.minimap_raw(arena).and_then(|b| dds::decode(&b))
    }

    /// The minimap texture file as stored in the game (DDS).
    pub fn minimap_raw(&self, arena: &ArenaDef) -> Option<Vec<u8>> {
        let path = if arena.minimap_path.is_empty() { format!("spaces/{}/mmap.dds", arena.map) } else { arena.minimap_path.clone() };
        // Map packages are named after the space; some variants (e.g. `_sm24`) reuse a base package.
        let space = path.split('/').nth(1).unwrap_or(&arena.map).to_string();
        let candidates = [format!("{space}.pkg"), format!("{}.pkg", arena.map)];
        candidates.iter().find_map(|p| self.read(p, &path))
    }

    /// Random-event minimap overlays (danger zones and aftermath) with their file paths.
    pub fn minimap_event_layers(&self, arena: &ArenaDef) -> Vec<(String, dds::Image)> {
        self.minimap_event_layers_raw(arena).into_iter().filter_map(|(p, b)| Some((p, dds::decode(&b)?))).collect()
    }

    /// The overlay files as stored in the game (DDS).
    pub fn minimap_event_layers_raw(&self, arena: &ArenaDef) -> Vec<(String, Vec<u8>)> {
        let pkg = format!("{}.pkg", arena.map);
        arena.minimap_layers.iter().filter_map(|p| Some((p.clone(), self.read(&pkg, p)?))).collect()
    }

    /// Stock-config hit points of a vehicle type (`nation:tag`): hull + strongest turret.
    pub fn vehicle_max_health(&self, vehicle_type: &str) -> Option<i32> {
        let (nation, tag) = vehicle_type.split_once(':')?;
        let root = self.script_xml(&format!("scripts/item_defs/vehicles/{nation}/{tag}.xml"))?;
        let num = |n: &Node| n.child_text("maxHealth").and_then(|v| v.trim().parse::<f32>().ok());
        let hull = root.child("hull").and_then(num)?;
        let turret = root.child("turrets0").map(|t| t.children.iter().filter_map(num).fold(0.0, f32::max)).unwrap_or(0.0);
        Some((hull + turret) as i32)
    }

    /// Vehicle metadata for a replay vehicle type such as `ussr:R04_T-34`.
    /// Event variants like `ussr:R228_KR_1_7x7` fall back to the base vehicle.
    pub fn vehicle(&self, vehicle_type: &str) -> Option<VehicleInfo> {
        let (nation, tag) = vehicle_type.split_once(':')?;
        self.load_nation(nation);
        let map = self.vehicles.lock().unwrap();
        let mut t = tag;
        loop {
            if let Some(v) = map.get(t) {
                return Some(v.clone());
            }
            t = &t[..t.rfind('_')?];
        }
    }

    pub fn vehicle_tag_by_id(&self, nation: &str, id: u16) -> Option<String> {
        self.load_nation(nation);
        self.vehicle_ids.lock().unwrap().get(&(nation.to_string(), id)).cloned()
    }

    fn load_nation(&self, nation: &str) {
        let first_load = {
            let mut loaded = self.nations_loaded.lock().unwrap();
            if loaded.iter().any(|n| n == nation) {
                false
            } else {
                loaded.push(nation.to_string());
                true
            }
        };
        if first_load {
            if let Some(list) = self.script_xml(&format!("scripts/item_defs/vehicles/{nation}/list.xml")) {
                let mut map = self.vehicles.lock().unwrap();
                let mut ids = self.vehicle_ids.lock().unwrap();
                for v in &list.children {
                    if let Some(id) = v.child_text("id").and_then(|s| s.parse().ok()) {
                        ids.insert((nation.to_string(), id), v.name.clone());
                    }
                    let key = v.child_text("userString").unwrap_or("");
                    let name = self.tr(key).unwrap_or_else(|| v.name.clone());
                    let short_name = self.tr(&format!("{key}_short")).filter(|s| !s.is_empty()).unwrap_or_else(|| name.clone());
                    map.insert(
                        v.name.clone(),
                        VehicleInfo {
                            tag: v.name.clone(),
                            class: VehicleClass::from_tags(v.child_text("tags").unwrap_or("")),
                            tier: v.child_text("level").and_then(|s| s.parse().ok()).unwrap_or(0),
                            name,
                            short_name,
                        },
                    );
                }
            }
        }
    }
}
