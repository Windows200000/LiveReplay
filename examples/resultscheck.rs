//! Developer tool: learn the cached battle-result layouts from replays with results, then rebuild
//! the results of those replays from the cache and compare every field with the real block.
use livereplay::replay::{cached_results, file::ReplayFile};
fn main() {
    cached_results::archive_new();
    let dir = std::env::args().nth(1).unwrap_or("D:/Games/World_of_Tanks_EU/replays".into());
    let mut with = Vec::new();
    let mut without = 0;
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "wotreplay") { continue }
        let Ok(f) = ReplayFile::open(&p) else { continue };
        match f.results().and_then(|r| r.get(0)).cloned() {
            Some(r) => with.push((p, r, f.start_info().clone())),
            None => without += 1,
        }
    }
    println!("replays with results {}, without {without}", with.len());
    cached_results::learn(&with.iter().map(|w| w.1.clone()).collect::<Vec<_>>());
    let (mut fields, mut equal, mut battles) = (0, 0, 0);
    for (p, real, si) in &with {
        let ids: Vec<i32> = si.get("vehicles").and_then(|v| v.as_object()).map(|o| o.keys().filter_map(|k| k.parse().ok()).collect()).unwrap_or_default();
        let Some(rebuilt) = cached_results::results_for(&ids) else { continue };
        let rebuilt = &rebuilt[0];
        if rebuilt.get("arenaUniqueID") != real.get("arenaUniqueID") { println!("{}: matched a different battle!", p.display()); continue }
        battles += 1;
        for (vid, list) in real["vehicles"].as_object().unwrap() {
            let r = &list[0];
            let b = &rebuilt["vehicles"][vid][0];
            for k in ["damageDealt", "kills", "xp", "spotted", "damageAssistedRadio", "damageAssistedTrack", "damageBlockedByArmor", "shots", "directEnemyHits", "piercingEnemyHits", "lifeTime", "killerID", "deathReason", "accountDBID", "credits", "health", "damageReceived"] {
                fields += 1;
                let rv = r.get(k).cloned().unwrap_or_default();
                let bv = b.get(k).cloned().unwrap_or(serde_json::Value::from(0));
                if rv == bv || (rv.as_f64() == bv.as_f64() && rv.as_f64().is_some()) { equal += 1 } else if fields < 400 { println!("  {k}: real {rv} rebuilt {bv}") }
            }
        }
        if let (Some(a), Some(b)) = (real["common"].get("winnerTeam"), rebuilt["common"].get("winnerTeam")) { if a != b { println!("winner differs") } }
    }
    println!("battles rebuilt {battles}, fields equal {equal}/{fields}");
}
