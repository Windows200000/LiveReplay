//! Developer tool: preview sheet of the game's crit icons (scratch output, not committed).
//! `critsheet <out.png>`: row 1 damaged modules, row 2 destroyed modules, row 3 destroyed crew,
//! each shown at 3x on a dark background.
use std::io::Write;
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

fn main() {
    let out = std::env::args().nth(1).unwrap();
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let modules = ["ammobay", "engine", "fueltank", "gun", "radio", "surveyingdevice", "track", "turretrotator", "wheel"];
    let crew = ["commander", "driver", "gunner", "loader", "radioman"];
    let rows: Vec<Vec<String>> = vec![
        modules.iter().map(|m| format!("{m}criticalsmall")).collect(),
        modules.iter().map(|m| format!("{m}destroyedsmall")).collect(),
        crew.iter().map(|m| format!("{m}destroyedsmall")).collect(),
    ];
    let (cell, scale) = (84usize, 3usize);
    let (w, h) = (cell * 9, cell * 3);
    let mut px = vec![0u8; w * h * 4];
    for p in px.chunks_exact_mut(4) { p.copy_from_slice(&[24, 27, 32, 255]); }
    for (ri, row) in rows.iter().enumerate() {
        for (ci, name) in row.iter().enumerate() {
            let Some(img) = gd.gui_file(&format!("gui/maps/icons/library/crits/{name}.png")).and_then(|b| livereplay::gamedata::models::png_image(&b)) else { eprintln!("missing {name}"); continue };
            eprintln!("{name}: {}x{}", img.width, img.height);
            let (ox, oy) = (ci * cell + (cell - img.width * scale) / 2, ri * cell + (cell - img.height * scale) / 2);
            for y in 0..img.height * scale { for x in 0..img.width * scale {
                let s = &img.rgba[((y / scale) * img.width + x / scale) * 4..][..4];
                let (tx, ty) = (ox + x, oy + y);
                if tx >= w || ty >= h { continue }
                let d = &mut px[(ty * w + tx) * 4..][..4];
                let a = s[3] as u32;
                for k in 0..3 { d[k] = ((s[k] as u32 * a + d[k] as u32 * (255 - a)) / 255) as u8; }
            }}
        }
    }
    let mut raw = Vec::new();
    for y in 0..h { raw.push(0); raw.extend_from_slice(&px[y * w * 4..(y + 1) * w * 4]); }
    std::fs::write(out, png_wh(w, h, &raw)).unwrap();
}

fn png_wh(w: usize, h: usize, raw: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(raw).unwrap();
    let idat = z.finish().unwrap();
    let mut out = vec![0x89u8, b'P', b'N', b'G', 13, 10, 26, 10];
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    out
}
