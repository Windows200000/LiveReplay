//! lifeTime / deathReason / killerID per vehicle from the results block vs. what the stream gave.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&a[1]), &gd, &defs)?;
    let start = b.battle_start();
    for v in b.vehicles.values() {
        let f = v.final_stats.as_ref();
        let g = |k: &str| f.and_then(|s| s.get(k)).cloned().unwrap_or_default();
        println!("{:14} t{} stream death {:?}  results lifeTime {} deathReason {} killer {} health {}", v.tank_short, v.team, v.death_time.map(|d| (d - start).round()), g("lifeTime"), g("deathReason"), g("killerID"), g("health"));
    }
    Ok(())
}
