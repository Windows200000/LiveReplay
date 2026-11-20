//! Check the sign convention of recorded hull pitch/roll against the terrain slope: `orient <replay>`
use livereplay::gamedata::{cache3d::Cache3D, find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let gd = GameData::new(find_game_dir("").expect("game dir"));
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&path), &gd, &defs)?;
    let terrain = Cache3D::new(&gd).unwrap().terrain(&gd, &b.map).unwrap();
    let h = |x: f32, z: f32| terrain.height_at(x, z);
    let (mut pp, mut pn, mut rp, mut rn) = (0, 0, 0, 0);
    let (mut bias_p, mut bias_r, mut n) = (0.0f64, 0.0f64, 0usize);
    for v in b.vehicles.values() {
        for s in &v.track {
            let (sn, cs) = s.yaw.sin_cos();
            let [x, _, z] = s.pos;
            let (l, w) = (2.5, 1.5);
            let fwd = ((h(x + sn * l, z + cs * l) - h(x - sn * l, z - cs * l)) / (2.0 * l)).atan();
            let right = ((h(x + cs * w, z - sn * w) - h(x - cs * w, z + sn * w)) / (2.0 * w)).atan();
            if s.pitch != 0.0 || s.roll != 0.0 {
                bias_p += ((-s.pitch) - fwd).to_degrees() as f64;
                bias_r += (s.roll - right).to_degrees() as f64;
                n += 1;
            }
            if fwd.abs() > 0.06 && s.pitch.abs() > 0.03 {
                if fwd.signum() == s.pitch.signum() { pp += 1 } else { pn += 1 }
            }
            if right.abs() > 0.06 && s.roll.abs() > 0.03 {
                if right.signum() == s.roll.signum() { rp += 1 } else { rn += 1 }
            }
        }
    }
    println!("pitch vs uphill-forward slope: same sign {pp}, opposite {pn}");
    println!("mean (nose-up pitch - terrain slope) {:.2}°, mean (roll - side slope) {:.2}° over {n}", bias_p / n as f64, bias_r / n as f64);
    println!("roll vs right-side-higher slope: same sign {rp}, opposite {rn}");
    Ok(())
}
