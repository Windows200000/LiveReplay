//! Developer tool: count entity method calls in a replay by entity type and method name, and print
//! the decoded arguments of methods whose name contains a filter word.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::defs::{Cursor, Definitions};
use livereplay::replay::file::{packets, ReplayFile};
use std::collections::{BTreeMap, HashMap};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let filter = args.get(2).cloned().unwrap_or_default().to_lowercase();
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd).unwrap();
    let rf = ReplayFile::open(std::path::Path::new(&args[1])).unwrap();
    let stream = rf.packet_stream().unwrap();
    let mut types: HashMap<i32, u16> = HashMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut shown = 0;
    for p in packets(&stream) {
        let mut c = Cursor::new(p.payload);
        match p.kind {
            0x00 | 0x05 => { if let (Some(id), Some(ty)) = (c.i32(), c.i16()) { types.insert(id, ty as u16); } }
            0x08 => {
                let (Some(id), Some(mid), Some(_)) = (c.i32(), c.u32(), c.u32()) else { continue };
                let Some(&ty) = types.get(&id) else { continue };
                let Some(e) = defs.by_type(ty) else { continue };
                let Some(m) = e.client_methods.get(mid as usize) else { continue };
                *counts.entry(format!("{}.{}", e.name, m.name)).or_default() += 1;
                if !filter.is_empty() && m.name.to_lowercase().contains(&filter) && shown < 100000 {
                    shown += 1;
                    println!("{:7.1} {} {} {:?}", p.time, id, m.name, m.decode(c.rest()));
                }
            }
            _ => {}
        }
    }
    for (k, v) in counts { println!("{v:6} {k}"); }
}
