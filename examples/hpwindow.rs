//! Developer tool: health changes in a time window (seconds after battle start), with whether the
//! tank was in view and its death time. `hpwindow <replay> <from> <to>`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd).unwrap();
    let b = Battle::load(std::path::Path::new(&a[1]), &gd, &defs).unwrap();
    let s = b.battle_start();
    let (from, to): (f32, f32) = (a[2].parse::<f32>().unwrap() + s, a[3].parse::<f32>().unwrap() + s);
    for v in b.vehicles.values() {
        let in_view = |t: f32| v.visibility.iter().rev().find(|x| x.0 <= t).map(|x| x.1).unwrap_or(true);
        let last_pos = |t: f32| v.track.iter().rev().find(|x| x.t <= t).map(|x| x.t);
        for w in v.health.windows(2) {
            if w[1].0 >= from && w[1].0 <= to {
                println!("{:<14} {:<16} t={:.1} ({:.1}) hp {} -> {} in_view={} last_pos_t={:?} death={:?}",
                    v.tank_short, v.name, w[1].0 - s, w[1].0, w[0].1, w[1].1, in_view(w[1].0), last_pos(w[1].0).map(|x| x - s), v.death_time.map(|d| d - s));
            }
        }
    }
}
