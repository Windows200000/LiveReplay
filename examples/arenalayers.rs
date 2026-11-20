//! Print a map's minimapLayers section from its arena definition: `arenalayers <map>`
use livereplay::gamedata::{find_game_dir, GameData};
fn dump(n: &livereplay::gamedata::pxml::Node, depth: usize) {
    println!("{}{} = {}", "  ".repeat(depth), n.name, n.text().chars().take(100).collect::<String>());
    for c in &n.children { dump(c, depth + 1); }
}
fn main() {
    let map = std::env::args().nth(1).unwrap();
    let gd = GameData::new(find_game_dir("").unwrap());
    let root = gd.script_xml(&format!("scripts/arena_defs/{map}.xml")).expect("arena def");
    for c in &root.children {
        if c.name.to_lowercase().contains("layer") || c.name.to_lowercase().contains("random") || c.name.to_lowercase().contains("scenar") { dump(c, 0); }
    }
}
