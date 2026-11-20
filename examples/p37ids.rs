//! Packets of a type that contain any vehicle id, with the id's offset: `p37ids <replay> <type hex>`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions, file};
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let kind = u32::from_str_radix(&a[2], 16)?;
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&a[1]), &gd, &defs)?;
    let rf = file::ReplayFile::open(std::path::Path::new(&a[1]))?;
    let s = rf.packet_stream()?;
    for p in file::packets(&s).filter(|p| p.kind == kind) {
        let mut hits = Vec::new();
        for v in b.vehicles.values() {
            let n = v.id.to_le_bytes();
            for (i, w) in p.payload.windows(4).enumerate() {
                if w == n { hits.push(format!("{}@{i}", v.tank_short)); }
            }
        }
        if hits.is_empty() { continue; }
        let hex: String = p.payload.iter().take(64).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
        println!("t={:6.1} len={:4} ids={hits:?}\n   {hex}", p.time, p.payload.len());
    }
    Ok(())
}
