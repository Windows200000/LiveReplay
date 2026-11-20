//! Random-event minimap layers of a map: opaque pixel count, centre, average colour.
use livereplay::gamedata::{find_game_dir, GameData};
fn main() {
    let map = std::env::args().nth(1).unwrap();
    let gd = GameData::new(find_game_dir("").unwrap());
    let a = gd.arena(&map).unwrap();
    println!("bbox {:?} {:?}", a.bbox_min, a.bbox_max);
    for (p, img) in gd.minimap_event_layers(&a) {
        let (mut n, mut sx, mut sy, mut c) = (0usize, 0usize, 0usize, [0usize; 4]);
        for y in 0..img.height { for x in 0..img.width {
            let o = (y * img.width + x) * 4;
            if img.rgba[o + 3] > 40 { n += 1; sx += x; sy += y; for k in 0..4 { c[k] += img.rgba[o + k] as usize; } }
        }}
        let n1 = n.max(1);
        println!("{p}: {}x{} opaque {n} centre px ({}, {}) avg rgba {:?}", img.width, img.height, sx / n1, sy / n1, c.map(|v| v / n1));
    }
}
