//! Developer tool: list NetworkEntity (map scenario prefab) creations and their properties over time.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{defs::{Cursor, Definitions}, file};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").unwrap());
    let d = Definitions::load(&gd).unwrap();
    let ne_type = d.entities.iter().position(|e| e.name == "NetworkEntity").unwrap() as u16 + 1;
    let ne = d.by_name("NetworkEntity").unwrap();
    println!("NetworkEntity props: {:?}", ne.client_props.iter().map(|p| p.name.clone()).collect::<Vec<_>>());
    println!("NetworkEntity methods: {:?}", ne.client_methods.iter().map(|m| m.name.clone()).collect::<Vec<_>>());
    let rf = file::ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    let mut ids = std::collections::HashMap::new();
    for p in file::packets(&s) {
        let mut c = Cursor::new(p.payload);
        match p.kind {
            0x05 => {
                let (Some(id), Some(ty)) = (c.i32(), c.i16()) else { continue };
                if ty as u16 != ne_type { continue }
                c.take(12 + 24); c.u32();
                let cnt = c.u8().unwrap_or(0);
                let mut props = Vec::new();
                for _ in 0..cnt {
                    let Some(i) = c.u8() else { break };
                    let Some(pr) = ne.client_props.get(i as usize) else { break };
                    let v = c.read(&pr.ty);
                    let txt = match &v { Some(livereplay::replay::defs::Val::Bytes(b)) => String::from_utf8_lossy(b).into_owned(), o => format!("{o:?}") };
                    props.push(format!("{}={}", pr.name, txt.chars().take(80).collect::<String>()));
                }
                ids.insert(id, ());
                println!("t={:7.1} create {id} {}", p.time, props.join(" | "));
            }
            0x04 => { if let Some(id) = c.i32() { if ids.contains_key(&id) { println!("t={:7.1} leave  {id}", p.time); } } }
            0x07 | 0x08 => {
                let (Some(id), Some(mid)) = (c.i32(), c.u32()) else { continue };
                if ids.contains_key(&id) {
                    let name = if p.kind == 8 { ne.client_methods.get(mid as usize).map(|m| m.name.clone()) } else { ne.client_props.get(mid as usize).map(|m| m.name.clone()) };
                    println!("t={:7.1} {} {id} {:?}", p.time, if p.kind == 8 { "method" } else { "prop" }, name);
                }
            }
            _ => {}
        }
    }
}
