//! Geometry from the client: tank part meshes (`.primitives_processed`) and terrain heights
//! (`terrain2/heights` in each `.cdata_processed` chunk), plus a vertex-clustering simplifier
//! so the 3D view only ever keeps a few hundred triangles per part.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// Optional per-vertex colour (baked from the diffuse texture for map objects).
    pub colors: Vec<[u8; 3]>,
    /// Optional per-corner texture coordinates (len = indices.len()).
    pub corner_uv: Vec<[f32; 2]>,
    /// Optional per-triangle texture id (len = indices.len() / 3).
    pub tri_tex: Vec<u16>,
}

impl Mesh {
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for p in &self.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        (mn, mx)
    }

    /// Vertex clustering: snap vertices to a grid of `cell` metres, merge, drop collapsed
    /// and duplicate triangles. Keeps the overall silhouette at a tiny fraction of the size.
    pub fn simplify(&self, cell: f32) -> Mesh {
        let mut map: HashMap<(i32, i32, i32), u32> = HashMap::new();
        let mut sums: Vec<([f32; 3], u32)> = Vec::new();
        let remap: Vec<u32> = self
            .positions
            .iter()
            .map(|p| {
                let key = ((p[0] / cell).round() as i32, (p[1] / cell).round() as i32, (p[2] / cell).round() as i32);
                *map.entry(key).or_insert_with(|| {
                    sums.push(([0.0; 3], 0));
                    sums.len() as u32 - 1
                })
            })
            .collect();
        for (i, p) in self.positions.iter().enumerate() {
            let s = &mut sums[remap[i] as usize];
            for k in 0..3 {
                s.0[k] += p[k];
            }
            s.1 += 1;
        }
        let positions: Vec<[f32; 3]> = sums.iter().map(|(s, n)| [s[0] / *n as f32, s[1] / *n as f32, s[2] / *n as f32]).collect();
        let colors = if self.colors.len() == self.positions.len() {
            let mut acc = vec![[0u32; 4]; positions.len()];
            for (i, c) in self.colors.iter().enumerate() {
                let a = &mut acc[remap[i] as usize];
                a[0] += c[0] as u32;
                a[1] += c[1] as u32;
                a[2] += c[2] as u32;
                a[3] += 1;
            }
            acc.iter().map(|a| { let n = a[3].max(1); [(a[0] / n) as u8, (a[1] / n) as u8, (a[2] / n) as u8] }).collect()
        } else {
            Vec::new()
        };
        let mut seen = std::collections::HashSet::new();
        let mut indices = Vec::new();
        let textured = self.corner_uv.len() == self.indices.len() && self.tri_tex.len() * 3 == self.indices.len();
        let (mut corner_uv, mut tri_tex) = (Vec::new(), Vec::new());
        for (ti, t) in self.indices.chunks_exact(3).enumerate() {
            let (Some(&a), Some(&b), Some(&c)) = (remap.get(t[0] as usize), remap.get(t[1] as usize), remap.get(t[2] as usize)) else {
                continue;
            };
            if a == b || b == c || a == c {
                continue;
            }
            let mut key = [a, b, c];
            key.sort();
            if seen.insert(key) {
                indices.extend_from_slice(&[a, b, c]);
                if textured {
                    corner_uv.extend_from_slice(&self.corner_uv[ti * 3..ti * 3 + 3]);
                    tri_tex.push(self.tri_tex[ti]);
                }
            }
        }
        Mesh { positions, indices, colors, corner_uv, tri_tex }
    }

    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        if !self.colors.is_empty() || (self.positions.is_empty() && !other.colors.is_empty()) {
            self.colors.resize(self.positions.len(), [160, 160, 160]);
            let mut oc = other.colors.clone();
            oc.resize(other.positions.len(), [160, 160, 160]);
            self.colors.extend_from_slice(&oc);
        }
        self.positions.extend_from_slice(&other.positions);
        self.indices.extend(other.indices.iter().map(|i| i + base));
        if other.corner_uv.len() == other.indices.len() {
            self.corner_uv.extend_from_slice(&other.corner_uv);
            self.tri_tex.extend_from_slice(&other.tri_tex);
        }
    }
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Parse a `.primitives_processed` file: every `*indices` + matching `*vertices` section pair
/// becomes part of one mesh (positions only).
pub fn parse_primitives(b: &[u8]) -> Option<Mesh> {
    let table_len = u32_at(b, b.len().checked_sub(4)?)? as usize;
    let table = b.get(b.len().checked_sub(4 + table_len)?..b.len() - 4)?;
    let mut sections: Vec<(String, usize, usize)> = Vec::new();
    let (mut p, mut off) = (0usize, 4usize);
    while p + 24 <= table.len() {
        let size = u32_at(table, p)? as usize;
        let name_len = u32_at(table, p + 20)? as usize;
        let name = String::from_utf8_lossy(table.get(p + 24..p + 24 + name_len)?).into_owned();
        p = (p + 24 + name_len + 3) & !3;
        sections.push((name, off, size));
        off = (off + size + 3) & !3;
    }
    let mut mesh = Mesh::default();
    for (name, io, is) in sections.iter().filter(|s| s.0.ends_with("indices")) {
        let prefix = &name[..name.len() - "indices".len()];
        let Some((_, vo, vs)) = sections.iter().find(|s| s.0 == format!("{prefix}vertices")) else { continue };
        let vb = b.get(*vo..vo + vs)?;
        // 'BPVT' + 64-byte format + 64-byte format + u32 count
        let (header, count) = if vb.starts_with(b"BPVT") { (136, u32_at(vb, 132)? as usize) } else { (68, u32_at(vb, 64)? as usize) };
        if count == 0 {
            continue;
        }
        let stride = (vs - header) / count;
        if stride < 12 {
            continue;
        }
        let ib = b.get(*io..io + is)?;
        let fmt = String::from_utf8_lossy(&ib[..64]);
        let wide = fmt.trim_end_matches('\0').starts_with("list32");
        let n_idx = u32_at(ib, 64)? as usize;
        let base = mesh.positions.len() as u32;
        for i in 0..count {
            let o = header + i * stride;
            let f = |k: usize| f32::from_le_bytes(vb[o + 4 * k..o + 4 * k + 4].try_into().unwrap());
            mesh.positions.push([f(0), f(1), f(2)]);
        }
        for i in 0..n_idx {
            let idx = if wide {
                u32_at(ib, 72 + 4 * i)?
            } else {
                u16::from_le_bytes(ib.get(72 + 2 * i..74 + 2 * i)?.try_into().ok()?) as u32
            };
            mesh.indices.push(base + idx);
        }
    }
    (!mesh.indices.is_empty()).then_some(mesh)
}

/// One vertex/index section pair of a primitives file, with UVs and primitive groups
/// (start index, primitive count) as referenced by the `.visual` materials.
pub struct Section {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub groups: Vec<(usize, usize)>,
}

/// All sections of a primitives file, keyed by name prefix (e.g. "" or "gun_07Shape.").
pub fn parse_sections(b: &[u8]) -> Option<HashMap<String, Section>> {
    let table_len = u32_at(b, b.len().checked_sub(4)?)? as usize;
    let table = b.get(b.len().checked_sub(4 + table_len)?..b.len() - 4)?;
    let mut raw: Vec<(String, usize, usize)> = Vec::new();
    let (mut p, mut off) = (0usize, 4usize);
    while p + 24 <= table.len() {
        let size = u32_at(table, p)? as usize;
        let name_len = u32_at(table, p + 20)? as usize;
        let name = String::from_utf8_lossy(table.get(p + 24..p + 24 + name_len)?).into_owned();
        p = (p + 24 + name_len + 3) & !3;
        raw.push((name, off, size));
        off = (off + size + 3) & !3;
    }
    let mut out = HashMap::new();
    for (name, io, is) in raw.iter().filter(|s| s.0.ends_with("indices")) {
        let prefix = name[..name.len() - "indices".len()].to_string();
        let Some((_, vo, vs)) = raw.iter().find(|s| s.0 == format!("{prefix}vertices")) else { continue };
        let vb = b.get(*vo..vo + vs)?;
        let (header, count, fmt) = if vb.starts_with(b"BPVT") {
            (136, u32_at(vb, 132)? as usize, String::from_utf8_lossy(&vb[4..68]).trim_end_matches('\0').to_string())
        } else {
            (68, u32_at(vb, 64)? as usize, String::from_utf8_lossy(&vb[..64]).trim_end_matches('\0').to_string())
        };
        if count == 0 {
            continue;
        }
        let stride = (vs - header) / count;
        // Packed normals ("...tb" formats) take 4 bytes, plain "xyznuv" uses 3 floats.
        let uv_off = if fmt.contains("tb") || fmt.contains("iii") { 16 } else { 24 };
        let ib = b.get(*io..io + is)?;
        let wide = String::from_utf8_lossy(&ib[..64]).trim_end_matches('\0').starts_with("list32");
        let n_idx = u32_at(ib, 64)? as usize;
        let n_groups = u32_at(ib, 68)? as usize;
        let mut sec = Section { positions: Vec::with_capacity(count), uvs: Vec::with_capacity(count), indices: Vec::with_capacity(n_idx), groups: Vec::new() };
        for i in 0..count {
            let o = header + i * stride;
            let f = |k: usize| f32::from_le_bytes(vb[o + k..o + k + 4].try_into().unwrap());
            sec.positions.push([f(0), f(4), f(8)]);
            sec.uvs.push(if stride >= uv_off + 8 { [f(uv_off), f(uv_off + 4)] } else { [0.0, 0.0] });
        }
        let isz = if wide { 4 } else { 2 };
        for i in 0..n_idx {
            sec.indices.push(if wide { u32_at(ib, 72 + 4 * i)? } else { u16::from_le_bytes(ib.get(72 + 2 * i..74 + 2 * i)?.try_into().ok()?) as u32 });
        }
        let g0 = 72 + n_idx * isz;
        for g in 0..n_groups {
            let (si, np) = (u32_at(ib, g0 + g * 16)? as usize, u32_at(ib, g0 + g * 16 + 4)? as usize);
            sec.groups.push((si, np));
        }
        out.insert(prefix, sec);
    }
    Some(out)
}

/// Minimal PNG decoder (8-bit, non-interlaced): gray, gray+alpha, RGB or RGBA -> RGBA.
pub fn png_image(png: &[u8]) -> Option<super::dds::Image> {
    let (w, h, bpp, raw) = png_raw(png)?;
    let rgba: Vec<u8> = match bpp {
        4 => raw,
        3 => raw.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        2 => raw.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        _ => raw.iter().flat_map(|&g| [g, g, g, 255]).collect(),
    };
    Some(super::dds::Image { width: w, height: h, rgba })
}

/// Terrain heights are 8-bit RGBA PNGs.
fn png_rgba8(png: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let (w, h, bpp, raw) = png_raw(png)?;
    (bpp == 4).then_some((w, h, raw))
}

fn png_raw(png: &[u8]) -> Option<(usize, usize, usize, Vec<u8>)> {
    use std::io::Read;
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let be = |o: usize| -> Option<usize> { Some(u32::from_be_bytes(png.get(o..o + 4)?.try_into().ok()?) as usize) };
    let (w, h) = (be(16)?, be(20)?);
    let bpp = match png[25] {
        0 => 1,
        4 => 2,
        2 => 3,
        6 => 4,
        _ => return None,
    };
    if png[24] != 8 || png[28] != 0 {
        return None;
    }
    let mut idat = Vec::new();
    let mut p = 8;
    while p + 8 <= png.len() {
        let len = be(p)?;
        if &png[p + 4..p + 8] == b"IDAT" {
            idat.extend_from_slice(png.get(p + 8..p + 8 + len)?);
        }
        p += 12 + len;
    }
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&idat[..]).read_to_end(&mut raw).ok()?;
    let stride = w * bpp;
    let mut out = vec![0u8; stride * h];
    for y in 0..h {
        let f = *raw.get(y * (stride + 1))?;
        let src = raw.get(y * (stride + 1) + 1..(y + 1) * (stride + 1))?;
        for i in 0..stride {
            let a = if i >= bpp { out[y * stride + i - bpp] as i32 } else { 0 };
            let up = if y > 0 { out[(y - 1) * stride + i] as i32 } else { 0 };
            let c = if y > 0 && i >= bpp { out[(y - 1) * stride + i - bpp] as i32 } else { 0 };
            let pred = match f {
                1 => a,
                2 => up,
                3 => (a + up) / 2,
                4 => {
                    let pp = a + up - c;
                    let (pa, pb, pc) = ((pp - a).abs(), (pp - up).abs(), (pp - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        up
                    } else {
                        c
                    }
                }
                _ => 0,
            };
            out[y * stride + i] = (src[i] as i32 + pred) as u8;
        }
    }
    Some((w, h, bpp, out))
}

/// Heights of one 100 m terrain chunk in metres, row-major, rows = +z, columns = +x,
/// with the 2-sample border already removed (so `n` samples span exactly 100 m).
pub fn parse_chunk_heights(cdata: &[u8]) -> Option<(usize, Vec<f32>)> {
    let zip = super::pkg::Pkg::from_bytes(cdata)?;
    let b = zip.read("terrain2/heights")?;
    if b.get(..3)? != b"hmp" {
        return None;
    }
    let png_at = b.windows(4).position(|w| w == b"\x89PNG")?;
    let (w, h, px) = png_rgba8(&b[png_at..])?;
    let n = w.min(h).checked_sub(4)?;
    let mut out = Vec::with_capacity(n * n);
    for y in 2..2 + n {
        for x in 2..2 + n {
            let o = (y * w + x) * 4;
            out.push(i32::from_le_bytes(px[o..o + 4].try_into().ok()?) as f32 / 1000.0);
        }
    }
    Some((n, out))
}
