//! Print terrain / arena extents for a map: `cargo run --example terrain -- 23_westfeld`
use livereplay::gamedata::{cache3d::Cache3D, find_game_dir, GameData};

fn main() {
    let map = std::env::args().nth(1).unwrap_or_else(|| "23_westfeld".into());
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let cache = Cache3D::new(&gd).expect("cache");
    let a = gd.arena(&map).expect("arena");
    println!("arena bbox {:?} {:?}", a.bbox_min, a.bbox_max);
    let t = cache.terrain(&gd, &map).expect("terrain");
    println!("terrain x0 {} z0 {} spacing {} w {} h {}", t.x0, t.z0, t.spacing, t.w, t.h);
    let (mn, mx) = t.heights.iter().fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
    println!("heights {mn}..{mx}");
    for zi in (0..t.h).step_by(t.h / 8) {
        let row: Vec<String> = (0..t.w).step_by(t.w / 12).map(|xi| format!("{:6.1}", t.heights[zi * t.w + xi])).collect();
        println!("{}", row.join(" "));
    }
}
