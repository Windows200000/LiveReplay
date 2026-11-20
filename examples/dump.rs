//! Developer tool: decode a replay into the battle timeline and print a summary.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let t0 = std::time::Instant::now();
    let b = Battle::load(std::path::Path::new(&args[1]), &gd, &defs)?;
    println!("loaded in {:?}: map {} ({}) gameplay {} recorder {:?} team {} dur {:.0}s periods {:?} winner {:?}", t0.elapsed(), b.map, b.map_display, b.gameplay, b.recorder_vehicle, b.recorder_team, b.duration, b.periods, b.winner_team);
    if let Some(a) = gd.arena(&b.map) { println!("arena bbox {:?}..{:?} gp {:?}", a.bbox_min, a.bbox_max, a.gameplay(&b.gameplay)); }
    for v in b.vehicles.values() {
        let first = v.track.first().map(|s| format!("{:.0},{:.0},{:.0} yaw {:.2}", s.pos[0], s.pos[1], s.pos[2], s.yaw)).unwrap_or_default();
        println!("{} t{} {:<20} {:<6} {:<22} {:?} T{} hp {}/{} track {} vis {} death {:?} first {}", v.id, v.team, v.name, v.clan, v.tank_short, v.class, v.tier, v.health.last().map(|h| h.1).unwrap_or(-1), v.max_health, v.track.len(), v.visibility.len(), v.death_time, first);
    }
    let (mut s, mut d, mut k) = (0, 0, 0);
    for e in &b.events { match e.kind { livereplay::replay::battle::EventKind::Shot{..} => s+=1, livereplay::replay::battle::EventKind::Damage{..} => d+=1, _ => k+=1 } }
    println!("events: shots {s} damage {d} kills {k}");
    let landed = b.shots.iter().filter(|s| s.end_t - s.t < 5.0).count();
    let hit = b.shots.iter().filter(|s| s.hit.is_some()).count();
    println!("tracers: {} (with impact {landed}, tank hits {hit}), hit events {}", b.shots.len(), b.hits.len());
    println!("gameplay id {}; random-event objects:", b.gameplay_id);
    for e in &b.random_events { println!("  event {} warned {:.1} start {:?} end {:?} pos {:?}", e.name, e.warned, e.start, e.end, e.pos); }
    for s in &b.scenario { println!("  {:<52} at ({:.0},{:.0},{:.0}) yaw {:.2} from {:.1} to {:?}", s.prefab, s.pos[0], s.pos[1], s.pos[2], s.yaw, s.created, s.removed); }
    // Turret check: gun direction (hull yaw + turret yaw) vs. tracer direction at the shot.
    let mut diffs = Vec::new();
    for s in &b.shots {
        let Some(v) = b.vehicles.get(&s.shooter) else { continue };
        let Some(hull) = v.track.iter().rev().find(|p| p.t <= s.t) else { continue };
        let Some(tur) = v.turret_yaw.iter().rev().find(|p| p.0 <= s.t) else { continue };
        let dir = (s.end[0] - s.start[0]).atan2(s.end[2] - s.start[2]);
        let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        diffs.push((wrap(hull.yaw + tur.1 - dir), wrap(hull.yaw - tur.1 - dir)));
    }
    let med = |mut v: Vec<f32>| { v.sort_by(|a, b| a.abs().total_cmp(&b.abs())); v.get(v.len() / 2).copied().unwrap_or(0.0) };
    println!("turret check over {} shots: median |err| hull+turret {:.2} rad, hull-turret {:.2} rad", diffs.len(), med(diffs.iter().map(|d| d.0).collect()), med(diffs.iter().map(|d| d.1).collect()));
    for s in b.shots.iter().take(3) { println!("  shot t={:.1} from {:?} to {:?} flight {:.2}s hit {:?}", s.t, s.start, s.end, s.end_t - s.t, s.hit); }
    if let Some(focus) = args.get(2).and_then(|x| x.parse::<i32>().ok()) {
        for e in &b.events {
            if let livereplay::replay::battle::EventKind::Damage { attacker, target, amount } = e.kind {
                if attacker == focus || target == focus {
                    let n = |id: i32| b.vehicles.get(&id).map(|v| v.name.clone()).unwrap_or(id.to_string());
                    println!("  t={:.1} {} -> {} : {}", e.t, n(attacker), n(target), amount);
                }
            }
        }
    }
    Ok(())
}
