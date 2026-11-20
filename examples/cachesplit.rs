//! Developer tool: what the space in each cached map scene is (ground image, object atlas, geometry).
use livereplay::gamedata::{cache3d, webpack};
use std::io::Write;
fn z(b: &[u8]) -> usize { let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast()); e.write_all(b).unwrap(); e.finish().unwrap().len() }
fn main() {
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let c = cache3d::Cache3D::new(&gd).unwrap();
    for e in std::fs::read_dir(c.dir().join("maps")).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.ends_with(".scene.bin") { continue }
        let raw = std::fs::read(e.path()).unwrap();
        let sc = cache3d::decode_scene(&raw).unwrap();
        let mb = |x: usize| x as f64 / 1e6;
        let g = sc.albedo.as_ref().map(|a| (a.len(), z(a))).unwrap_or_default();
        let a = sc.atlas.as_ref().map(|a| (a.len(), z(a))).unwrap_or_default();
        let m = webpack::encode_mesh(&sc.objects);
        println!("{n:34} file {:5.1}  ground {:5.1}/{:5.1}z  atlas {:5.1}/{:5.1}z  mesh quantised {:5.1}/{:5.1}z", mb(raw.len()), mb(g.0), mb(g.1), mb(a.0), mb(a.1), mb(m.len()), mb(z(&m)));
    }
}
