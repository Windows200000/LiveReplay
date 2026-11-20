//! Developer tool: viewer death times vs the results block's lifeTime, over many replays,
//! with LIVEREPLAY_LIKE_LIVE semantics optional (stream only). `deathtimes <dir> [n]`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions, file::ReplayFile};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&a[1]).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wotreplay")).collect();
    files.sort();
    files.reverse();
    let (mut ok, mut late, mut missing, mut total) = (0, 0, 0, 0);
    for f in files.iter().take(n) {
        let Ok(rf) = ReplayFile::open(f) else { continue };
        let Some(res) = rf.results().and_then(|r| r.get(0)).cloned() else { continue };
        // Stream only (what a live battle has).
        std::env::set_var("LIVEREPLAY_LIKE_LIVE", "1");
        let Ok(b) = Battle::load(f, &gd, &defs) else { continue };
        std::env::remove_var("LIVEREPLAY_LIKE_LIVE");
        let s = b.battle_start();
        for (vid, list) in res["vehicles"].as_object().unwrap() {
            let st = &list[0];
            let life = st["lifeTime"].as_f64().unwrap_or(0.0) as f32;
            let dead = st["deathReason"].as_i64().unwrap_or(-1) >= 0;
            if !dead { continue }
            total += 1;
            let id: i32 = vid.parse().unwrap();
            match b.vehicles.get(&id).and_then(|v| v.death_time) {
                Some(d) if (d - s - life).abs() <= 2.0 => ok += 1,
                Some(d) => { late += 1; println!("{}: {} death at {:.1}, results {:.0}", f.file_name().unwrap().to_string_lossy(), b.vehicles[&id].name, d - s, life) }
                None => { missing += 1; println!("{}: {} no death in stream (results {:.0})", f.file_name().unwrap().to_string_lossy(), b.vehicles.get(&id).map(|v| v.name.as_str()).unwrap_or("?"), life) }
            }
        }
    }
    println!("deaths {total}: on time {ok}, off by >2 s {late}, missing {missing}");
}
