//! Developer tool: write a finished replay's packets as a header-less in-progress file
//! (like the game's `temp.wotreplay`) and print the list summary built from it.
//! `livetemp <replay> <out>`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions, file::ReplayFile};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::Path::new(&args[2]);
    std::fs::write(out, ReplayFile::open(std::path::Path::new(&args[1]))?.packet_stream()?)?;
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(out, &gd, &defs)?;
    println!("summarize: {:?}", livereplay::app::library::summarize(out, Some(&gd)).is_some());
    println!("{:#?}", livereplay::app::library::live_entry(out, &b, &gd));
    Ok(())
}
