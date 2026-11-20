//! Check 0x37 kill records against results lifetimes: `killcheck <replay>...`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions, file};
/// Kill records in a state-update packet (type 0x37): a record holding the pickled
/// `{'deathOrder': n}` is followed by a 16-byte record of (3 path bytes, victim id, killer id, ...),
/// both with the same 3-byte record header. Byte values of the header differ between battles,
/// so only the structure is matched.
fn kill_records(d: &[u8]) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let needle = b"deathOrder";
    let mut from = 0;
    while let Some(j) = d.get(from..).and_then(|s| s.windows(needle.len()).position(|w| w == needle)).map(|p| p + from) {
        from = j + needle.len();
        // Start of the record containing the pickle: header (3 bytes) + length 0x1a.
        let Some(r) = (j.saturating_sub(40)..j).rev().find(|&r| d.get(r + 3) == Some(&0x1a) && r + 30 + 4 <= d.len() && d[r..r + 3] == d[r + 30..r + 33]) else { continue };
        let n = r + 30;
        if d.get(n + 3) != Some(&0x10) || d.len() < n + 4 + 11 {
            continue;
        }
        let id = |o: usize| i32::from_le_bytes(d[o..o + 4].try_into().unwrap());
        out.push((id(n + 7), id(n + 11)));
    }
    out
}

fn main() -> anyhow::Result<()> {
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd)?;
    let (mut ok, mut bad, mut missing, mut extra) = (0, 0, 0, 0);
    for path in std::env::args().skip(1) {
        let Ok(b) = Battle::load(std::path::Path::new(&path), &gd, &defs) else { continue };
        let rf = file::ReplayFile::open(std::path::Path::new(&path))?;
        let s = rf.packet_stream()?;
        let mut kills: std::collections::HashMap<i32, (f32, i32)> = Default::default();
        for p in file::packets(&s).filter(|p| p.kind == 0x37) {
            for (v, k) in kill_records(p.payload) {
                kills.entry(v).or_insert((p.time, k));
            }
        }
        let start = b.battle_start();
        for v in b.vehicles.values() {
            let f = v.final_stats.as_ref();
            let reason = f.and_then(|s| s.get("deathReason")).and_then(|x| x.as_i64()).unwrap_or(-2);
            let life = f.and_then(|s| s.get("lifeTime")).and_then(|x| x.as_f64()).unwrap_or(-1.0) as f32;
            match (reason >= 0, kills.get(&v.id)) {
                (true, Some((t, _))) => if ((t - start) - life).abs() < 3.0 { ok += 1 } else { bad += 1; println!("{path}: {} t {} vs life {life}", v.tank_short, t - start) },
                (true, None) => { if reason != -2 && start + life < b.duration - 1.0 { missing += 1; println!("{path}: missing kill {} life {life} reason {reason}", v.tank_short) } }
                (false, Some(_)) => { if reason == -1 { extra += 1; println!("{path}: extra kill {}", v.tank_short) } }
                (false, None) => {}
            }
        }
    }
    println!("ok {ok} bad {bad} missing {missing} extra {extra}");
    Ok(())
}
