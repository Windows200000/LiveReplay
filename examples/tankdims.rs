//! Print a tank model's pivots and bounds: `cargo run --example tankdims -- germany:G42_Maus`
use livereplay::gamedata::{cache3d::Cache3D, find_game_dir, GameData};

fn main() {
    let vt = std::env::args().nth(1).unwrap_or_else(|| "germany:G42_Maus".into());
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let cache = Cache3D::new(&gd).expect("cache");
    let t = cache.tank(&gd, &vt).expect("tank");
    println!("body bounds {:?}", t.body.bounds());
    println!("turret pivot {:?} bounds {:?}", t.turret_pivot, t.turret.bounds());
    println!("gun pivot {:?} bounds {:?}", t.gun_pivot, t.gun.bounds());
}
