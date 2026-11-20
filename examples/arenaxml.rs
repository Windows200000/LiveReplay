//! Print an arena definition's gameplayTypes section: `arenaxml <map> [gameplay]`
use livereplay::gamedata::{find_game_dir, GameData};
fn dump(n: &livereplay::gamedata::pxml::Node, d: usize) {
    println!("{}{} = {}", "  ".repeat(d), n.name, n.text().chars().take(80).collect::<String>());
    for c in &n.children { dump(c, d + 1); }
}
fn main() {
    let map = std::env::args().nth(1).unwrap();
    let want = std::env::args().nth(2).unwrap_or_default();
    let gd = GameData::new(find_game_dir("").unwrap());
    let root = gd.script_xml(&format!("scripts/arena_defs/{map}.xml")).expect("arena def");
    if let Some(g) = root.child("gameplayTypes") {
        for c in &g.children {
            if want.is_empty() || c.name == want { dump(c, 0); }
        }
    }
}
