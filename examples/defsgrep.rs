//! Developer tool: list client methods / properties of the entity definitions matching a word.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::defs::Definitions;
fn main() {
    let words: Vec<String> = std::env::args().skip(1).map(|s| s.to_lowercase()).collect();
    let gd = GameData::new(find_game_dir("").unwrap());
    let d = Definitions::load(&gd).unwrap();
    for e in &d.entities {
        for (i, m) in e.client_methods.iter().enumerate() {
            if words.iter().any(|w| m.name.to_lowercase().contains(w)) { println!("{}.{}() #{i} args {:?}", e.name, m.name, m.args); }
        }
        for p in &e.client_props {
            if words.iter().any(|w| p.name.to_lowercase().contains(w)) { println!("{}.{} prop {:?}", e.name, p.name, p.ty); }
        }
    }
}
