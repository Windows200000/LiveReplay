//! Dump packets of one type: `p37 <replay> <type hex> [max]`
use livereplay::replay::file;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let kind = u32::from_str_radix(&a[2], 16).unwrap();
    let max: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);
    let rf = file::ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    for p in file::packets(&s).filter(|p| p.kind == kind).take(max) {
        let hex: String = p.payload.iter().take(80).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
        let txt: String = p.payload.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
        println!("t={:6.1} len={:4} {hex}\n            {}", p.time, p.payload.len(), txt.chars().take(160).collect::<String>());
    }
}
