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

/// Colour + alpha of the icon at (x, y) in 0..1.
fn shade(x: f32, y: f32, n: usize) -> [f32; 4] {
    let mut c = [0.0f32; 4];
    // Rounded square background with a soft vertical gradient.
    let r = 0.2;
    let (dx, dy) = ((x - 0.5).abs() - (0.5 - r), (y - 0.5).abs() - (0.5 - r));
    let inside = if dx > 0.0 && dy > 0.0 { (dx * dx + dy * dy).sqrt() <= r } else { true };
    if !inside {
        return c;
    }
    let top = rgb(0x2a2f37);
    let bot = rgb(0x16191d);
    let bg = [top[0] + (bot[0] - top[0]) * y, top[1] + (bot[1] - top[1]) * y, top[2] + (bot[2] - top[2]) * y];
    c = over(c, bg, 1.0);
    // Minimap grid (only where it stays crisp enough).
    if n >= 32 {
        let (m0, m1) = (0.14, 0.86);
        if (m0..=m1).contains(&x) && (m0..=m1).contains(&y) {
            let w = 0.008;
            for k in 0..=4 {
                let g = m0 + (m1 - m0) * k as f32 / 4.0;
                if (x - g).abs() < w || (y - g).abs() < w {
                    c = over(c, [1.0, 1.0, 1.0], 0.07);
                }
            }
        }
    }
    // Green view-range ring.
    let (cx, cy) = (0.48, 0.54);
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    let ring_w = if n <= 16 { 0.09 } else { 0.06 };
    if (d - 0.29).abs() < ring_w / 2.0 {
        c = over(c, rgb(0x80c824), 1.0);
    }
    // White play triangle in the ring.
    let (tx, ty) = (x - cx, y - cy);
    let s = 0.17;
    if tx > -s * 0.55 && tx < s * 0.95 && ty.abs() < (s * 0.95 - tx) * 0.62 {
        c = over(c, rgb(0xf4f4f0), 1.0);
    }
    // Red "live" dot with a dark rim.
    let (lx, ly) = (0.79, 0.21);
    let ld = ((x - lx).powi(2) + (y - ly).powi(2)).sqrt();
    if ld < 0.125 {
        c = over(c, rgb(0x16191d), 1.0);
    }
    if ld < 0.09 {
        c = over(c, rgb(0xff4a36), 1.0);
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
