//! Developer tool: play a finished replay to a server as if it were a live battle.
//! `fakelive <server> <replay> [speed]` — prints a viewing code, sends a frame every 2 s.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::net::api::*;
use livereplay::replay::{battle::Battle, defs::Definitions, wire};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (server, file) = (&args[1], &args[2]);
    let speed: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let full = Battle::load(std::path::Path::new(file), &gd, &defs)?;
    let key = "fakelive-test-key-0123456789abcdef0123456789";
    let nb = NewBattle { player: "live_tester".into(), realm: full.server_realm.clone(), map: full.map.clone(), map_display: full.map_display.clone(),
        mode: "Test".into(), tank: "Test tank".into(), date: "20.11.2026 12:00:00".into(), version: String::new(), live: true, file_name: String::new(), arena_id: full.arena_id,
        vehicle_ids: full.vehicles.keys().copied().collect(), players: vec![] };
    let info: BattleInfo = ureq::post(&format!("{server}/api/battles")).set(KEY_HEADER, key).send_json(&nb)?.into_json()?;
    let code: CodeInfo = ureq::post(&format!("{server}/api/codes")).set(KEY_HEADER, key).send_json(&NewCode { days: 1 })?.into_json()?;
    println!("battle {} code {}", info.id, code.code);
    let mut sent = 0.0f32;
    let mut t = 0.0f32;
    while t < full.duration {
        t += 2.0 * speed;
        let mut part = full.clone();
        part.duration = t.min(full.duration);
        for v in part.vehicles.values_mut() {
            v.track.retain(|s| s.t < t); v.gun_codes.retain(|s| s.0 < t); v.health.retain(|s| s.0 < t);
            v.visibility.retain(|s| s.0 < t); v.shot_pitch.retain(|s| s.0 < t);
            v.final_stats = None;
            if v.death_time.is_some_and(|d| d >= t) { v.death_time = None; }
        }
        part.shots.retain(|s| s.t < t); part.events.retain(|s| s.t < t); part.hits.retain(|s| s.t < t); part.captures.retain(|s| s.t < t);
        part.periods.retain(|p| p.0 < t);
        part.live = true;
        let from = if sent > 0.0 { (sent - 2.0).max(0.001) } else { 0.0 };
        ureq::post(&format!("{server}/api/battles/{}/frames", info.id)).set(KEY_HEADER, key).send_bytes(&wire::encode_frame(&part, from))?;
        sent = part.duration;
        println!("sent until {sent:.0}");
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    ureq::post(&format!("{server}/api/battles/{}/frames?reset=1&finish=1", info.id)).set(KEY_HEADER, key).send_bytes(&wire::encode_frame(&full, 0.0))?;
    println!("finished");
    Ok(())
}
