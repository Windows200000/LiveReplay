//! Developer tool: list files in game packages whose path contains all given words.
//! `pkgfind <pkg glob word> <word>...` e.g. `pkgfind gui ammoBay`
use livereplay::gamedata::find_game_dir;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = find_game_dir("").unwrap().join("res").join("packages");
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.contains(&a[0]) || !n.ends_with(".pkg") { continue }
        let Ok(p) = livereplay::gamedata::pkg::Pkg::open(&e.path()) else { continue };
        for f in p.names() {
            let l = f.to_lowercase();
            if a[1..].iter().all(|w| l.contains(&w.to_lowercase())) { println!("{n}: {f}"); }
        }
    }
}
