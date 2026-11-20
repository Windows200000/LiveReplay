//! Simplified 3D data built from the client once and cached on disk, keyed by game version
//! (a game update changes the key, so everything is re-parsed automatically).
//!
//! * Terrain: chunk height maps, decimated to one sample every 6.25 m.
//! * Tanks: chassis / hull / top turret / top gun meshes, vertex-clustered to a few hundred
//!   triangles each, with the pivot points from the vehicle definition.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use super::models::{parse_chunk_heights, parse_primitives, parse_sections, Mesh};
use super::pkg::Pkg;
use super::GameData;

const FORMAT: u32 = 17;
/// Keep every 4th height sample: 65 samples / 100 m -> 17 samples (6.25 m spacing).
const TERRAIN_STEP: usize = 4;

#[derive(Debug, Clone, Default)]
pub struct Terrain {
    pub x0: f32,
    pub z0: f32,
    pub spacing: f32,
    pub w: usize,
    pub h: usize,
    /// Row-major, rows = +z.
    pub heights: Vec<f32>,
}

impl Terrain {
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        if self.w < 2 || self.h < 2 {
            return 0.0;
        }
        let fx = ((x - self.x0) / self.spacing).clamp(0.0, (self.w - 1) as f32 - 0.001);
        let fz = ((z - self.z0) / self.spacing).clamp(0.0, (self.h - 1) as f32 - 0.001);
        let (ix, iz) = (fx as usize, fz as usize);
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let g = |x: usize, z: usize| self.heights[z * self.w + x];
        let a = g(ix, iz) * (1.0 - tx) + g(ix + 1, iz) * tx;
        let b = g(ix, iz + 1) * (1.0 - tx) + g(ix + 1, iz + 1) * tx;
        a * (1.0 - tz) + b * tz
    }
}

/// Static map content around the terrain.
#[derive(Debug, Clone, Default)]
pub struct MapScene {
    /// Buildings, bunkers, rocks... merged into one simplified mesh in world coordinates.
    pub objects: Mesh,
    /// Water surfaces: (min x, min z, max x, max z, height).
    pub water: Vec<[f32; 5]>,
    /// In-game ground colour (`global_AM.dds`, cropped) and the world rectangle it covers
    /// (min x, min z, max x, max z).
    /// (DDS, DXT1 with mip levels)
    pub albedo: Option<Vec<u8>>,
    pub albedo_extent: [f32; 4],
    /// Object texture atlas (DDS, DXT1 with mips): `atlas_grid` x `atlas_grid` tiles of
    /// ATLAS_TILE px; `objects.tri_tex` = tile index (u16::MAX = untextured, vertex colour).
    pub atlas: Option<Vec<u8>>,
    pub atlas_grid: u32,
}

pub const ATLAS_TILE: usize = 256;
const ATLAS_MAX_GRID: usize = 32;

#[derive(Debug, Clone, Default)]
pub struct TankModel {
    /// Chassis and hull in vehicle space (hull already lifted to its position).
    pub body: Mesh,
    /// Turret mesh relative to its pivot; pivot in vehicle space.
    pub turret: Mesh,
    pub turret_pivot: [f32; 3],
    /// Gun mesh relative to its pivot; pivot relative to the turret pivot.
    pub gun: Mesh,
    pub gun_pivot: [f32; 3],
}

pub struct Cache3D {
    dir: PathBuf,
    vehicle_index: Mutex<Option<HashMap<String, String>>>,
}

// --- tiny binary format helpers ---
fn put_u32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}
fn put_f32(w: &mut Vec<u8>, v: f32) {
    w.extend_from_slice(&v.to_le_bytes());
}
/// Meshes are stored quantised and delta-coded (see `webpack::encode_mesh`): ~40% smaller.
fn put_mesh(w: &mut Vec<u8>, m: &Mesh) {
    let e = super::webpack::encode_mesh(m);
    put_u32(w, e.len() as u32);
    w.extend_from_slice(&e);
}
struct Rd<'a>(&'a [u8], usize);
impl Rd<'_> {
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_le_bytes(self.0.get(self.1..self.1 + 4)?.try_into().ok()?);
        self.1 += 4;
        Some(v)
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn v3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
    fn mesh(&mut self) -> Option<Mesh> {
        let n = self.u32()? as usize;
        let m = super::webpack::decode_mesh(self.0.get(self.1..self.1 + n)?)?;
        self.1 += n;
        Some(m)
    }
}

fn save(path: &PathBuf, data: &[u8]) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let compressed = {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        let _ = e.write_all(data);
        e.finish().unwrap_or_default()
    };
    let _ = std::fs::write(path, compressed);
}

/// Keep what matters for reading a battle: buildings, bunkers, bridges, big rocks/cliffs.
/// Fences, decor, small scatter and outland scenery are dropped to keep the scene light.
fn keep_object(path: &str, size: f32) -> bool {
    let category = path.split('/').nth(1).unwrap_or("");
    match category {
        "GatesAndFences" | "Decor" | "Outland" | "misc" | "Vegetation" | "Flora" | "Particles" | "content_deferred" => false,
        "Environment" | "Nature" => size >= 8.0 && size < 400.0,
        _ => size >= 3.0 && size < 400.0,
    }
}

/// Colour of a (small) texture at a UV, wrapping like the GPU would.
fn sample(tex: Option<&super::dds::Image>, uv: [f32; 2]) -> [u8; 3] {
    let Some(t) = tex else { return [150, 145, 135] };
    if t.width == 0 || t.height == 0 {
        return [150, 145, 135];
    }
    let x = (uv[0].rem_euclid(1.0) * t.width as f32) as usize % t.width;
    let y = (uv[1].rem_euclid(1.0) * t.height as f32) as usize % t.height;
    let o = (y * t.width + x) * 4;
    [t.rgba[o], t.rgba[o + 1], t.rgba[o + 2]]
}

const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

fn gd_read_any(c: &Cache3D, gd: &GameData, path: &str) -> Option<Vec<u8>> {
    c.find_file(gd, path)
}

fn xform(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

/// Prefab transform: position, euler rotation in degrees (yaw y, pitch x, roll z), scale.
fn prefab_matrix(t: &serde_json::Value) -> [f32; 16] {
    let v = |k: &str, d: f32| -> [f32; 3] {
        let o = &t[k];
        let g = |c: &str| o[c].as_f64().map(|x| x as f32).unwrap_or(d);
        [g("x"), g("y"), g("z")]
    };
    let pos = v("position", 0.0);
    let rot = v("rotation", 0.0);
    let sc = v("scale", 1.0);
    let (sy, cy) = rot[1].to_radians().sin_cos();
    let (sp, cp) = rot[0].to_radians().sin_cos();
    let (sr, cr) = rot[2].to_radians().sin_cos();
    let ry = [cy, 0.0, -sy, 0.0, 0.0, 1.0, 0.0, 0.0, sy, 0.0, cy, 0.0, 0.0, 0.0, 0.0, 1.0];
    let rx = [1.0, 0.0, 0.0, 0.0, 0.0, cp, sp, 0.0, 0.0, -sp, cp, 0.0, 0.0, 0.0, 0.0, 1.0];
    let rz = [cr, sr, 0.0, 0.0, -sr, cr, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let s = [sc[0], 0.0, 0.0, 0.0, 0.0, sc[1], 0.0, 0.0, 0.0, 0.0, sc[2], 0.0, 0.0, 0.0, 0.0, 1.0];
    let t = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, pos[0], pos[1], pos[2], 1.0];
    mat_mul(&t, &mat_mul(&ry, &mat_mul(&rx, &mat_mul(&rz, &s))))
}

/// Visit every `BW::DynamicModel` in a prefab tree with its accumulated transform.
fn walk_prefab(node: &serde_json::Value, parent: [f32; 16], f: &mut dyn FnMut(&str, &[f32; 16])) {
    match node {
        serde_json::Value::Object(o) => {
            let comps = o.get("components");
            let local = comps.and_then(|c| c.get("cgf::TransformComponent")).map(prefab_matrix).unwrap_or(IDENTITY);
            let m = mat_mul(&parent, &local);
            if let Some(model) = comps.and_then(|c| c.get("BW::DynamicModel")).and_then(|d| d.get("modelName")).and_then(|v| v.as_str()) {
                f(model, &m);
            }
            for (k, v) in o {
                if k == "components" {
                    continue;
                }
                walk_prefab(v, m, f);
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|v| walk_prefab(v, parent, f)),
        _ => {}
    }
}

/// Decode a cached single-mesh file (prefabs).
pub fn decode_mesh(raw: &[u8]) -> Option<Mesh> {
    let b = inflate(raw)?;
    Rd(&b, 0).mesh()
}

fn sub_len(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
}

/// The ground image is not included: it is stored once per map (`ground_path`), shared by all modes.
fn encode_scene(sc: &MapScene) -> Vec<u8> {
    let mut b = Vec::new();
    put_mesh(&mut b, &sc.objects);
    put_u32(&mut b, sc.water.len() as u32);
    sc.water.iter().flatten().for_each(|v| put_f32(&mut b, *v));
    sc.albedo_extent.iter().for_each(|v| put_f32(&mut b, *v));
    put_u32(&mut b, sc.atlas_grid);
    for tex in [&sc.atlas, &None] {
        let t = tex.as_deref().unwrap_or(&[]);
        put_u32(&mut b, t.len() as u32);
        b.extend_from_slice(t);
    }
    b
}

/// Decode a cached map scene file.
pub fn decode_scene(raw: &[u8]) -> Option<MapScene> {
    let b = inflate(raw)?;
    let mut r = Rd(&b, 0);
    let objects = r.mesh()?;
    let n = r.u32()? as usize;
    let water = (0..n).map(|_| Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?])).collect::<Option<Vec<_>>>()?;
    let albedo_extent = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let atlas_grid = r.u32()?;
    let mut tex = || -> Option<Option<Vec<u8>>> {
        let n = r.u32()? as usize;
        let t = (n > 0).then(|| b.get(r.1..r.1 + n).map(<[u8]>::to_vec)).flatten();
        r.1 += n;
        Some(t)
    };
    let atlas = tex()?;
    let albedo = tex()?;
    Some(MapScene { objects, water, albedo, albedo_extent, atlas, atlas_grid })
}

fn inflate(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(raw).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Decode a cached terrain file (as stored on disk / served by the server).
pub fn decode_terrain(raw: &[u8]) -> Option<Terrain> {
    let b = inflate(raw)?;
    let mut r = Rd(&b, 0);
    let (x0, z0, spacing) = (r.f32()?, r.f32()?, r.f32()?);
    let (w, h) = (r.u32()? as usize, r.u32()? as usize);
    let heights = (0..w * h).map(|_| r.f32()).collect::<Option<Vec<_>>>()?;
    Some(Terrain { x0, z0, spacing, w, h, heights })
}

/// Decode a cached tank model file.
pub fn decode_tank(raw: &[u8]) -> Option<TankModel> {
    let b = inflate(raw)?;
    let mut r = Rd(&b, 0);
    Some(TankModel { body: r.mesh()?, turret: r.mesh()?, turret_pivot: r.v3()?, gun: r.mesh()?, gun_pivot: r.v3()? })
}

impl Cache3D {
    /// Cache in the user's local app data, keyed by the installed game version.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(gd: &GameData) -> Option<Self> {
        let version = std::fs::read_to_string(gd.root.join("version.xml"))
            .ok()
            .and_then(|x| x.split("<version>").nth(1).and_then(|v| v.split("</version>").next()).map(|v| v.trim().to_string()))
            .unwrap_or_else(|| "unknown".into());
        let key: String = version.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' }).collect();
        let root = crate::dirs::cache_dir()?.join("3d");
        let dir = root.join(format!("{key}-f{FORMAT}"));
        // Caches of older game versions / formats are never used again: free the disk space.
        for old in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            if old.path() != dir && old.path().is_dir() {
                let _ = std::fs::remove_dir_all(old.path());
            }
        }
        Some(Self::at(dir))
    }

    /// Cache in a given folder (the server keeps one per game version).
    pub fn at(dir: PathBuf) -> Self {
        Self { dir, vehicle_index: Mutex::new(None) }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Relative cache path of a map's terrain / a tank model (also used as the upload path).
    pub fn terrain_path(map: &str) -> String {
        format!("maps/{map}.bin")
    }
    pub fn tank_path(vehicle_type: &str) -> String {
        format!("tanks/{}.bin", vehicle_type.replace(':', "-"))
    }

    /// Terrain for a map: from cache, or parsed from the map package and cached.
    pub fn terrain(&self, gd: &GameData, map: &str) -> Option<Terrain> {
        let path = self.dir.join(Self::terrain_path(map));
        if let Some(t) = std::fs::read(&path).ok().and_then(|b| decode_terrain(&b)) {
            return Some(t);
        }
        let t = build_terrain(gd, map)?;
        let mut b = Vec::new();
        for v in [t.x0, t.z0, t.spacing] {
            put_f32(&mut b, v);
        }
        put_u32(&mut b, t.w as u32);
        put_u32(&mut b, t.h as u32);
        t.heights.iter().for_each(|v| put_f32(&mut b, *v));
        save(&path, &b);
        Some(t)
    }

    /// Ground image of a map (DDS, stored as is: DXT1 hardly compresses).
    pub fn ground_path(map: &str) -> String {
        format!("maps/{map}.ground.dds")
    }

    pub fn scene_path(map: &str, gameplay_id: i32) -> String {
        format!("maps/{map}.{gameplay_id}.scene.bin")
    }

    pub fn prefab_path(name: &str) -> String {
        format!("prefabs/{name}.bin")
    }

    /// Buildings / water / ground texture for a map in a game mode (objects differ per mode).
    pub fn scene(&self, gd: &GameData, map: &str, gameplay_id: i32) -> Option<MapScene> {
        let path = self.dir.join(Self::scene_path(map, gameplay_id));
        if let Some(mut sc) = std::fs::read(&path).ok().and_then(|b| decode_scene(&b)) {
            sc.albedo = std::fs::read(self.dir.join(Self::ground_path(map))).ok();
            return Some(sc);
        }
        let sc = self.build_scene(gd, map, gameplay_id)?;
        save(&path, &encode_scene(&sc));
        Some(sc)
    }

    /// Geometry of a map-scenario prefab (e.g. a building's `_normal` / `_destroyed` state),
    /// in the prefab's own space.
    pub fn prefab(&self, gd: &GameData, name: &str) -> Option<Mesh> {
        let path = self.dir.join(Self::prefab_path(name));
        if let Some(m) = std::fs::read(&path).ok().and_then(|b| decode_mesh(&b)) {
            return Some(m);
        }
        let file = self.find_by_file_name(gd, name)?;
        let json: serde_json::Value = serde_json::from_slice(&gd_read_any(self, gd, &file)?).ok()?;
        let mut mesh = Mesh::default();
        let mut cache: HashMap<String, Option<Mesh>> = HashMap::new();
        walk_prefab(&json, IDENTITY, &mut |model: &str, m: &[f32; 16]| {
            let prim = model.trim_end_matches(".model").to_string() + ".primitives";
            let part = cache
                .entry(prim.clone())
                .or_insert_with(|| {
                    let mesh = self.object_mesh(gd, &prim)?;
                    let (mn, mx) = mesh.bounds();
                    Some(mesh.simplify((sub_len(mn, mx) / 16.0).clamp(0.2, 2.0)))
                })
                .clone();
            if let Some(part) = part {
                let base = mesh.positions.len() as u32;
                for p in &part.positions {
                    mesh.positions.push(xform(m, *p));
                }
                mesh.indices.extend(part.indices.iter().map(|i| i + base));
                let mut c = part.colors.clone();
                c.resize(part.positions.len(), [150, 145, 135]);
                mesh.colors.extend_from_slice(&c);
            }
        });
        let mut b = Vec::new();
        put_mesh(&mut b, &mesh);
        save(&path, &b);
        Some(mesh)
    }

    fn find_by_file_name(&self, gd: &GameData, name: &str) -> Option<String> {
        self.find_file(gd, "");
        let idx = self.vehicle_index.lock().unwrap();
        let suffix = format!("/{}", name.to_lowercase());
        idx.as_ref()?.keys().find(|k| k.ends_with(&suffix)).cloned()
    }

    /// A texture at about ATLAS_TILE px. `path#CxR` = only the first cell of a C x R atlas image.
    fn load_texture(&self, gd: &GameData, spec: &str) -> Option<super::dds::Image> {
        let (path, grid) = match spec.split_once('#') {
            Some((p, g)) => {
                let (c, r) = g.split_once('x')?;
                (p, (c.parse::<usize>().ok()?.max(1), r.parse::<usize>().ok()?.max(1)))
            }
            None => (spec, (1, 1)),
        };
        let b = self.find_file(gd, path)?;
        let w = u32::from_le_bytes(b.get(16..20)?.try_into().ok()?) as usize / grid.0;
        let img = super::dds::decode_level(&b, (w / ATLAS_TILE).max(1).trailing_zeros() as usize)?;
        if grid == (1, 1) {
            return Some(img);
        }
        let (cw, ch) = ((img.width / grid.0).max(1), (img.height / grid.1).max(1));
        let mut rgba = Vec::with_capacity(cw * ch * 4);
        for y in 0..ch {
            rgba.extend_from_slice(&img.rgba[(y * img.width) * 4..(y * img.width + cw) * 4]);
        }
        Some(super::dds::Image { width: cw, height: ch, rgba })
    }

    /// `.atlas` (compiled as `.atlas_processed`): header, then per tile a rect and the source
    /// texture path (`.png`, shipped as `.dds`). Returns the first tile's texture.
    fn atlas_base_tile(&self, gd: &GameData, atlas: &str) -> Option<String> {
        let b = self.find_file(gd, &format!("{atlas}_processed"))?;
        let start = b.windows(8).position(|w| w == b"content/")?;
        let end = start + b[start..].iter().position(|&c| c == 0)?;
        let png = std::str::from_utf8(&b[start..end]).ok()?;
        let dds = png.rsplit_once('.').map(|(s, _)| format!("{s}.dds"))?;
        if self.find_file(gd, &dds).is_some() {
            return Some(dds);
        }
        let name = dds.rsplit('/').next()?;
        self.find_by_file_name(gd, name)
    }

    fn build_scene(&self, gd: &GameData, map: &str, gameplay_id: i32) -> Option<MapScene> {
        let arena = gd.arena(map)?;
        let terrain = self.terrain(gd, map)?;
        let pkg = format!("{map}.pkg");
        let mut scene = MapScene::default();
        // Map overview colour over the battle area (also colours rocks).
        let mut overview: Option<super::dds::Image> = None;

        // Ground texture: the client's map-wide pre-blended albedo (`global_AM.dds`, 4096 px over
        // the whole space) at full resolution, cropped to the battle area. Rows run north -> south.
        if let Some(img) = gd.read(&pkg, &format!("spaces/{map}/global_AM.dds")).and_then(|b| super::dds::decode(&b)) {
            let [sx0, sz0] = [terrain.x0, terrain.z0];
            let sx1 = terrain.x0 + (terrain.w - 1) as f32 * terrain.spacing;
            let sz1 = terrain.z0 + (terrain.h - 1) as f32 * terrain.spacing;
            let [bx0, bz0] = arena.bbox_min;
            let [bx1, bz1] = arena.bbox_max;
            let px = |x: f32| (((x - sx0) / (sx1 - sx0)) * img.width as f32).round().clamp(0.0, img.width as f32) as usize;
            let py = |z: f32| (((sz1 - z) / (sz1 - sz0)) * img.height as f32).round().clamp(0.0, img.height as f32) as usize;
            let (x0, x1, y0, y1) = (px(bx0), px(bx1), py(bz1), py(bz0));
            if x1 > x0 && y1 > y0 {
                let mut rgba = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
                for y in y0..y1 {
                    rgba.extend_from_slice(&img.rgba[(y * img.width + x0) * 4..(y * img.width + x1) * 4]);
                }
                let base = super::dds::Image { width: x1 - x0, height: y1 - y0, rgba };
                overview = Some(base.clone());
                // Sharper: the map colour plus the ground textures' detail, up to 8 px/m.
                // Baked once per map (all game modes share it).
                let gpath = self.dir.join(Self::ground_path(map));
                scene.albedo = match std::fs::read(&gpath) {
                    Ok(b) => Some(b),
                    Err(_) => {
                        let ppm = (8192.0 / (bx1 - bx0).max(bz1 - bz0)).min(8.0);
                        let img = super::ground::bake(gd, map, [bx0, bz0, bx1, bz1], ppm, &base, &|n: &str| self.find_file(gd, n)).unwrap_or(base);
                        let dds = super::ground::dds_dxt1_mips(&img);
                        if let Some(d) = gpath.parent() {
                            let _ = std::fs::create_dir_all(d);
                        }
                        let _ = std::fs::write(&gpath, &dds);
                        Some(dds)
                    }
                };
                scene.albedo_extent = [bx0, bz0, bx1, bz1];
            }
        }

        let Some(space) = gd.read(&pkg, &format!("spaces/{map}/space.bin")).and_then(|b| super::space::parse(&b)) else {
            return Some(scene);
        };
        let [bx0, bz0] = arena.bbox_min;
        let [bx1, bz1] = arena.bbox_max;
        for (mn, mx) in &space.water {
            let (x0, z0, x1, z1) = (mn[0].max(bx0), mn[2].max(bz0), mx[0].min(bx1), mx[2].min(bz1));
            if x1 > x0 && z1 > z0 {
                scene.water.push([x0, z0, x1, z1, mn[1].max(mx[1])]);
            }
        }

        // Objects inside the battle area, skipping small props; each model simplified once.
        // Textures go into one atlas: path -> tile index.
        let mut meshes: HashMap<usize, Option<(Mesh, Vec<u16>, Vec<bool>)>> = HashMap::new();
        let mut tiles: HashMap<String, u16> = HashMap::new();
        let mut tile_imgs: Vec<super::dds::Image> = Vec::new();
        let mut tile_means: Vec<[f32; 3]> = Vec::new();
        for (model, m, mask) in &space.instances {
            // Objects of other game modes / map variants (bit N = gameplay type N).
            // Bit N = gameplay type N (e.g. Live Oaks: 0x..01 = Standard only, 0x7ffffffe = all but Standard).
            if (0..32).contains(&gameplay_id) && mask & (1u32 << gameplay_id) == 0 {
                if std::env::var_os("LIVEREPLAY_DEBUG_OBJECTS").is_some() {
                    eprintln!("MASKED {} mask={mask:#x} at {:.0},{:.0}", space.models[*model].0.as_deref().unwrap_or("?"), m[12], m[14]);
                }
                continue;
            }
            let (tx, tz) = (m[12], m[14]);
            if tx < bx0 - 10.0 || tx > bx1 + 10.0 || tz < bz0 - 10.0 || tz > bz1 + 10.0 {
                continue;
            }
            let (path, mn, mx) = &space.models[*model];
            let size = sub_len(*mn, *mx);
            if std::env::var_os("LIVEREPLAY_DEBUG_OBJECTS").is_some() {
                let on_water = space.water.iter().any(|(wmn, wmx)| tx >= wmn[0] && tx <= wmx[0] && tz >= wmn[2] && tz <= wmx[2]);
                eprintln!("OBJ {} water={on_water} size={size:.1} keep={} mask={mask:#x} at {tx:.0},{:.1},{tz:.0}", path.as_deref().unwrap_or("?"), keep_object(path.as_deref().unwrap_or(""), size), m[13]);
            }
            if !keep_object(path.as_deref().unwrap_or(""), size) {
                continue;
            }
            let mesh = meshes.entry(*model).or_insert_with(|| {
                let path = path.as_ref()?;
                let (mesh, tex_list, rock_tex) = self.object_mesh_tex(gd, path)?;
                // local texture id -> atlas tile
                let mut map = Vec::with_capacity(tex_list.len());
                for tp in &tex_list {
                    let next = tiles.len() as u16;
                    let tile = match tiles.get(tp) {
                        Some(&t) => t,
                        None if (next as usize) < ATLAS_MAX_GRID * ATLAS_MAX_GRID => {
                            let img = self.load_texture(gd, tp);
                            if let Some(img) = img {
                                let px = (img.rgba.len() / 4).max(1) as f32;
                                tile_means.push([0, 1, 2].map(|k| img.rgba.chunks_exact(4).map(|p| p[k] as f32).sum::<f32>() / px));
                                tile_imgs.push(img);
                                tiles.insert(tp.clone(), next);
                                next
                            } else {
                                u16::MAX
                            }
                        }
                        None => u16::MAX,
                    };
                    map.push(tile);
                }
                Some((mesh.simplify((size / 16.0).clamp(0.3, 2.0)), map, rock_tex))
            });
            let Some((mesh, tile_map, rock_tex)) = mesh else {
                if std::env::var_os("LIVEREPLAY_DEBUG_OBJECTS").is_some() {
                    eprintln!("MESHFAIL {}", path.as_deref().unwrap_or("?"));
                }
                continue;
            };
            let base = scene.objects.positions.len() as u32;
            for p in &mesh.positions {
                scene.objects.positions.push([
                    m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
                    m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
                    m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
                ]);
            }
            scene.objects.indices.extend(mesh.indices.iter().map(|i| i + base));
            let mut c = mesh.colors.clone();
            c.resize(mesh.positions.len(), [150, 145, 135]);
            // Textured triangles: the vertex colour is a multiplier for the texture (128 = x1).
            // Rocks get the map overview colour at their position, so cliffs match their surroundings.
            if mesh.tri_tex.len() * 3 == mesh.indices.len() {
                for (ti, &lt) in mesh.tri_tex.iter().enumerate() {
                    let Some(&tile) = tile_map.get(lt as usize) else { continue };
                    if tile == u16::MAX {
                        continue;
                    }
                    let rock = rock_tex.get(lt as usize).copied().unwrap_or(false);
                    for k in 0..3 {
                        let vi = mesh.indices[ti * 3 + k] as usize;
                        c[vi] = match (rock, &overview) {
                            (true, Some(img)) => {
                                let wp = scene.objects.positions[base as usize + vi];
                                let u = ((wp[0] - bx0) / (bx1 - bx0)).clamp(0.0, 0.999);
                                let v = ((bz1 - wp[2]) / (bz1 - bz0)).clamp(0.0, 0.999);
                                let o = (((v * img.height as f32) as usize) * img.width + (u * img.width as f32) as usize) * 4;
                                let mean = tile_means.get(tile as usize).copied().unwrap_or([128.0; 3]);
                                // Baked variation and fake shading: a stable random brightness per spot
                                // (hash of the world position) and darker down/side-facing faces.
                                let p = [0, 1, 2].map(|j| scene.objects.positions[base as usize + mesh.indices[ti * 3 + j] as usize]);
                                let (e1, e2) = ([p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]], [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]]);
                                let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                                let up = n[1] / (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                                let shade = 0.62 + 0.38 * (up.abs() * 0.5 + 0.5);
                                let h = ((wp[0] * 1.31).floor() as i32).wrapping_mul(73856093) ^ ((wp[1] * 1.31).floor() as i32).wrapping_mul(19349663) ^ ((wp[2] * 1.31).floor() as i32).wrapping_mul(83492791);
                                let noise = 0.82 + 0.36 * ((h as u32 % 1000) as f32 / 1000.0);
                                [0, 1, 2].map(|j| (img.rgba[o + j] as f32 / mean[j].max(8.0) * 128.0 * shade * noise).clamp(0.0, 255.0) as u8)
                            }
                            _ => [128, 128, 128],
                        };
                    }
                }
            }
            scene.objects.colors.extend_from_slice(&c);
            if mesh.corner_uv.len() == mesh.indices.len() {
                let uv0 = scene.objects.corner_uv.len();
                scene.objects.corner_uv.extend_from_slice(&mesh.corner_uv);
                // Rocks: their own UVs address a small atlas region, so project the stone texture by
                // world position instead (5 m per repeat, plane chosen by the face direction).
                for (ti, &lt) in mesh.tri_tex.iter().enumerate() {
                    if !rock_tex.get(lt as usize).copied().unwrap_or(false) {
                        continue;
                    }
                    let p = [0, 1, 2].map(|k| scene.objects.positions[base as usize + mesh.indices[ti * 3 + k] as usize]);
                    let (e1, e2) = ([p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]], [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]]);
                    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                    let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
                    for k in 0..3 {
                        let q = p[k];
                        let uv = if ay >= ax && ay >= az { [q[0], q[2]] } else if ax >= az { [q[2], q[1]] } else { [q[0], q[1]] };
                        scene.objects.corner_uv[uv0 + ti * 3 + k] = [uv[0] / 5.0, uv[1] / 5.0];
                    }
                }
                scene.objects.tri_tex.extend(mesh.tri_tex.iter().map(|&t| tile_map.get(t as usize).copied().unwrap_or(u16::MAX)));
            } else {
                scene.objects.corner_uv.extend(std::iter::repeat([0.0, 0.0]).take(mesh.indices.len()));
                scene.objects.tri_tex.extend(std::iter::repeat(u16::MAX).take(mesh.indices.len() / 3));
            }
        }
        if !tile_imgs.is_empty() {
            // Square grid just big enough for the textures used.
            let grid = (tile_imgs.len() as f32).sqrt().ceil() as usize;
            let atlas_w = grid * ATLAS_TILE;
            let mut atlas = vec![0u8; atlas_w * atlas_w * 4];
            for (i, img) in tile_imgs.iter().enumerate() {
                let (tx, ty) = ((i % grid) * ATLAS_TILE, (i / grid) * ATLAS_TILE);
                for y in 0..ATLAS_TILE {
                    for x in 0..ATLAS_TILE {
                        let s = ((y * img.height / ATLAS_TILE) * img.width + x * img.width / ATLAS_TILE) * 4;
                        let d = ((ty + y) * atlas_w + tx + x) * 4;
                        atlas[d..d + 3].copy_from_slice(&img.rgba[s..s + 3]);
                        atlas[d + 3] = 255;
                    }
                }
            }
            scene.atlas = Some(super::ground::dds_dxt1_mips(&super::dds::Image { width: atlas_w, height: atlas_w, rgba: atlas }));
            scene.atlas_grid = grid as u32;
        }
        Some(scene)
    }

    /// A map object's intact geometry: primitive groups whose material isn't a `d_` (destroyed)
    /// variant, with each vertex coloured from its material's diffuse texture.
    fn object_mesh(&self, gd: &GameData, primitives_path: &str) -> Option<Mesh> {
        // Vertex-coloured only (prefabs have no atlas).
        self.object_mesh_tex(gd, primitives_path).map(|(mut m, _, _)| {
            m.corner_uv.clear();
            m.tri_tex.clear();
            m
        })
    }

    /// Like `object_mesh`, plus per-corner UVs and per-triangle texture ids into the returned
    /// list of texture paths.
    /// Also returns, per texture, whether it is a rock texture (coloured by the map overview).
    fn object_mesh_tex(&self, gd: &GameData, primitives_path: &str) -> Option<(Mesh, Vec<String>, Vec<bool>)> {
        let sections = parse_sections(&self.find_file(gd, &format!("{primitives_path}_processed"))?)?;
        let visual_path = primitives_path.trim_end_matches(".primitives").to_string() + ".visual_processed";
        let visual = self.find_file(gd, &visual_path).and_then(|b| super::pxml::parse(&b));
        let mut textures: HashMap<String, Option<super::dds::Image>> = HashMap::new();
        let mut tex_list: Vec<String> = Vec::new();
        let mut rock_tex: Vec<bool> = Vec::new();
        let mut mesh = Mesh::default();
        let mut add = |sec: &super::models::Section, group: Option<usize>, tex: Option<(&super::dds::Image, u16)>| {
            let range = match group.and_then(|g| sec.groups.get(g)) {
                Some(&(start, prims)) => start..(start + prims * 3).min(sec.indices.len()),
                None => 0..sec.indices.len(),
            };
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let uv_of = |i: u32| sec.uvs.get(i as usize).copied().unwrap_or([0.0, 0.0]);
            let tri_start = mesh.indices.len();
            for &i in &sec.indices[range] {
                let Some(&pos) = sec.positions.get(i as usize) else { continue };
                let new = *remap.entry(i).or_insert_with(|| {
                    mesh.positions.push(pos);
                    mesh.colors.push(sample(tex.map(|t| t.0), uv_of(i)));
                    mesh.positions.len() as u32 - 1
                });
                mesh.indices.push(new);
                mesh.corner_uv.push(uv_of(i));
            }
            let added = (mesh.indices.len() - tri_start) / 3;
            let id = tex.map(|t| t.1).unwrap_or(u16::MAX);
            mesh.tri_tex.extend(std::iter::repeat(id).take(added));
            // keep corners aligned with whole triangles
            mesh.indices.truncate(tri_start + added * 3);
            mesh.corner_uv.truncate(tri_start + added * 3);
        };
        match visual {
            Some(v) => {
                for rs in v.children_named("renderSet") {
                    let Some(geom) = rs.child("geometry") else { continue };
                    let prim = geom.child_text("primitive").unwrap_or("indices");
                    let prefix = prim.trim_end_matches("indices").to_string();
                    let Some(sec) = sections.get(&prefix).or_else(|| sections.get("")) else { continue };
                    for pg in geom.children_named("primitiveGroup") {
                        let mat = pg.child("material");
                        let id = mat.and_then(|m| m.child_text("identifier")).unwrap_or("");
                        if id.starts_with("d_") {
                            continue; // pre-broken rubble shown only after destruction
                        }
                        let prop = |name: &str| {
                            mat.and_then(|m| m.children_named("property").find(|p| p.text() == name).and_then(|p| p.child_text("Texture")).map(str::to_string))
                        };
                        // Normal materials: diffuseMap. Tiled-atlas materials (rocks): the atlas's
                        // first (base) tile texture, which tiles over the model's UVs.
                        // Collision-only shapes (no shader, `s_...`) are never drawn by the game.
                        if mat.and_then(|m| m.child("fx")).is_none() && id.starts_with("s_") {
                            continue;
                        }
                        // Texture by material type: diffuseMap (classic), the first tile of tiled PBS
                        // materials (buildings), or the atlas's base tile (rocks).
                        let classic = prop("diffuseMap").or_else(|| prop("albedoHeightTile0"));
                        let is_rock = classic.is_none();
                        let tex_path = classic.or_else(|| {
                            let a = prop("atlasAlbedoHeight")?;
                            if a.ends_with(".dds") {
                                // A whole atlas image: use its first tile ("#cols x rows" = crop).
                                let grid = mat
                                    .and_then(|m| m.children_named("property").find(|p| p.text() == "g_atlasSizes"))
                                    .and_then(|p| p.child_text("Vector4"))
                                    .map(|v| v.split_whitespace().filter_map(|x| x.parse::<f32>().ok()).collect::<Vec<_>>())
                                    .filter(|v| v.len() >= 2 && v[0] >= 1.0 && v[1] >= 1.0)
                                    .map(|v| (v[0] as usize, v[1] as usize))
                                    .unwrap_or((2, 2));
                                Some(format!("{a}#{}x{}", grid.0, grid.1))
                            } else {
                                self.atlas_base_tile(gd, &a)
                            }
                        });
                        let tex = tex_path.and_then(|tp| {
                            let img = textures
                                .entry(tp.clone())
                                .or_insert_with(|| self.load_texture(gd, &tp))
                                .clone()?;
                            let local = match tex_list.iter().position(|t| *t == tp) {
                                Some(i) => i,
                                None => {
                                    tex_list.push(tp.clone());
                                    rock_tex.push(is_rock);
                                    tex_list.len() - 1
                                }
                            };
                            Some((img, local as u16))
                        });
                        if tex.is_none() && std::env::var_os("LIVEREPLAY_DEBUG_OBJECTS").is_some() {
                            let fx = mat.and_then(|m| m.child_text("fx")).unwrap_or("?");
                            let props: Vec<String> = mat
                                .map(|m| m.children_named("property").map(|p| format!("{}={}", p.text(), p.child_text("Texture").unwrap_or(""))).collect())
                                .unwrap_or_default();
                            let kids: Vec<String> = mat.map(|m| m.children.iter().map(|c| format!("{}={}", c.name, c.text().chars().take(80).collect::<String>())).collect()).unwrap_or_default();
                            eprintln!("NOTEX {visual_path} fx={fx} props={} kids={}", props.join(" "), kids.join(" "));
                        }
                        add(sec, pg.text().parse().ok(), tex.as_ref().map(|t| (&t.0, t.1)));
                    }
                }
            }
            None => {
                for sec in sections.values() {
                    add(sec, None, None);
                }
            }
        }
        (!mesh.indices.is_empty()).then_some((mesh, tex_list, rock_tex))
    }

    /// Simplified tank model for a vehicle type such as `ussr:R04_T-34`.
    pub fn tank(&self, gd: &GameData, vehicle_type: &str) -> Option<TankModel> {
        let path = self.dir.join(Self::tank_path(vehicle_type));
        if let Some(m) = std::fs::read(&path).ok().and_then(|b| decode_tank(&b)) {
            return Some(m);
        }
        let m = self.build_tank(gd, vehicle_type)?;
        let mut b = Vec::new();
        put_mesh(&mut b, &m.body);
        put_mesh(&mut b, &m.turret);
        m.turret_pivot.iter().for_each(|v| put_f32(&mut b, *v));
        put_mesh(&mut b, &m.gun);
        m.gun_pivot.iter().for_each(|v| put_f32(&mut b, *v));
        save(&path, &b);
        Some(m)
    }

    /// Which package holds each model file (built once per session).
    fn find_file(&self, gd: &GameData, path: &str) -> Option<Vec<u8>> {
        let mut idx = self.vehicle_index.lock().unwrap();
        if idx.is_none() {
            let mut map = HashMap::new();
            if let Ok(rd) = std::fs::read_dir(gd.root.join("res").join("packages")) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    // Model files can live in any package (e.g. a hull in `particles-part1.pkg`).
                    // Prefer normal packages; some base models only ship in the `_hd` ones.
                    if name.ends_with(".pkg") {
                        let hd = name.contains("_hd");
                        if let Ok(p) = Pkg::open(&e.path()) {
                            for n in p.names().filter(|n| n.ends_with(".primitives_processed") || n.ends_with(".visual_processed") || n.ends_with(".prefab") || n.ends_with(".atlas_processed") || (n.ends_with(".dds") && (n.starts_with("content/") || n.starts_with("maps/landscape/")))) {
                                let existing_is_hd = map.get(n).map(|v: &String| v.contains("_hd"));
                                if existing_is_hd.is_none() || (existing_is_hd == Some(true) && !hd) {
                                    map.insert(n.clone(), name.clone());
                                }
                            }
                        }
                    }
                }
            }
            *idx = Some(map);
        }
        if path.is_empty() {
            return None;
        }
        let pkg = idx.as_ref()?.get(&path.to_lowercase())?.clone();
        drop(idx);
        gd.read(&pkg, path)
    }

    fn part(&self, gd: &GameData, model: &str, cell: f32) -> Option<Mesh> {
        let prim = model.trim().replace(".model", ".primitives_processed");
        let mesh = parse_primitives(&self.find_file(gd, &prim)?)?;
        Some(mesh.simplify(cell))
    }

    fn build_tank(&self, gd: &GameData, vehicle_type: &str) -> Option<TankModel> {
        let (nation, tag) = vehicle_type.split_once(':')?;
        // Event variants (e.g. `_7x7`) may not have their own definition: fall back to the base tag.
        let mut t = tag;
        let def = loop {
            if let Some(d) = gd.script_xml(&format!("scripts/item_defs/vehicles/{nation}/{t}.xml")) {
                break d;
            }
            t = &t[..t.rfind('_')?];
        };
        let model_of = |n: &super::pxml::Node| n.child("models").and_then(|m| m.child_text("undamaged")).map(str::to_string);
        let v3 = |n: Option<&super::pxml::Node>| {
            let f = n.map(|n| n.floats()).unwrap_or_default();
            [f.first().copied().unwrap_or(0.0), f.get(1).copied().unwrap_or(0.0), f.get(2).copied().unwrap_or(0.0)]
        };

        let chassis = def.child("chassis")?.children.last()?;
        let hull = def.child("hull")?;
        let turret = def.child("turrets0")?.children.last()?;
        let gun = turret.child("guns").and_then(|g| g.children.last());

        let hull_pos = v3(chassis.child("hullPosition"));
        let turret_pos = v3(hull.child("turretPositions").and_then(|t| t.children.first()));
        let gun_pos = v3(turret.child("gunPosition"));

        let mut body = self.part(gd, &model_of(chassis)?, 0.26).unwrap_or_default();
        let mut hull_mesh = self.part(gd, &model_of(hull)?, 0.22)?;
        hull_mesh.positions.iter_mut().for_each(|p| {
            p[0] += hull_pos[0];
            p[1] += hull_pos[1];
            p[2] += hull_pos[2];
        });
        body.append(&hull_mesh);
        let turret_mesh = model_of(turret).and_then(|m| self.part(gd, &m, 0.18)).unwrap_or_default();
        let mut gun_mesh = gun.and_then(model_of).and_then(|m| self.part(gd, &m, 0.07)).unwrap_or_default();
        // Gun meshes are modelled barrel-towards -z (the client rotates the gun node); vehicles face +z.
        let (mn, mx) = gun_mesh.bounds();
        if mn[2].abs() > mx[2].abs() {
            gun_mesh.positions.iter_mut().for_each(|p| {
                p[0] = -p[0];
                p[2] = -p[2];
            });
        }
        Some(TankModel {
            body,
            turret: turret_mesh,
            turret_pivot: [hull_pos[0] + turret_pos[0], hull_pos[1] + turret_pos[1], hull_pos[2] + turret_pos[2]],
            gun: gun_mesh,
            gun_pivot: gun_pos,
        })
    }
}

fn build_terrain(gd: &GameData, map: &str) -> Option<Terrain> {
    let pkg = Pkg::open(&gd.root.join("res").join("packages").join(format!("{map}.pkg"))).ok()?;
    let prefix = format!("spaces/{map}/");
    let mut chunks: Vec<(i32, i32, String)> = Vec::new();
    for n in pkg.names() {
        if let Some(rest) = n.strip_prefix(&prefix) {
            if rest.len() == 25 && rest.ends_with("o.cdata_processed") {
                let cx = i16::from_str_radix(&rest[0..4], 16).ok().or_else(|| u16::from_str_radix(&rest[0..4], 16).ok().map(|v| v as i16))?;
                let cz = i16::from_str_radix(&rest[4..8], 16).ok().or_else(|| u16::from_str_radix(&rest[4..8], 16).ok().map(|v| v as i16))?;
                chunks.push((cx as i32, cz as i32, n.clone()));
            }
        }
    }
    if chunks.is_empty() {
        return None;
    }
    let (min_x, max_x) = (chunks.iter().map(|c| c.0).min()?, chunks.iter().map(|c| c.0).max()?);
    let (min_z, max_z) = (chunks.iter().map(|c| c.1).min()?, chunks.iter().map(|c| c.1).max()?);
    let per = 64 / TERRAIN_STEP; // cells per chunk
    let w = (max_x - min_x + 1) as usize * per + 1;
    let h = (max_z - min_z + 1) as usize * per + 1;
    let mut heights = vec![f32::NAN; w * h];
    for (cx, cz, name) in &chunks {
        let Some((n, hm)) = pkg.read(name).and_then(|b| parse_chunk_heights(&b)) else { continue };
        // Chunks have 65 or 129 (or other 2^k+1) samples per 100 m.
        if n < 65 || (n - 1) % 64 != 0 {
            continue;
        }
        let k = (n - 1) / 64 * TERRAIN_STEP;
        let ox = (cx - min_x) as usize * per;
        let oz = (cz - min_z) as usize * per;
        for j in 0..=per {
            for i in 0..=per {
                heights[(oz + j) * w + ox + i] = hm[(j * k) * n + i * k];
            }
        }
    }
    let fill = heights.iter().copied().filter(|v| v.is_finite()).fold(f32::MAX, f32::min);
    heights.iter_mut().filter(|v| !v.is_finite()).for_each(|v| *v = if fill == f32::MAX { 0.0 } else { fill });
    Some(Terrain {
        x0: min_x as f32 * 100.0,
        z0: min_z as f32 * 100.0,
        spacing: 100.0 / per as f32,
        w,
        h,
        heights,
    })
}
