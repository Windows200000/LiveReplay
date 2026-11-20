//! Compact 3D data for the web viewer, made by the desktop client from its 3D cache:
//! geometry quantised (1 cm, 1/1024 texture units) and delta-coded, ground image limited to
//! 2048 px, object textures to 64 px per atlas tile. Same container as battles (format byte +
//! deflate(CBOR)), so packs stay readable when fields are added.

use serde::{Deserialize, Serialize};

use super::cache3d::{MapScene, TankModel, Terrain};
use super::models::Mesh;
use crate::replay::wire::{self, get_varint, put_varint};

/// Largest ground image and atlas tile sent to browsers.
pub const WEB_GROUND_PX: usize = 2048;
pub const WEB_TILE_PX: usize = 64;

const POS: f32 = 100.0;
const UV: f32 = 1024.0;

/// A DXT1 DDS without its mip levels larger than `max_w` pixels wide.
pub fn drop_mips(dds: &[u8], max_w: usize) -> Vec<u8> {
    let Some(levels) = super::dds::dxt1_levels(dds) else { return dds.to_vec() };
    let skip = levels.iter().take_while(|l| l.0 > max_w).count().min(levels.len() - 1);
    if skip == 0 {
        return dds.to_vec();
    }
    let mut out = dds[..128].to_vec();
    let (w, h) = (levels[skip].0, levels[skip].1);
    out[12..16].copy_from_slice(&(h as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(w as u32).to_le_bytes());
    out[28..32].copy_from_slice(&((levels.len() - skip) as u32).to_le_bytes());
    // Pitch / linear size field: size of the top level.
    out[20..24].copy_from_slice(&(levels[skip].2.len() as u32).to_le_bytes());
    for l in &levels[skip..] {
        out.extend_from_slice(l.2);
    }
    out
}

fn q(v: f32, s: f32) -> i64 {
    (v * s).round() as i64
}

pub fn encode_mesh(m: &Mesh) -> Vec<u8> {
    let mut b = Vec::new();
    put_varint(&mut b, m.positions.len() as i64);
    let mut prev = [0i64; 3];
    for p in &m.positions {
        for k in 0..3 {
            let v = q(p[k], POS);
            put_varint(&mut b, v - prev[k]);
            prev[k] = v;
        }
    }
    put_varint(&mut b, m.indices.len() as i64);
    let mut pi = 0i64;
    for &i in &m.indices {
        put_varint(&mut b, i as i64 - pi);
        pi = i as i64;
    }
    put_varint(&mut b, m.colors.len() as i64);
    for c in &m.colors {
        b.extend_from_slice(c);
    }
    put_varint(&mut b, m.corner_uv.len() as i64);
    let mut pu = [0i64; 2];
    for uv in &m.corner_uv {
        for k in 0..2 {
            let v = q(uv[k], UV);
            put_varint(&mut b, v - pu[k]);
            pu[k] = v;
        }
    }
    put_varint(&mut b, m.tri_tex.len() as i64);
    let mut pt = 0i64;
    for &t in &m.tri_tex {
        put_varint(&mut b, t as i64 - pt);
        pt = t as i64;
    }
    b
}

pub fn decode_mesh(b: &[u8]) -> Option<Mesh> {
    let mut at = 0;
    let mut n = |at: &mut usize| get_varint(b, at).map(|v| v.max(0) as usize);
    let np = n(&mut at)?;
    let mut prev = [0i64; 3];
    let mut positions = Vec::with_capacity(np.min(1 << 24));
    for _ in 0..np {
        for p in prev.iter_mut() {
            *p += get_varint(b, &mut at)?;
        }
        positions.push(prev.map(|v| v as f32 / POS));
    }
    let ni = n(&mut at)?;
    let mut indices = Vec::with_capacity(ni.min(1 << 26));
    let mut pi = 0i64;
    for _ in 0..ni {
        pi += get_varint(b, &mut at)?;
        indices.push(pi as u32);
    }
    let nc = n(&mut at)?;
    let colors = b.get(at..at + nc * 3)?.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    at += nc * 3;
    let nu = n(&mut at)?;
    let mut corner_uv = Vec::with_capacity(nu.min(1 << 26));
    let mut pu = [0i64; 2];
    for _ in 0..nu {
        for p in pu.iter_mut() {
            *p += get_varint(b, &mut at)?;
        }
        corner_uv.push(pu.map(|v| v as f32 / UV));
    }
    let nt = n(&mut at)?;
    let mut tri_tex = Vec::with_capacity(nt.min(1 << 24));
    let mut pt = 0i64;
    for _ in 0..nt {
        pt += get_varint(b, &mut at)?;
        tri_tex.push(pt as u16);
    }
    Some(Mesh { positions, indices, colors, corner_uv, tri_tex })
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct ScenePack {
    #[serde(with = "wire::bytes")]
    objects: Vec<u8>,
    water: Vec<[f32; 5]>,
    #[serde(with = "wire::bytes")]
    albedo: Vec<u8>,
    albedo_extent: [f32; 4],
    #[serde(with = "wire::bytes")]
    atlas: Vec<u8>,
    atlas_grid: u32,
}

pub fn scene_pack(sc: &MapScene) -> Vec<u8> {
    let tile_levels = |a: &Vec<u8>| drop_mips(a, sc.atlas_grid.max(1) as usize * WEB_TILE_PX);
    wire::pack(&ScenePack {
        objects: encode_mesh(&sc.objects),
        water: sc.water.clone(),
        albedo: sc.albedo.as_ref().map(|a| drop_mips(a, WEB_GROUND_PX)).unwrap_or_default(),
        albedo_extent: sc.albedo_extent,
        atlas: sc.atlas.as_ref().map(tile_levels).unwrap_or_default(),
        atlas_grid: sc.atlas_grid,
    })
}

pub fn decode_scene_pack(b: &[u8]) -> Option<MapScene> {
    let p: ScenePack = wire::unpack(b).ok()?;
    Some(MapScene {
        objects: decode_mesh(&p.objects)?,
        water: p.water,
        albedo: (!p.albedo.is_empty()).then_some(p.albedo),
        albedo_extent: p.albedo_extent,
        atlas: (!p.atlas.is_empty()).then_some(p.atlas),
        atlas_grid: p.atlas_grid,
    })
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct TankPack {
    #[serde(with = "wire::bytes")]
    body: Vec<u8>,
    #[serde(with = "wire::bytes")]
    turret: Vec<u8>,
    turret_pivot: [f32; 3],
    #[serde(with = "wire::bytes")]
    gun: Vec<u8>,
    gun_pivot: [f32; 3],
}

pub fn tank_pack(t: &TankModel) -> Vec<u8> {
    wire::pack(&TankPack { body: encode_mesh(&t.body), turret: encode_mesh(&t.turret), turret_pivot: t.turret_pivot, gun: encode_mesh(&t.gun), gun_pivot: t.gun_pivot })
}

pub fn decode_tank_pack(b: &[u8]) -> Option<TankModel> {
    let p: TankPack = wire::unpack(b).ok()?;
    Some(TankModel { body: decode_mesh(&p.body)?, turret: decode_mesh(&p.turret)?, turret_pivot: p.turret_pivot, gun: decode_mesh(&p.gun)?, gun_pivot: p.gun_pivot })
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct MeshPack {
    #[serde(with = "wire::bytes")]
    mesh: Vec<u8>,
}

pub fn mesh_pack(m: &Mesh) -> Vec<u8> {
    wire::pack(&MeshPack { mesh: encode_mesh(m) })
}

pub fn decode_mesh_pack(b: &[u8]) -> Option<Mesh> {
    decode_mesh(&wire::unpack::<MeshPack>(b).ok()?.mesh)
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct TerrainPack {
    x0: f32,
    z0: f32,
    spacing: f32,
    w: u32,
    h: u32,
    /// Heights in cm, delta-coded.
    #[serde(with = "wire::bytes")]
    heights: Vec<u8>,
}

pub fn terrain_pack(t: &Terrain) -> Vec<u8> {
    let mut b = Vec::new();
    let mut prev = 0i64;
    for &h in &t.heights {
        let v = q(h, POS);
        put_varint(&mut b, v - prev);
        prev = v;
    }
    wire::pack(&TerrainPack { x0: t.x0, z0: t.z0, spacing: t.spacing, w: t.w as u32, h: t.h as u32, heights: b })
}

pub fn decode_terrain_pack(b: &[u8]) -> Option<Terrain> {
    let p: TerrainPack = wire::unpack(b).ok()?;
    let (w, h) = (p.w as usize, p.h as usize);
    let mut at = 0;
    let mut prev = 0i64;
    let mut heights = Vec::with_capacity((w * h).min(1 << 24));
    for _ in 0..w * h {
        prev += get_varint(&p.heights, &mut at)?;
        heights.push(prev as f32 / POS);
    }
    Some(Terrain { x0: p.x0, z0: p.z0, spacing: p.spacing, w, h, heights })
}

/// Server asset names of the 3D data.
pub fn terrain_asset(map: &str) -> String {
    format!("t3d-{map}")
}
pub fn scene_asset(map: &str, gameplay_id: i32) -> String {
    format!("s3d-{map}-{gameplay_id}")
}
pub fn tank_asset(vehicle_type: &str) -> String {
    format!("tank-{}", vehicle_type.replace(':', "-"))
}
pub fn prefab_asset(name: &str) -> String {
    format!("pf-{}", &crate::net::sha256_hex(name.as_bytes())[..20])
}
