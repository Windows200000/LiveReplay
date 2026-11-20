//! Object triangle counts and GPU vertex bytes per cached map scene: `scenesize`
fn main() {
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let c = livereplay::gamedata::cache3d::Cache3D::new(&gd).unwrap();
    let dir = c.dir().join("maps");
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.ends_with(".scene.bin") { continue }
        let raw = std::fs::read(e.path()).unwrap();
        let Some(sc) = livereplay::gamedata::cache3d::decode_scene(&raw) else { continue };
        let tris = sc.objects.indices.len() / 3;
        let uniq = sc.objects.positions.len();
        println!("{n:32} file {:6.1} MB  tris {tris:8}  verts(indexed) {uniq:8}  soup@48B {:6.1} MB  indexed@28B+idx {:6.1} MB",
            raw.len() as f64 / 1e6, (tris * 3 * 48) as f64 / 1e6, (uniq * 28 + tris * 12) as f64 / 1e6);
    }
}
