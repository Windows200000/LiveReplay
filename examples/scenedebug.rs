//! Build a map scene into a throw-away cache (set LIVEREPLAY_DEBUG_OBJECTS=1 to list objects):
//! `scenedebug <map> <gameplay id>`
use livereplay::gamedata::{cache3d::Cache3D, find_game_dir, GameData};

fn main() {
    let mut a = std::env::args().skip(1);
    let map = a.next().unwrap_or_else(|| "44_north_america".into());
    let gp: i32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let dir = std::env::temp_dir().join("livereplay-scenedebug");
    let _ = std::fs::remove_dir_all(&dir);
    let sc = Cache3D::at(dir).scene(&gd, &map, gp).expect("scene");
    eprintln!("objects: {} triangles, water {}", sc.objects.indices.len() / 3, sc.water.len());
}
