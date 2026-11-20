//! Developer tool: size of a battle in the wire encoding (whole, and as live chunks).
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions, wire};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&args[1]), &gd, &defs)?;
    let raw = std::fs::metadata(&args[1])?.len();
    let t = std::time::Instant::now();
    let full = wire::encode_frame(&b, 0.0);
    println!("replay {raw} bytes -> wire {} bytes ({:?})", full.len(), t.elapsed());
    let t = std::time::Instant::now();
    let back = wire::decode_battle_checked(&full).unwrap();
    println!("decode {:?}; vehicles {} samples {} vs {}", t.elapsed(), back.vehicles.len(),
        back.vehicles.values().map(|v| v.track.len()).sum::<usize>(), b.vehicles.values().map(|v| v.track.len()).sum::<usize>());
    // live: a frame every 5 s with 4 s overlap
    let (mut total, mut n, mut last) = (0usize, 0, 0.0f32);
    let mut t = 5.0;
    while t < b.duration + 5.0 {
        let mut part = wire::slice(&b, 0.0);
        for v in part.vehicles.values_mut() { v.track.retain(|s| s.t < t); v.gun_codes.retain(|s| s.0 < t); v.health.retain(|s| s.0 < t); v.visibility.retain(|s| s.0 < t); v.shot_pitch.retain(|s| s.0 < t); }
        part.shots.retain(|s| s.t < t); part.events.retain(|s| s.t < t); part.hits.retain(|s| s.t < t); part.captures.retain(|s| s.t < t);
        let from = (last - 4.0).max(0.0);
        total += wire::encode_frame(&part, from).len();
        n += 1;
        last = t;
        t += 5.0;
    }
    println!("live: {n} frames, {total} bytes; metadata-only frame {} bytes", wire::encode_frame(&b, b.duration + 1.0).len());
    Ok(())
}
