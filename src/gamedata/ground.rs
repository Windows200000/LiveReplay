//! Ground texture for the 3D view, baked once per map into one large image the way the game's
//! terrain shader mixes it: every 100 m chunk has blend images (`terrain2/blend_textures`, DXT5,
//! 128x128, the first listed texture's weight in alpha and the second's in green) and per-texture
//! world projections (`terrain2/layers`). Each pixel = the chunk's ground textures, each sampled
//! with its own projection, mixed by their interpolated weights. The client's overview colour
//! (`global_AM`) only fills where a chunk has no data. Stored DXT1-compressed in the 3D cache.

use std::collections::HashMap;

use super::dds::Image;
use super::pkg::Pkg;
use super::GameData;

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Texture projection: u = dot((x, y, z, 1), u), v = dot((x, y, z, 1), v).
type Projection = ([f32; 4], [f32; 4]);

/// Per-texture projections from `terrain2/layers` ("bld" records: size, then u and v vectors,
/// ..., then the texture name).
fn parse_layers(b: &[u8], out: &mut HashMap<String, Projection>) {
    let mut p = 0;
    while let Some(rel) = b.get(p..).and_then(|s| s.windows(4).position(|w| w == b"bld\0")) {
        p += rel;
        let vec4 = |o: usize| -> Option<[f32; 4]> { Some([f32_at(b, o)?, f32_at(b, o + 4)?, f32_at(b, o + 8)?, f32_at(b, o + 12)?]) };
        let (Some(u), Some(v)) = (vec4(p + 16), vec4(p + 32)) else { return };
        if let Some(start) = b.get(p + 48..).and_then(|t| t.windows(5).position(|w| w == b"maps/")) {
            let s = p + 48 + start;
            let name: Vec<u8> = b[s..].iter().take_while(|&&c| (0x20..0x7f).contains(&c)).copied().collect();
            let ok = |a: &[f32; 4]| a.iter().all(|x| x.is_finite()) && a[..3].iter().any(|x| x.abs() > 1e-4);
            if ok(&u) && ok(&v) {
                out.insert(String::from_utf8_lossy(&name).to_lowercase(), (u, v));
            }
        }
        p += 4;
    }
}

/// One chunk's ground: texture name + its weight map (n x n, rows = +z) per layer.
struct ChunkBlend {
    n: usize,
    layers: Vec<(String, Vec<u8>)>,
}

/// `terrain2/blend_textures`: "bwb" header, then "bwt" records: version, width, height (u16),
/// format, texture count (u16), padding, the texture names (u32 length + text), DXT5 blocks.
fn parse_blend(b: &[u8]) -> Option<ChunkBlend> {
    if b.get(0..4)? != b"bwb\0" {
        return None;
    }
    let mut p = 24;
    let mut layers = Vec::new();
    let mut n = 0;
    while b.get(p..p + 4) == Some(b"bwt\0") {
        let u16_at = |o: usize| -> Option<usize> { Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize) };
        let (w, h, count) = (u16_at(p + 8)?, u16_at(p + 10)?, u16_at(p + 14)?);
        let mut q = p + 24;
        let mut names = Vec::new();
        for _ in 0..count {
            let len = u32_at(b, q)? as usize;
            names.push(String::from_utf8_lossy(b.get(q + 4..q + 4 + len)?).to_lowercase());
            q += 4 + len;
        }
        let size = w.div_ceil(4) * h.div_ceil(4) * 16;
        let img = super::dds::decode(&dds_header_dxt5(w, h, b.get(q..q + size)?))?;
        // First texture's weight in alpha, second's in green (the precise DXT5 channels).
        for (k, name) in names.into_iter().enumerate().take(2) {
            let ch = if k == 0 { 3 } else { 1 };
            layers.push((name, img.rgba.chunks_exact(4).map(|px| px[ch]).collect()));
        }
        n = w;
        p = q + size;
    }
    (!layers.is_empty() && n > 0).then_some(ChunkBlend { n, layers })
}

fn dds_header_dxt5(w: usize, h: usize, data: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 128];
    d[0..4].copy_from_slice(b"DDS ");
    d[4..8].copy_from_slice(&124u32.to_le_bytes());
    d[12..16].copy_from_slice(&(h as u32).to_le_bytes());
    d[16..20].copy_from_slice(&(w as u32).to_le_bytes());
    d[28..32].copy_from_slice(&1u32.to_le_bytes());
    d[76..80].copy_from_slice(&32u32.to_le_bytes());
    d[80..84].copy_from_slice(&4u32.to_le_bytes());
    d[84..88].copy_from_slice(b"DXT5");
    d.extend_from_slice(data);
    d
}

/// A ground texture with its projection and average colour.
struct Layer {
    img: Image,
    proj: Projection,
    mean: [f32; 3],
}

impl Layer {
    /// Bilinear, wrapping texel lookup.
    fn texel(&self, tu: f32, tv: f32) -> [f32; 3] {
        let (w, h) = (self.img.width as i64, self.img.height as i64);
        let (fx, fy) = (tu * w as f32 - 0.5, tv * h as f32 - 0.5);
        let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let at = |x: i64, y: i64, k: usize| self.img.rgba[((y.rem_euclid(h) * w + x.rem_euclid(w)) * 4) as usize + k] as f32;
        [0, 1, 2].map(|k| (at(ix, iy, k) * (1.0 - tx) + at(ix + 1, iy, k) * tx) * (1.0 - ty) + (at(ix, iy + 1, k) * (1.0 - tx) + at(ix + 1, iy + 1, k) * tx) * ty)
    }

    /// Texture colour at a world position. Two lookups at different scale and rotation are
    /// averaged so the tiling doesn't show as a grid.
    fn sample(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let (u, v) = self.proj;
        let tu = u[0] * x + u[1] * y + u[2] * z + u[3];
        let tv = v[0] * x + v[1] * y + v[2] * z + v[3];
        let a = self.texel(tu, tv);
        let (s, c) = (0.62f32, 0.78f32);
        let b = self.texel((c * tu - s * tv) * 0.41 + 0.37, (s * tu + c * tv) * 0.41 + 0.71);
        [0, 1, 2].map(|k| (a[k] + b[k]) * 0.5)
    }
}

/// Bake the ground for the world rectangle [x0, z0, x1, z1] at `px_per_m` (rows north -> south).
/// `base` (the overview colour covering exactly that rectangle) fills gaps; `height` gives the
/// ground height for projections that use it.
pub fn bake(gd: &GameData, map: &str, rect: [f32; 4], px_per_m: f32, base: &Image, find: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<Image> {
    let pkg = Pkg::open(&gd.root.join("res").join("packages").join(format!("{map}.pkg"))).ok()?;
    let [x0, z0, x1, z1] = rect;
    let (cx0, cx1) = ((x0 / 100.0).floor() as i32, ((x1 - 0.01) / 100.0).floor() as i32);
    let (cz0, cz1) = ((z0 / 100.0).floor() as i32, ((z1 - 0.01) / 100.0).floor() as i32);
    let mut chunks: HashMap<(i32, i32), ChunkBlend> = HashMap::new();
    let mut projections: HashMap<String, Projection> = HashMap::new();
    for cx in cx0..=cx1 {
        for cz in cz0..=cz1 {
            let name = format!("spaces/{map}/{:04x}{:04x}o.cdata_processed", cx as u16, cz as u16);
            let Some(zip) = pkg.read(&name).and_then(|b| Pkg::from_bytes(&b)) else { continue };
            if let Some(l) = zip.read("terrain2/layers") {
                parse_layers(&l, &mut projections);
            }
            if let Some(c) = zip.read("terrain2/blend_textures").and_then(|b| parse_blend(&b)) {
                chunks.insert((cx, cz), c);
            }
        }
    }
    let mut layers: HashMap<String, Option<Layer>> = HashMap::new();
    for c in chunks.values() {
        for (n, _) in &c.layers {
            layers.entry(n.clone()).or_insert_with(|| {
                let b = find(n)?;
                let w = u32::from_le_bytes(b.get(16..20)?.try_into().ok()?) as usize;
                let img = super::dds::decode_level(&b, (w / 256).max(1).trailing_zeros() as usize)?;
                let proj = projections.get(n).copied().unwrap_or(([0.333, 0.0, 0.0, 0.0], [0.0, 0.0, 0.333, 0.0]));
                let px = (img.rgba.len() / 4).max(1) as f32;
                let mean = [0, 1, 2].map(|k| img.rgba.chunks_exact(4).map(|p| p[k] as f32).sum::<f32>() / px);
                Some(Layer { img, proj, mean })
            });
        }
    }

    let w = ((x1 - x0) * px_per_m).round() as usize;
    let h = ((z1 - z0) * px_per_m).round() as usize;
    if w == 0 || h == 0 || base.width == 0 {
        return None;
    }
    let base_at = |u: f32, v: f32| -> [f32; 3] {
        let fx = (u * base.width as f32 - 0.5).clamp(0.0, (base.width - 1) as f32);
        let fy = (v * base.height as f32 - 0.5).clamp(0.0, (base.height - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (ix1, iy1) = ((ix + 1).min(base.width - 1), (iy + 1).min(base.height - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let at = |x: usize, y: usize, k: usize| base.rgba[(y * base.width + x) * 4 + k] as f32;
        [0, 1, 2].map(|k| (at(ix, iy, k) * (1.0 - tx) + at(ix1, iy, k) * tx) * (1.0 - ty) + (at(ix, iy1, k) * (1.0 - tx) + at(ix1, iy1, k) * tx) * ty)
    };
    // Bilinear weight of every layer of the chunk at a world position.
    let mix = |wx: f32, wz: f32| -> Option<([f32; 3], [f32; 3])> {
        let (cx, cz) = ((wx / 100.0).floor() as i32, (wz / 100.0).floor() as i32);
        let c = chunks.get(&(cx, cz))?;
        let n = c.n as f32;
        let fx = ((wx - cx as f32 * 100.0) / 100.0 * n - 0.5).clamp(0.0, n - 1.0);
        let fz = ((wz - cz as f32 * 100.0) / 100.0 * n - 0.5).clamp(0.0, n - 1.0);
        let (ix, iz) = (fx as usize, fz as usize);
        let (ix1, iz1) = ((ix + 1).min(c.n - 1), (iz + 1).min(c.n - 1));
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let (mut acc, mut avg, mut sum) = ([0.0f32; 3], [0.0f32; 3], 0.0f32);
        for (name, wm) in &c.layers {
            let at = |x: usize, z: usize| wm[z * c.n + x] as f32;
            let wgt = (at(ix, iz) * (1.0 - tx) + at(ix1, iz) * tx) * (1.0 - tz) + (at(ix, iz1) * (1.0 - tx) + at(ix1, iz1) * tx) * tz;
            if wgt < 1.0 {
                continue;
            }
            let Some(Some(layer)) = layers.get(name) else { continue };
            let col = layer.sample(wx, 0.0, wz);
            for k in 0..3 {
                acc[k] += col[k] * wgt;
                avg[k] += layer.mean[k] * wgt;
            }
            sum += wgt;
        }
        (sum > 0.0).then(|| (acc.map(|a| a / sum), avg.map(|a| a / sum)))
    };

    let mut rgba = vec![0u8; w * h * 4];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let rows_per = h.div_ceil(threads);
    std::thread::scope(|s| {
        for (ti, part) in rgba.chunks_mut(rows_per * w * 4).enumerate() {
            let (mix, base_at) = (&mix, &base_at);
            s.spawn(move || {
                for (r, row) in part.chunks_mut(w * 4).enumerate() {
                    let py = ti * rows_per + r;
                    let wz = z1 - (py as f32 + 0.5) / px_per_m;
                    for px in 0..w {
                        let wx = x0 + (px as f32 + 0.5) / px_per_m;
                        // Detail from the ground textures, large-scale colour from the overview image.
                        let overview = base_at((px as f32 + 0.5) / w as f32, (py as f32 + 0.5) / h as f32);
                        let c = match mix(wx, wz) {
                            Some((detail, avg)) => [0, 1, 2].map(|k| detail[k] * (overview[k] / avg[k].max(8.0)).clamp(0.35, 2.5)),
                            None => overview,
                        };
                        let o = px * 4;
                        for i in 0..3 {
                            row[o + i] = c[i].clamp(0.0, 255.0) as u8;
                        }
                        row[o + 3] = 255;
                    }
                }
            });
        }
    });
    Some(Image { width: w, height: h, rgba })
}

/// Simple DXT1 encoder (endpoints = per-block min/max luminance colours).
pub fn encode_dxt1(img: &Image) -> Vec<u8> {
    let (bw, bh) = (img.width.div_ceil(4), img.height.div_ceil(4));
    let mut out = Vec::with_capacity(bw * bh * 8);
    let to565 = |c: [u8; 3]| -> u16 { ((c[0] as u16 >> 3) << 11) | ((c[1] as u16 >> 2) << 5) | (c[2] as u16 >> 3) };
    for by in 0..bh {
        for bx in 0..bw {
            let mut px = [[0u8; 3]; 16];
            for (i, p) in px.iter_mut().enumerate() {
                let (x, y) = ((bx * 4 + i % 4).min(img.width - 1), (by * 4 + i / 4).min(img.height - 1));
                let o = (y * img.width + x) * 4;
                *p = [img.rgba[o], img.rgba[o + 1], img.rgba[o + 2]];
            }
            let lum = |c: &[u8; 3]| c[0] as u32 * 3 + c[1] as u32 * 6 + c[2] as u32;
            let (mut lo, mut hi) = (px[0], px[0]);
            for c in &px {
                if lum(c) < lum(&lo) {
                    lo = *c;
                }
                if lum(c) > lum(&hi) {
                    hi = *c;
                }
            }
            let (mut c0, mut c1) = (to565(hi), to565(lo));
            if c0 < c1 {
                std::mem::swap(&mut c0, &mut c1);
            }
            let dec = |c: u16| -> [i32; 3] { [((c >> 11) & 31) as i32 * 255 / 31, ((c >> 5) & 63) as i32 * 255 / 63, (c & 31) as i32 * 255 / 31] };
            let (a, b) = (dec(c0), dec(c1));
            let pal = [a, b, [(2 * a[0] + b[0]) / 3, (2 * a[1] + b[1]) / 3, (2 * a[2] + b[2]) / 3], [(a[0] + 2 * b[0]) / 3, (a[1] + 2 * b[1]) / 3, (a[2] + 2 * b[2]) / 3]];
            let mut bits = 0u32;
            for (i, c) in px.iter().enumerate() {
                let best = (0..4).min_by_key(|&k| (0..3).map(|j| (pal[k][j] - c[j] as i32).pow(2)).sum::<i32>()).unwrap_or(0);
                // index 0/1 = c0/c1, 2/3 = interpolants (only valid when c0 > c1)
                let best = if c0 == c1 { 0 } else { best };
                bits |= (best as u32) << (2 * i);
            }
            out.extend_from_slice(&c0.to_le_bytes());
            out.extend_from_slice(&c1.to_le_bytes());
            out.extend_from_slice(&bits.to_le_bytes());
        }
    }
    out
}

/// DDS (DXT1) with a full mip chain, ready for compressed upload to the GPU.
pub fn dds_dxt1_mips(img: &Image) -> Vec<u8> {
    let mut levels = vec![img.clone()];
    while levels.last().is_some_and(|l| l.width > 4 && l.height > 4) {
        let p = levels.last().unwrap();
        let (w, h) = ((p.width / 2).max(1), (p.height / 2).max(1));
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                for k in 0..4 {
                    let s = |dx: usize, dy: usize| p.rgba[((2 * y + dy).min(p.height - 1) * p.width + (2 * x + dx).min(p.width - 1)) * 4 + k] as u32;
                    rgba[(y * w + x) * 4 + k] = ((s(0, 0) + s(1, 0) + s(0, 1) + s(1, 1)) / 4) as u8;
                }
            }
        }
        levels.push(Image { width: w, height: h, rgba });
    }
    let mut out = dds_header(img, levels.len());
    for l in &levels {
        out.extend(encode_dxt1(l));
    }
    out
}

fn dds_header(img: &Image, mips: usize) -> Vec<u8> {
    let mut h = vec![0u8; 128];
    h[0..4].copy_from_slice(b"DDS ");
    h[4..8].copy_from_slice(&124u32.to_le_bytes());
    h[12..16].copy_from_slice(&(img.height as u32).to_le_bytes());
    h[16..20].copy_from_slice(&(img.width as u32).to_le_bytes());
    h[28..32].copy_from_slice(&(mips as u32).to_le_bytes());
    h[76..80].copy_from_slice(&32u32.to_le_bytes());
    h[80..84].copy_from_slice(&4u32.to_le_bytes());
    h[84..88].copy_from_slice(b"DXT1");
    h
}

/// Wrap DXT1 blocks into a minimal DDS so the normal decoder reads it back.
pub fn dds_dxt1(img: &Image) -> Vec<u8> {
    let mut h = vec![0u8; 128];
    h[0..4].copy_from_slice(b"DDS ");
    h[4..8].copy_from_slice(&124u32.to_le_bytes());
    h[12..16].copy_from_slice(&(img.height as u32).to_le_bytes());
    h[16..20].copy_from_slice(&(img.width as u32).to_le_bytes());
    h[28..32].copy_from_slice(&1u32.to_le_bytes());
    h[76..80].copy_from_slice(&32u32.to_le_bytes());
    h[80..84].copy_from_slice(&4u32.to_le_bytes());
    h[84..88].copy_from_slice(b"DXT1");
    h.extend(encode_dxt1(img));
    h
}
