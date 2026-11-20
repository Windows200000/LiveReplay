//! Dump a model's .visual_processed materials: `visual <path>`
use livereplay::gamedata::{find_game_dir, GameData};
fn dump(n: &livereplay::gamedata::pxml::Node, d: usize) {
    if n.name == "renderSet" || n.name == "geometry" || n.name == "primitiveGroup" || n.name == "material" || n.name == "property" || d < 1 || n.children.is_empty() {
        println!("{}{} = {}", "  ".repeat(d), n.name, n.text().chars().take(90).collect::<String>());
    }
    for c in &n.children { dump(c, d + 1); }
}
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let gd = GameData::new(find_game_dir("").unwrap());
    for pkg in std::fs::read_dir(gd.root.join("res/packages")).unwrap().flatten() {
        let name = pkg.file_name().to_string_lossy().into_owned();
        if let Some(b) = gd.read(&name, &path) {
            if let Some(n) = livereplay::gamedata::pxml::parse(&b) { dump(&n, 0); }
            return;
        }
    }
}
