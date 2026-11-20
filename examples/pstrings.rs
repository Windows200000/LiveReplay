//! Developer tool: packet kinds in a replay and the readable strings inside packets of one kind.
use livereplay::replay::file::{packets, ReplayFile};
use std::collections::BTreeMap;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let kind = a.get(2).and_then(|k| u32::from_str_radix(k.trim_start_matches("0x"), 16).ok());
    let rf = ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    let mut kinds: BTreeMap<u32, usize> = BTreeMap::new();
    let mut strings: BTreeMap<String, usize> = BTreeMap::new();
    for p in packets(&s) {
        *kinds.entry(p.kind as u32).or_default() += 1;
        if Some(p.kind as u32) == kind {
            let mut cur = String::new();
            for &b in p.payload.iter().chain(std::iter::once(&0u8)) {
                if b.is_ascii_alphabetic() || b == b'_' { cur.push(b as char) } else { if cur.len() >= 4 { *strings.entry(cur.clone()).or_default() += 1 } cur.clear() }
            }
        }
    }
    println!("{kinds:x?}");
    let mut v: Vec<_> = strings.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, n) in v.iter().take(60) { println!("{n:6} {k}"); }
}
