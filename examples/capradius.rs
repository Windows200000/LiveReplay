//! Distance of each tank to the control point / bases at given times: `capradius <replay> <t>...`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{battle::Battle, defs::Definitions};
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let gd = GameData::new(find_game_dir("").unwrap());
    let defs = Definitions::load(&gd)?;
    let b = Battle::load(std::path::Path::new(&a[1]), &gd, &defs)?;
    let arena = gd.arena(&b.map).unwrap();
    let gp = arena.gameplay.get(&b.gameplay).cloned().unwrap();
    let mut pts: Vec<[f32; 2]> = gp.control_points.clone();
    for t in &gp.bases { pts.extend(t.iter().copied()); }
    for ts in &a[2..] {
        let t: f32 = ts.parse()?;
        for p in &pts {
            let mut d: Vec<(f32, String)> = b.vehicles.values().filter_map(|v| {
                let s = v.track.iter().rev().find(|s| s.t <= t)?;
                if t - s.t > 1.0 { return None; }
                Some((((s.pos[0] - p[0]).powi(2) + (s.pos[2] - p[1]).powi(2)).sqrt(), format!("{} t{}", v.tank_short, v.team)))
            }).collect();
            d.sort_by(|x, y| x.0.total_cmp(&y.0));
            println!("t={t} point {p:?}: {:?}", d.iter().take(3).map(|(x, n)| format!("{n} {x:.1}m")).collect::<Vec<_>>());
        }
    }
    Ok(())
}
