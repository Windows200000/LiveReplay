//! Check 0x37 vehicle info records against the final roster: `infocheck <replay>...`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::{vehicle_info_records, Battle}, defs::Definitions, file};
fn main() -> anyhow::Result<()> {
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd)?;
    let (mut ok, mut bad) = (0, 0);
    for path in std::env::args().skip(1) {
        let Ok(b) = Battle::load(std::path::Path::new(&path), &gd, &defs) else { continue };
        let ids: Vec<i32> = b.vehicles.keys().copied().collect();
        let rf = file::ReplayFile::open(std::path::Path::new(&path))?;
        let s = rf.packet_stream()?;
        for p in file::packets(&s).filter(|p| p.kind == 0x37) {
            for (id, name, vt) in vehicle_info_records(&gd, p.payload, &ids) {
                match b.vehicles.get(&id) {
                    Some(v) if v.name == name && v.vehicle_type == vt => ok += 1,
                    Some(v) => { bad += 1; println!("{path}: {id} got {name}/{vt}, roster {}/{} t{}", v.name, v.vehicle_type, v.team) }
                    None => { bad += 1; println!("{path}: unknown id {id} {name} {vt}") }
                }
            }
        }
    }
    println!("ok {ok} bad {bad}");
    Ok(())
}
