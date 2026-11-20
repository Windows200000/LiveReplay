//! Water bodies and terrain heights at points: `water <map> [x,z ...]`
use livereplay::gamedata::{cache3d::Cache3D, find_game_dir, GameData};

fn main() {
    let mut a = std::env::args().skip(1);
    let map = a.next().unwrap();
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let pkg = format!("{map}.pkg");
    let space = gd.read(&pkg, &format!("spaces/{map}/space.bin")).and_then(|b| livereplay::gamedata::space::parse(&b)).expect("space");
    for w in &space.water {
        println!("water {:?} .. {:?}", w.0, w.1);
    }
    let t = Cache3D::new(&gd).unwrap().terrain(&gd, &map).unwrap();
    for p in a {
        let v: Vec<f32> = p.split(',').filter_map(|s| s.parse().ok()).collect();
        println!("terrain at {p}: {:.2}", t.height_at(v[0], v[1]));
    }
}
