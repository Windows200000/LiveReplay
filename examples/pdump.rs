//! Developer tool: dump packets of given kinds (hex) with times: `pdump <replay> 31,32 [from] [to]`
use livereplay::replay::file::{packets, ReplayFile};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let kinds: Vec<u32> = a[2].split(',').filter_map(|k| u32::from_str_radix(k, 16).ok()).collect();
    let from: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let to: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(1e9);
    let rf = ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    for p in packets(&s) {
        if kinds.contains(&(p.kind as u32)) && p.time >= from && p.time <= to {
            let hex: String = p.payload.iter().take(48).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            println!("{:7.2} {:02x} len {:4} {hex}", p.time, p.kind, p.payload.len());
        }
    }
}
