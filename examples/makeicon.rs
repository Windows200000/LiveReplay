//! Developer tool: draws the LiveReplay icon and writes `assets/app.ico` (Windows exe),
//! `assets/icon.png` (window icon) and `web/favicon.png`. `cargo run --example makeicon`
use std::io::Write;

/// Coverage-based drawing at `n` x `n`, 4x4 supersampled.
fn render(n: usize) -> Vec<u8> {
    let ss = 4;
    let mut out = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (px as f32 + (sx as f32 + 0.5) / ss as f32) / n as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / ss as f32) / n as f32;
                    let c = shade(x, y, n);
                    // premultiplied accumulate
                    for k in 0..3 {
                        acc[k] += c[k] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let m = (ss * ss) as f32;
            let a = acc[3] / m;
            let i = (py * n + px) * 4;
            for k in 0..3 {
                out[i + k] = if a > 0.0 { (acc[k] / acc[3] * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
            }
            out[i + 3] = (a * 255.0).round() as u8;
        }
    }
    out
}

fn over(dst: [f32; 4], src: [f32; 3], a: f32) -> [f32; 4] {
    if a <= 0.0 {
        return dst;
    }
    let oa = a + dst[3] * (1.0 - a);
    let mut o = [0.0, 0.0, 0.0, oa];
    for k in 0..3 {
        o[k] = (src[k] * a + dst[k] * dst[3] * (1.0 - a)) / oa.max(1e-6);
    }
    o
}

fn rgb(h: u32) -> [f32; 3] {
    [((h >> 16) & 255) as f32 / 255.0, ((h >> 8) & 255) as f32 / 255.0, (h & 255) as f32 / 255.0]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Signed distance to a convex polygon (negative inside; approximate outside the corners).
fn poly_sd(x: f32, y: f32, pts: &[(f32, f32)]) -> f32 {
    let (cx, cy) = (pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32, pts.iter().map(|p| p.1).sum::<f32>() / pts.len() as f32);
    let mut d = f32::MIN;
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        let (ex, ey) = (q.0 - p.0, q.1 - p.1);
        let l = (ex * ex + ey * ey).sqrt();
        let (nx, ny) = (ey / l, -ex / l);
        let s = if (cx - p.0) * nx + (cy - p.1) * ny > 0.0 { -1.0 } else { 1.0 };
        d = d.max(s * ((x - p.0) * nx + (y - p.1) * ny));
    }
    d
}

/// Shield and emblem outlines, measured on a 900 px drawing (shield 254..646 x 177..720),
/// mapped into the icon (0.80 tall, centred at x 0.47).
const S: f32 = 0.80 / 543.0;
fn m(p: (f32, f32)) -> (f32, f32) {
    (0.47 + (p.0 - 450.0) * S, 0.07 + (p.1 - 177.0) * S)
}
fn sd_px(x: f32, y: f32, pts: &[(f32, f32)]) -> f32 {
    let v: Vec<(f32, f32)> = pts.iter().map(|p| m(*p)).collect();
    poly_sd(x, y, &v)
}
const OUTER: [(f32, f32); 7] = [(356.0, 177.0), (544.0, 177.0), (646.0, 279.0), (646.0, 524.0), (450.0, 720.0), (254.0, 524.0), (254.0, 279.0)];
/// Width of the shield's rim.
const RIM: f32 = 21.5 * S;
const BAR: [(f32, f32); 4] = [(376.0, 218.0), (524.0, 218.0), (602.0, 297.0), (298.0, 297.0)];
const STEM: [(f32, f32); 5] = [(411.0, 297.0), (489.0, 297.0), (489.0, 623.0), (450.0, 662.0), (411.0, 623.0)];
// Wings: a rectangle plus a convex lower piece each (the notch makes the whole wing concave).
const WING_L: [(f32, f32); 4] = [(297.0, 331.0), (373.0, 331.0), (373.0, 476.0), (297.0, 476.0)];
const WING_L_LOW: [(f32, f32); 5] = [(297.0, 476.0), (373.0, 476.0), (409.0, 512.0), (355.0, 567.0), (297.0, 509.0)];
const WING_R: [(f32, f32); 4] = [(527.0, 331.0), (603.0, 331.0), (603.0, 476.0), (527.0, 476.0)];
const WING_R_LOW: [(f32, f32); 5] = [(527.0, 476.0), (603.0, 476.0), (603.0, 509.0), (545.0, 567.0), (491.0, 512.0)];

fn shield_sd(x: f32, y: f32) -> f32 {
    sd_px(x, y, &OUTER)
}

fn emblem_sd(x: f32, y: f32) -> f32 {
    let wings = sd_px(x, y, &WING_L).min(sd_px(x, y, &WING_L_LOW)).min(sd_px(x, y, &WING_R)).min(sd_px(x, y, &WING_R_LOW));
    sd_px(x, y, &BAR).min(sd_px(x, y, &STEM)).min(wings)
}

fn metal(x: f32, y: f32) -> [f32; 3] {
    let t = (y - 0.09) / 0.77 * 0.75 + (x - 0.17) * 0.35;
    let base = lerp3(rgb(0xf4f5f7), rgb(0x6f7680), t);
    // A soft diagonal sheen.
    let sheen = (((x + y) - 0.75).abs() < 0.06) as i32 as f32 * 0.12;
    lerp3(base, [1.0, 1.0, 1.0], sheen)
}

const GLYPHS: [[u8; 7]; 4] = [
    [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111], // L
    [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // I
    [0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b01010, 0b00100], // V
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111], // E
];

/// Colour + alpha of the icon at (x, y) in 0..1.
fn shade(x: f32, y: f32, n: usize) -> [f32; 4] {
    let mut c = [0.0f32; 4];
    // Film strip behind the shield, from bottom left to top right with a gentle wave.
    let (ax, ay, bx, by) = (0.0f32, 0.78f32, 1.0f32, 0.30f32);
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt();
    let u = ((x - ax) * dx + (y - ay) * dy) / len;
    let v = ((x - ax) * dy - (y - ay) * dx) / len - 0.035 * (u * std::f32::consts::TAU / len).sin();
    let half = 0.115;
    if v.abs() < half && x > 0.01 && x < 0.99 {
        let mut col = rgb(0x24262a);
        if v.abs() > 0.072 {
            col = rgb(0x3b3e44);
            // Sprocket holes.
            if n >= 32 && v.abs() > 0.083 && v.abs() < 0.103 && (u / 0.052).fract() > 0.3 && (u / 0.052).fract() < 0.75 {
                col = rgb(0xd4d7dc);
            }
        } else if n >= 48 && (u / 0.17).fract() < 0.035 {
            // Frame separators.
            col = rgb(0x4a4e55);
        }
        c = over(c, col, 1.0);
        // Rim light on the strip's upper edge.
        if v > half - 0.012 {
            c = over(c, rgb(0x8a8f97), 0.8);
        }
    }
    // Shield shadow.
    if shield_sd(x - 0.018, y - 0.026) < 0.0 {
        c = over(c, [0.0, 0.0, 0.0], 0.45);
    }
    let sd = shield_sd(x, y);
    let bevel = |c: [f32; 4], d: f32| {
        // Lighter on the top-left edges, darker bottom-right: a pressed-metal look.
        if d > -0.011 {
            let light = if x + y < 0.95 { [1.0, 1.0, 1.0] } else { [0.12, 0.13, 0.15] };
            over(c, light, 0.32)
        } else {
            c
        }
    };
    if sd < 0.016 {
        c = over(c, rgb(0x15171a), 1.0); // outline
    }
    if sd < 0.0 {
        c = bevel(over(c, metal(x, y), 1.0), sd);
    }
    // Dark field inside the rim.
    if sd < -RIM {
        let field = lerp3(rgb(0x30343b), rgb(0x101214), (y - 0.12) / 0.7);
        c = over(c, field, 1.0);
        // Inner edge of the rim casts a little shade.
        if sd > -RIM - 0.01 {
            c = over(c, [0.0, 0.0, 0.0], 0.35);
        }
    }
    // The emblem.
    let e = emblem_sd(x, y);
    if e < 0.0 {
        c = bevel(over(c, metal(x, y), 1.0), e);
    }
    // LIVE badge.
    let (px0, px1, py0, py1) = (0.56, 0.98, 0.745, 0.915);
    let r = (py1 - py0) / 2.0;
    let pill = |grow: f32| {
        let cx = x.clamp(px0 + r, px1 - r);
        let cy = (py0 + py1) / 2.0;
        ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < r + grow
    };
    if pill(0.022) {
        c = over(c, rgb(0x15171a), 1.0);
    }
    if pill(0.0) {
        c = over(c, lerp3(rgb(0xff5a44), rgb(0xd42619), (y - py0) / (py1 - py0)), 1.0);
        if n >= 48 {
            // "LIVE" in a 23 x 7 pixel font.
            let (tx0, tx1, ty0, ty1) = (0.635, 0.905, 0.789, 0.871);
            if x >= tx0 && x < tx1 && y >= ty0 && y < ty1 {
                let col = ((x - tx0) / (tx1 - tx0) * 23.0) as usize;
                let row = ((y - ty0) / (ty1 - ty0) * 7.0) as usize;
                let (g, gx) = (col / 6, col % 6);
                if gx < 5 && g < 4 && GLYPHS[g][row.min(6)] >> (4 - gx) & 1 == 1 {
                    c = over(c, [1.0, 1.0, 1.0], 1.0);
                }
            }
        } else {
            // Too small for text: a white dot.
            if ((x - 0.77).powi(2) + (y - 0.83).powi(2)).sqrt() < 0.04 {
                c = over(c, [1.0, 1.0, 1.0], 1.0);
            }
        }
    }
    c
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn png(n: usize, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..n {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * n * 4..(y + 1) * n * 4]);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&raw).unwrap();
    let idat = z.finish().unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(n as u32).to_be_bytes());
    ihdr.extend_from_slice(&(n as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    out
}

fn main() {
    let sizes = [16usize, 24, 32, 48, 64, 128, 256];
    let images: Vec<(usize, Vec<u8>)> = sizes.iter().map(|&n| (n, png(n, &render(n)))).collect();
    // ICO with PNG entries.
    let mut ico = vec![0, 0, 1, 0];
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (n, data) in &images {
        let b = if *n >= 256 { 0 } else { *n as u8 };
        ico.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in &images {
        ico.extend_from_slice(data);
    }
    std::fs::write("assets/app.ico", ico).unwrap();
    std::fs::write("assets/icon.png", &images.iter().find(|i| i.0 == 256).unwrap().1).unwrap();
    std::fs::write("web/favicon.png", png(64, &render(64))).unwrap();
    println!("wrote assets/app.ico, assets/icon.png, web/favicon.png");
}
