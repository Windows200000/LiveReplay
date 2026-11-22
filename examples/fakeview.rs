//! Developer tool: upload a replay as if a second player (an ally of the recorder) had recorded
//! the same battle: other recorder, clock shifted, and the recorder's enemies only seen half the
//! time. `fakeview <server> <replay> <date as in the original upload>` — tests merged viewpoints.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::net::api::*;
use livereplay::replay::{battle::Battle, defs::Definitions, wire};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (server, file, date) = (&args[1], &args[2], &args[3]);
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let mut b = Battle::load(std::path::Path::new(file), &gd, &defs)?;
    let me = b.recorder_vehicle.unwrap();
    let other = *b.vehicles.iter().find(|(id, v)| **id != me && v.team == b.recorder_team && v.track.len() > 500).map(|(id, _)| id).unwrap();
    b.recorder_vehicle = Some(other);
    // Different recording start: everything 7 s later.
    let tmp = { let mut x = Battle::default(); std::mem::swap(&mut x, &mut b); x };
    let mut b = shift(tmp, 7.0);
    for v in b.vehicles.values_mut() { v.final_stats = None; }
    let key = "fakeview-test-key-0123456789abcdef0123456789";
    let nb = NewBattle { player: "viewpoint_tester".into(), realm: b.server_realm.clone(), map: b.map.clone(), map_display: b.map_display.clone(),
        mode: "Test".into(), tank: b.vehicles[&other].tank_short.clone(), date: date.clone(), version: String::new(), live: false,
        file_name: "fakeview.wotreplay".into(), arena_id: 0, vehicle_ids: b.vehicles.keys().copied().collect(), players: b.vehicles.values().map(|v| v.name.clone()).collect() };
    let info: BattleInfo = ureq::post(&format!("{server}/api/battles")).header(KEY_HEADER, key).send_json(&nb)?.body_mut().read_json()?;
    ureq::post(&format!("{server}/api/battles/{}/frames?reset=1&finish=1", info.id)).header(KEY_HEADER, key).send(&wire::encode_frame(&b, 0.0))?;
    println!("battle {} group {:?} recorder {other}", info.id, info.group);
    b.duration = 0.0;
    Ok(())
}

fn shift(mut b: Battle, dt: f32) -> Battle {
    for v in b.vehicles.values_mut() {
        v.track.iter_mut().for_each(|s| s.t += dt);
        v.visibility.iter_mut().for_each(|s| s.0 += dt);
        v.health.iter_mut().for_each(|s| s.0 += dt);
        v.gun_codes.iter_mut().for_each(|s| s.0 += dt);
        if let Some(d) = &mut v.death_time { *d += dt; }
    }
    b.events.iter_mut().for_each(|e| e.t += dt);
    b.shots.iter_mut().for_each(|s| { s.t += dt; s.end_t += dt; });
    b.hits.iter_mut().for_each(|h| h.t += dt);
    b.captures.iter_mut().for_each(|c| c.t += dt);
    b.periods.iter_mut().for_each(|p| p.0 += dt);
    b.duration += dt;
    b
}
