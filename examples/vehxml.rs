//! maxHealth fields of a vehicle definition: `vehxml ussr R228_KR_1`
fn dump(n: &livereplay::gamedata::pxml::Node, path: &str) {
    let p = format!("{path}/{}", n.name);
    if n.name == "maxHealth" { println!("{p} = {}", n.text()); }
    for c in &n.children { dump(c, &p); }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let gd = livereplay::gamedata::GameData::new(livereplay::gamedata::find_game_dir("").unwrap());
    let root = gd.script_xml(&format!("scripts/item_defs/vehicles/{}/{}.xml", a[1], a[2])).expect("xml");
    dump(&root, "");
}
