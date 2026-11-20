//! 0x37 packets containing a byte string: `p37str <replay> <text>` (hex dump of the whole packet)
use livereplay::replay::file;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let needle = a[2].as_bytes();
    let rf = file::ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    for p in file::packets(&s).filter(|p| p.kind == 0x37 && p.payload.windows(needle.len()).any(|w| w == needle)) {
        println!("t={:6.1} len={}", p.time, p.payload.len());
        for (i, ch) in p.payload.chunks(32).enumerate() {
            let hex: String = ch.iter().map(|b| format!("{b:02x} ")).collect();
            let txt: String = ch.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
            println!("  {:04x}: {hex:<96} {txt}", i * 32);
        }
    }
}
