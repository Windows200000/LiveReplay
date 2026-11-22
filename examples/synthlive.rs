//! Developer tool: a made-up live battle (10 tanks driving circles) sent to a server the way the
//! app sends a real one (every 2 s, the last 2 s again), for testing live playback on the website
//! without World of Tanks. Also uploads a plain map pack and an empty icon pack.
//! `synthlive <server> [seconds]` — prints the battle link and a viewing code.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use livereplay::gamedata::{ArenaDef, GameplayDef};
use livereplay::net::api::*;
use livereplay::net::{encode_pack, IconPack, MapPack};
use livereplay::replay::battle::{Battle, Event, EventKind, Hit, Period, Sample, Shot, VehicleRec};
use livereplay::replay::wire;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let server = args.get(1).map(|s| s.trim_end_matches('/').to_string()).expect("synthlive <server> [seconds]");
    let secs: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120.0);
    let key = std::env::var("SYNTHLIVE_KEY").unwrap_or_else(|_| "synthlive-test-key-0123456789abcdef0123456789".into());
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let map = "99_synth";
    let arena = ArenaDef {
        map: map.into(),
        display_name: "Synthetic".into(),
        bbox_min: [-500.0, -500.0],
        bbox_max: [500.0, 500.0],
        round_length: 900.0,
        gameplay: BTreeMap::from([("ctf".to_string(), GameplayDef::default())]),
        ..Default::default()
    };
    for (name, body) in [
        (map_asset(map), encode_pack(&MapPack { arena, minimap: Vec::new(), layers: Vec::new() })),
        (livereplay::net::ICONS_ASSET.to_string(), encode_pack(&IconPack::default())),
    ] {
        let r = agent.put(&format!("{server}/api/assets/{name}")).query("version", "1").header(KEY_HEADER, &key).send(&body[..]).unwrap();
        println!("asset {name}: {}", r.status());
    }
    let nb = NewBattle { player: "synth_tester".into(), realm: "EU".into(), map: map.into(), map_display: "Synthetic".into(), live: true, arena_id: 4242, ..Default::default() };
    let info: BattleInfo = agent.post(&format!("{server}/api/battles")).header(KEY_HEADER, &key).send_json(&nb).unwrap().body_mut().read_json().unwrap();
    let code: CodeInfo = agent.post(&format!("{server}/api/codes")).header(KEY_HEADER, &key).send_json(NewCode { days: 1 }).unwrap().body_mut().read_json().unwrap();
    println!("battle {server}/b/{}\ncode {}", info.id, code.code);

    let mut b = Battle { map: map.into(), map_display: "Synthetic".into(), gameplay: "ctf".into(), player_name: "synth_tester".into(), server_realm: "EU".into(), live: true, arena_id: 4242, recorder_vehicle: Some(1), ..Default::default() };
    for i in 0..10 {
        b.vehicles.insert(
            i + 1,
            VehicleRec { id: i + 1, name: format!("tank{i}"), team: (i % 2) as u8 + 1, vehicle_type: "x".into(), tank_name: "Tank".into(), tank_short: "T".into(), tier: 10, max_health: 2000, ..Default::default() },
        );
    }
    b.recorder_team = 1;
    b.periods = vec![(0.0, Period::Battle, 900.0)];
    let start = Instant::now();
    let mut sent_to = 0.0f32;
    let mut first = true;
    while start.elapsed().as_secs_f32() < secs {
        // Positions 10 times a second up to now (a real recording grows the same way).
        let now = start.elapsed().as_secs_f32();
        let mut t = b.duration;
        while t < now {
            t += 0.1;
            for (i, v) in b.vehicles.values_mut().enumerate() {
                let a = t * 0.2 + i as f32 * 0.6;
                let r = 100.0 + 25.0 * i as f32;
                v.track.push(Sample { t, pos: [r * a.cos(), 0.0, r * a.sin()], yaw: a + std::f32::consts::FRAC_PI_2, ..Default::default() });
                if v.visibility.is_empty() {
                    v.visibility.push((0.0, true));
                }
            }
        }
        // A shell every second, taking turns: 2 of 4 do damage, 1 bounces, 1 misses.
        while (b.shots.len() as f32) < t - 1.0 {
            let n = b.shots.len() as i32;
            let shooter = n % 10 + 1;
            let target = (n + 3) % 10 + 1;
            let st = b.shots.len() as f32 + 1.0;
            let (from, to) = (b.vehicles[&shooter].track.last().unwrap().pos, b.vehicles[&target].track.last().unwrap().pos);
            let hit = (n % 4 != 3).then_some(target);
            b.shots.push(Shot { t: st, shooter, id: n, start: from, end: to, end_t: st + 0.3, hit, ..Default::default() });
            if let Some(target) = hit {
                b.hits.push(Hit { t: st + 0.3, target, attacker: shooter });
                if n % 4 < 2 {
                    let v = b.vehicles.get_mut(&target).unwrap();
                    let hp = v.health.last().map(|h| h.1).unwrap_or(v.max_health);
                    let amount = 240 + (n % 3) * 30;
                    v.health.push((st + 0.3, (hp - amount).max(1)));
                    b.events.push(Event { t: st + 0.3, kind: EventKind::Damage { attacker: shooter, target, amount } });
                }
            }
        }
        b.duration = t;
        let from = if first { 0.0 } else { (sent_to - 2.0).max(0.0) };
        let q = if first { "?reset=1" } else { "" };
        let r = agent.post(&format!("{server}/api/battles/{}/frames{q}", info.id)).header(KEY_HEADER, &key).send(&wire::encode_frame(&b, from)[..]).unwrap();
        if r.status() != 200 {
            println!("frames: {}", r.status());
        }
        sent_to = b.duration;
        first = false;
        std::thread::sleep(Duration::from_secs(2));
    }
    let _ = agent.post(&format!("{server}/api/battles/{}/frames?finish=1", info.id)).header(KEY_HEADER, &key).send(&[][..]);
    println!("finished");
}
