//! Print shots in a time window: `cargo run --example shots -- <replay> <from s> <to s>` (replay time).
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&args[1]), &gd, &defs)?;
    let (from, to): (f32, f32) = (args[2].parse()?, args[3].parse()?);
    for s in b.shots.iter().filter(|s| s.t >= from && s.t <= to) {
        let shooter = b.vehicles.get(&s.shooter).map(|v| v.tank_short.as_str()).unwrap_or("?");
        let pos = b.vehicles.get(&s.shooter).and_then(|v| v.track.iter().rev().find(|p| p.t <= s.t)).map(|p| p.pos);
        let mid = s.point(0.5);
        println!(
            "t={:.2} {shooter}: hull {:?} muzzle {:?} -> end {:?} (hit {:?}) mid-height {:.1} sag {:.1} g {:.1}",
            s.t, pos, s.start, s.end, s.hit.and_then(|h| b.vehicles.get(&h)).map(|v| &v.tank_short), mid[1], s.sag(), s.gravity
        );
    }
    Ok(())
}
