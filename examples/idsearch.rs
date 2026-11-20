//! Which packet types mention each vehicle id (to find arena-wide vehicle data): `idsearch <replay>`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions, file};
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&a[1]), &gd, &defs)?;
    let rf = file::ReplayFile::open(std::path::Path::new(&a[1]))?;
    let s = rf.packet_stream()?;
    let pk: Vec<_> = file::packets(&s).collect();
    for v in b.vehicles.values() {
        let needle = v.id.to_le_bytes();
        let mut c: std::collections::BTreeMap<u32, (usize, f32)> = Default::default();
        for p in &pk {
            if p.payload.windows(4).any(|w| w == needle) {
                let e = c.entry(p.kind).or_insert((0, p.time));
                e.0 += 1;
            }
        }
        let created = pk.iter().any(|p| p.kind == 5 && p.payload.get(..4) == Some(&needle[..]));
        println!("{:14} team {} created {created:5} death {:?}: {:?}", v.tank_short, v.team, v.death_time.map(|d| d.round()), c.iter().map(|(k, (n, t))| format!("0x{k:02x}x{n}@{t:.0}")).collect::<Vec<_>>());
    }
    Ok(())
}
