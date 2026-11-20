//! Developer tool: the named client events (packet 0x31) in a replay, with decoded pickled args.
//! `clientevents <replay> [name filter]`
use livereplay::replay::file::{packets, ReplayFile};
use livereplay::replay::pickle;
use std::collections::BTreeMap;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let filter = a.get(2).cloned().unwrap_or_default();
    let rf = ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    for p in packets(&s) {
        if p.kind != 0x31 { continue }
        let d = p.payload;
        let n = u32::from_le_bytes(d[0..4].try_into().unwrap()) as usize;
        let name = String::from_utf8_lossy(&d[4..4 + n]).into_owned();
        *names.entry(name.clone()).or_default() += 1;
        if !filter.is_empty() && name.contains(&filter) {
            let m = u32::from_le_bytes(d[4 + n..8 + n].try_into().unwrap()) as usize;
            println!("{:7.2} {name} {:?}", p.time, pickle::loads(&d[8 + n..8 + n + m]));
        }
    }
    if filter.is_empty() { for (k, v) in names { println!("{v:6} {k}"); } }
}
