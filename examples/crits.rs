//! Developer tool: module / crew state changes decoded from a replay.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd).unwrap();
    let b = Battle::load(std::path::Path::new(&a[1]), &gd, &defs).unwrap();
    println!("battle start {}", b.battle_start());
    let ex = gd.vehicle_extras();
    println!("extras: {} slots, 66 = {:?}, 67 = {:?}", ex.iter().flatten().count(), ex.get(66), ex.get(67));
    for v in b.vehicles.values().filter(|v| !v.crits.is_empty()) {
        let s: Vec<String> = v.crits.iter().map(|c| format!("{:.1} {} {}", c.t, c.slot, c.state)).collect();
        println!("{:<22} {}", v.tank_short, s.join(" | "));
    }
}
