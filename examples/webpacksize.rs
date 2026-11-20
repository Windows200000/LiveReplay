//! Developer tool: sizes of the web 3D packs for every cached map scene and tank.
use livereplay::gamedata::{cache3d, webpack};
fn main() {
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let c = cache3d::Cache3D::new(&gd).unwrap();
    for e in std::fs::read_dir(c.dir().join("maps")).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let raw = std::fs::read(e.path()).unwrap();
        if n.ends_with(".scene.bin") {
            let sc = cache3d::decode_scene(&raw).unwrap();
            let t = std::time::Instant::now();
            let p = webpack::scene_pack(&sc);
            let back = webpack::decode_scene_pack(&p).unwrap();
            println!("{n:34} {:6.1} MB -> {:5.2} MB  ({:?}; verts {} = {})", raw.len() as f64 / 1e6, p.len() as f64 / 1e6, t.elapsed(), sc.objects.positions.len(), back.objects.positions.len());
        } else if let Some(t) = cache3d::decode_terrain(&raw) {
            println!("{n:34} {:6.3} MB -> {:5.3} MB terrain", raw.len() as f64 / 1e6, webpack::terrain_pack(&t).len() as f64 / 1e6);
        }
    }
    let (mut a, mut b) = (0, 0);
    for e in std::fs::read_dir(c.dir().join("tanks")).unwrap().flatten() {
        let raw = std::fs::read(e.path()).unwrap();
        if let Some(t) = cache3d::decode_tank(&raw) { a += raw.len(); b += webpack::tank_pack(&t).len(); }
    }
    println!("tanks: {a} -> {b} bytes");
}
