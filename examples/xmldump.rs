//! Developer tool: print a packed XML file from scripts.pkg as an indented tree.
fn show(n: &livereplay::gamedata::pxml::Node, d: usize, max_d: usize) {
    if d > max_d { return }
    let t = n.text.trim();
    println!("{}{}{}", "  ".repeat(d), n.name, if t.is_empty() { String::new() } else { format!(" = {}", &t[..t.len().min(80)]) });
    for c in &n.children { show(c, d + 1, max_d) }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let n = gd.script_xml(&a[1]).expect("not found");
    show(&n, 0, a.get(2).and_then(|s| s.parse().ok()).unwrap_or(3));
}
