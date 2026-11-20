//! Developer tool: compressed size of each part of a cached map scene's geometry.
use livereplay::gamedata::{cache3d, webpack, models::Mesh};
use std::io::Write;
fn z(b: &[u8]) -> f64 { let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best()); e.write_all(b).unwrap(); e.finish().unwrap().len() as f64 / 1e6 }
fn main() {
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let c = cache3d::Cache3D::new(&gd).unwrap();
    let sc = c.scene(&gd, "10_hills", 0).unwrap();
    let m = &sc.objects;
    let part = |f: &dyn Fn(&mut Mesh)| { let mut x = m.clone(); f(&mut x); z(&webpack::encode_mesh(&x)) };
    let all = part(&|_| {});
    println!("all {all:.2} MB; verts {} idx {} uv {} tex {}", m.positions.len(), m.indices.len(), m.corner_uv.len(), m.tri_tex.len());
    println!("without uv {:.2}  without colors {:.2}  without tri_tex {:.2}", part(&|x| x.corner_uv.clear()), part(&|x| x.colors.clear()), part(&|x| x.tri_tex.clear()));
    let untex = m.tri_tex.iter().filter(|t| **t == u16::MAX).count();
    println!("untextured triangles {untex} of {}", m.tri_tex.len());
    println!("ground {:.2} MB at 2048, {:.2} MB at 1024", z(&webpack::drop_mips(sc.albedo.as_ref().unwrap(), 2048)), z(&webpack::drop_mips(sc.albedo.as_ref().unwrap(), 1024)));
}
