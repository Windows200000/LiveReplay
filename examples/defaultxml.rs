//! Print scripts/arena_defs/_default_.xml nodes whose path mentions a filter: `defaultxml [filter]`
use livereplay::gamedata::{find_game_dir, GameData};
fn dump(n: &livereplay::gamedata::pxml::Node, path: &str, f: &str) {
    let p = format!("{path}/{}", n.name);
    if p.to_lowercase().contains(f) && !n.text().trim().is_empty() { println!("{p} = {}", n.text().chars().take(80).collect::<String>()); }
    for c in &n.children { dump(c, &p, f); }
}
fn main() {
    let f = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let gd = GameData::new(find_game_dir("").unwrap());
    let root = gd.script_xml("scripts/arena_defs/_default_.xml").expect("default");
    dump(&root, "", &f);
}
