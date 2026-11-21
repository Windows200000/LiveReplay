//! Developer tool: the arenaUniqueID read from a replay's packets vs. the one in its results.
//! `arenaid <replay>...`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions, file::ReplayFile};

fn main() -> anyhow::Result<()> {
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    for f in std::env::args().skip(1) {
        let p = std::path::Path::new(&f);
        let b = Battle::load(p, &gd, &defs)?;
        let res = ReplayFile::open(p)?.results().and_then(|r| r.get(0)?.get("arenaUniqueID")?.as_u64());
        println!("{} packets={} results={:?}", p.file_name().unwrap().to_string_lossy(), b.arena_id, res);
    }
    Ok(())
}
