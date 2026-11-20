//! Developer tool: decode a stored battle file (server data) and print a summary.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let d = std::fs::read(p).unwrap();
    let (chunks, used) = livereplay::replay::wire::decode_frames(&d);
    println!("{} bytes, {} frames decoded, {used} used", d.len(), chunks.len());
    if let Some(b) = livereplay::replay::wire::decode_battle(&d) {
        println!("map {} vehicles {} duration {} recorder {:?} viewpoints {:?}", b.map, b.vehicles.len(), b.duration, b.recorder_vehicle, b.viewpoints);
    }
}
