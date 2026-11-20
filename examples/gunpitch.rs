//! Gun pitch calibration check: per vehicle, fitted code->pitch and the error at its shots.
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};
use livereplay::viewer::scene::gun_pitch_at;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&path), &gd, &defs)?;
    for v in b.vehicles.values() {
        if v.shot_pitch.is_empty() {
            continue;
        }
        let errs: Vec<f32> = v.shot_pitch.iter().map(|&(t, p)| (gun_pitch_at(v, t - 0.05) - p).to_degrees().abs()).collect();
        let mean = errs.iter().sum::<f32>() / errs.len() as f32;
        let fit = v.gun_pitch_fit.map(|(a, b)| format!("{:.1}° {:+.2}°/step", a.to_degrees(), b.to_degrees())).unwrap_or("none".into());
        println!("{:14} shots {:3} fit {fit:22} mean |err| {mean:4.1}°  pitches {:?}", v.tank_short, errs.len(), v.shot_pitch.iter().take(6).map(|p| (p.1.to_degrees() * 10.0).round() / 10.0).collect::<Vec<_>>());
    }
    Ok(())
}
