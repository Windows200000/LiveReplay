//! Brightness statistics of textures named in an .atlas file or given directly: `texstat <path>`
use livereplay::gamedata::{find_game_dir, GameData};
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let gd = GameData::new(find_game_dir("").unwrap());
    let read = |p: &str| -> Option<Vec<u8>> {
        for pkg in std::fs::read_dir(gd.root.join("res/packages")).ok()?.flatten() {
            let n = pkg.file_name().to_string_lossy().into_owned();
            if n.contains("_hd") { continue; }
            if let Some(b) = gd.read(&n, p) { return Some(b); }
        }
        None
    };
    let mut paths = vec![path.clone()];
    if path.ends_with(".atlas") {
        let b = read(&format!("{path}_processed")).expect("atlas");
        paths = b.split(|&c| c == 0).filter_map(|s| std::str::from_utf8(s).ok()).filter_map(|s| s.find("content/").map(|i| s[i..].replace(".png", ".dds"))).collect();
    }
    for p in paths {
        let Some(b) = read(&p) else { println!("{p}: not found"); continue };
        let img = livereplay::gamedata::dds::decode_level(&b, 2).unwrap();
        let l: Vec<f32> = img.rgba.chunks_exact(4).map(|c| 0.3 * c[0] as f32 + 0.59 * c[1] as f32 + 0.11 * c[2] as f32).collect();
        let m = l.iter().sum::<f32>() / l.len() as f32;
        let sd = (l.iter().map(|v| (v - m).powi(2)).sum::<f32>() / l.len() as f32).sqrt();
        let avg = [0, 1, 2].map(|k| img.rgba.chunks_exact(4).map(|c| c[k] as u32).sum::<u32>() / (img.rgba.len() / 4) as u32);
        println!("{p}: {}x{} mean {m:.0} sd {sd:.1} rgb {avg:?}", img.width, img.height);
    }
}
