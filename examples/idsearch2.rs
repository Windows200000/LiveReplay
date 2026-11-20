//! Developer tool: packets (by kind) in a time range whose payload contains an entity id (LE i32).
use livereplay::replay::file::{packets, ReplayFile};
use std::collections::BTreeMap;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let id: i32 = a[2].parse().unwrap();
    let (from, to): (f32, f32) = (a[3].parse().unwrap(), a[4].parse().unwrap());
    let needle = id.to_le_bytes();
    let rf = ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    let mut kinds: BTreeMap<u32, usize> = BTreeMap::new();
    let mut shown = 0;
    for p in packets(&s) {
        if p.time < from || p.time > to { continue }
        if let Some(i) = p.payload.windows(4).position(|w| w == needle) {
            *kinds.entry(p.kind as u32).or_default() += 1;
            if shown < 12 && ![0x0a, 0x07].contains(&p.kind) {
                shown += 1;
                let hex: String = p.payload.iter().skip(i.saturating_sub(8)).take(48).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
                println!("{:7.2} {:02x} at {i} len {} {hex}", p.time, p.kind, p.payload.len());
            }
        }
    }
    println!("{kinds:x?}");
}
