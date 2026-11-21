//! Developer tool: the arena periods a recording contains, and its winner. `periods <file>...`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};

fn main() -> anyhow::Result<()> {
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    for f in std::env::args().skip(1) {
        let b = Battle::load(std::path::Path::new(&f), &gd, &defs)?;
        println!("{f}: duration {:.0}, winner {:?}, periods {:?}", b.duration, b.winner_team, b.periods);
    }
    Ok(())
}
