//! `spaces/<map>/space.bin` (BigWorld "BWTB" compiled space): the placed static models
//! (buildings, bunkers, rocks, ...) and water bodies.
//!
//! Layout: header `BWTB`, version, then a table of sections (tag, version, u64 offset,
//! u64 size). Sections are sequences of arrays (u32 element size, u32 count, data).

use std::collections::HashMap;

pub struct Space {
    /// (model index, column-major 4x4 transform, game-mode visibility mask: bit N = gameplay type N)
    pub instances: Vec<(usize, [f32; 16], u32)>,
    /// Model primitives path (".primitives", without "_processed") and local bounds.
    pub models: Vec<(Option<String>, [f32; 3], [f32; 3])>,
    /// Water bodies: (min, max) boxes; the surface is at `min.y`.
    pub water: Vec<([f32; 3], [f32; 3])>,
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    u32_at(b, o).map(f32::from_bits)
}

/// The arrays of one section: (element size, count, data).
fn arrays<'a>(b: &'a [u8], off: usize, size: usize) -> Vec<(usize, usize, &'a [u8])> {
    let mut out = Vec::new();
    let mut p = off;
    while p + 8 <= off + size {
        let (Some(es), Some(n)) = (u32_at(b, p), u32_at(b, p + 4)) else { break };
        let (es, n) = (es as usize, n as usize);
        let Some(data) = es.checked_mul(n).and_then(|len| b.get(p + 8..p + 8 + len)) else { break };
        if es * n > size {
            break;
        }
        out.push((es, n, data));
        p += 8 + es * n;
    }
    out
}

pub fn parse(b: &[u8]) -> Option<Space> {
    if b.get(0..4)? != b"BWTB" {
        return None;
    }
    let n = u32_at(b, 20)? as usize;
    let mut sections: HashMap<String, (usize, usize)> = HashMap::new();
    for i in 0..n {
        let o = 24 + i * 24;
        let tag = String::from_utf8_lossy(b.get(o..o + 4)?).into_owned();
        let off = u64::from_le_bytes(b.get(o + 8..o + 16)?.try_into().ok()?) as usize;
        let size = u64::from_le_bytes(b.get(o + 16..o + 24)?.try_into().ok()?) as usize;
        sections.insert(tag, (off, size));
    }

    // String table: entries (key, offset, length), then u32 blob size + blob.
    let mut strings: HashMap<u32, String> = HashMap::new();
    if let Some(&(off, _)) = sections.get("BWST") {
        let es = u32_at(b, off)? as usize;
        let cnt = u32_at(b, off + 4)? as usize;
        let blob = off + 8 + es * cnt + 4;
        for i in 0..cnt {
            let e = off + 8 + i * es;
            let (k, so, len) = (u32_at(b, e)?, u32_at(b, e + 4)? as usize, u32_at(b, e + 8)? as usize);
            if let Some(s) = b.get(blob + so..blob + so + len) {
                strings.insert(k, String::from_utf8_lossy(s).into_owned());
            }
        }
    }

    // Models: array 2 = bounds min(3) max(3) + primitives path key.
    let mut models = Vec::new();
    if let Some(&(off, size)) = sections.get("BSMO") {
        if let Some(&(es, cnt, d)) = arrays(b, off, size).get(2) {
            for i in 0..cnt {
                let e = i * es;
                let f = |k: usize| f32_at(d, e + 4 * k).unwrap_or(0.0);
                let path = u32_at(d, e + 24).and_then(|k| strings.get(&k)).cloned();
                models.push((path, [f(0), f(1), f(2)], [f(3), f(4), f(5)]));
            }
        }
    }

    // Instances: array 0 = transforms, array 2 = game-mode mask, array 3 = (model index, flags).
    let mut instances = Vec::new();
    if let Some(&(off, size)) = sections.get("BSMI") {
        let a = arrays(b, off, size);
        if let (Some(&(tes, tcnt, td)), Some(&(mes, mcnt, md))) = (a.first(), a.get(3)) {
            let masks = a.get(2).filter(|m| m.0 == 4 && m.1 == tcnt);
            for i in 0..tcnt.min(mcnt) {
                let mut m = [0f32; 16];
                for (k, v) in m.iter_mut().enumerate() {
                    *v = f32_at(td, i * tes + 4 * k).unwrap_or(0.0);
                }
                let idx = u32_at(md, i * mes).unwrap_or(u32::MAX) as usize;
                let mask = masks.and_then(|m| u32_at(m.2, i * 4)).unwrap_or(u32::MAX);
                if idx < models.len() {
                    instances.push((idx, m, mask));
                }
            }
        }
    }

    // Water: array 0 = one entry per body, starting with its box.
    let mut water = Vec::new();
    if let Some(&(off, size)) = sections.get("BWWa") {
        if let Some(&(es, cnt, d)) = arrays(b, off, size).first() {
            for i in 0..cnt {
                let f = |k: usize| f32_at(d, i * es + 4 * k).unwrap_or(0.0);
                water.push(([f(0), f(1), f(2)], [f(3), f(4), f(5)]));
            }
        }
    }
    Some(Space { instances, models, water })
}
