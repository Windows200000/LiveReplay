//! Count packet types in a replay stream: `ptypes <replay>`
use livereplay::replay::file;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rf = file::ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    let mut c: std::collections::BTreeMap<u32, (usize, usize, f32)> = Default::default();
    for p in file::packets(&s) {
        let e = c.entry(p.kind).or_insert((0, 0, p.time));
        e.0 += 1;
        e.1 += p.payload.len();
    }
    for (k, (n, bytes, t)) in c {
        println!("type 0x{k:02x}: {n:6} packets, {bytes:8} bytes, first at {t:.1}");
    }
}
