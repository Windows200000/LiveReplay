//! Developer tool: list entity creations / method calls / script events in a time window.
//! `probe <replay> <from> <to>`
use livereplay::gamedata::{find_game_dir, GameData};
use livereplay::replay::{defs::{Cursor, Definitions}, file};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (from, to): (f32, f32) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let gd = GameData::new(find_game_dir("").unwrap());
    let d = Definitions::load(&gd).unwrap();
    let rf = file::ReplayFile::open(std::path::Path::new(&a[1])).unwrap();
    let s = rf.packet_stream().unwrap();
    let mut types: std::collections::HashMap<i32, u16> = Default::default();
    for p in file::packets(&s) {
        let mut c = Cursor::new(p.payload);
        if p.kind == 0x05 || p.kind == 0x00 {
            if let (Some(id), Some(ty)) = (c.i32(), c.i16()) { types.insert(id, ty as u16); }
        }
        if p.time < from || p.time > to { continue }
        let mut c = Cursor::new(p.payload);
        match p.kind {
            0x05 => { let (id, ty) = (c.i32().unwrap_or(0), c.i16().unwrap_or(0)); let n = d.entities.get(ty as usize - 1).map(|e| e.name.clone()).unwrap_or_default(); if n != "Vehicle" { println!("t={:6.1} create {n} {id}", p.time); } }
            0x08 => {
                let (Some(id), Some(mid)) = (c.i32(), c.u32()) else { continue };
                let Some(&ty) = types.get(&id) else { continue };
                let Some(e) = d.entities.get(ty as usize - 1) else { continue };
                if e.name == "Vehicle" { continue }
                let m = e.client_methods.get(mid as usize).map(|m| m.name.clone()).unwrap_or_default();
                if std::env::var_os("PROBE_ALL").is_none() && ["showTracer", "stopTracer", "explodeProjectile", "updateArena", "onRoundFinished", "showOwnVehicleHitDirection", "updatePositions", "updateVehicleHealth", "updateVehicleAmmo", "onFrictionWithVehicle", "showOtherVehicleDamagedDevices", "updateTargetingInfo", "updateGunMarker", "updateOwnVehiclePosition", "onSwitchViewpoint", "updateVehicleOptionalDeviceStatus", "updateVehicleSetting", "updateVehicleMiscStatus", "onCollisionWithVehicle", "updateVehicleGunReloadTime", "updateVehicleClipReloadTime", "showDevelopmentInfo", "updateVehicleQuickShellChanger", "updateDualGunState"].contains(&m.as_str()) { continue }
                let arg = e.client_methods.get(mid as usize).and_then(|mm| mm.decode(&p.payload[12..])).map(|v| format!("{v:?}")).unwrap_or_default();
                println!("t={:6.1} {}.{} {}", p.time, e.name, m, arg.chars().take(160).collect::<String>());
            }
            0x07 => {
                let (Some(id), Some(mid)) = (c.i32(), c.u32()) else { continue };
                let Some(&ty) = types.get(&id) else { continue };
                let Some(e) = d.entities.get(ty as usize - 1) else { continue };
                if e.name == "Vehicle" { continue }
                let pn = e.client_props.get(mid as usize).map(|m| m.name.clone()).unwrap_or_default();
                println!("t={:6.1} {}.prop {}", p.time, e.name, pn);
            }
            0x31 => println!("t={:6.1} script {}", p.time, String::from_utf8_lossy(p.payload).chars().filter(|c| !c.is_control()).take(160).collect::<String>()),
            0x04 | 0x06 => {}
            k => if k != 0x0a && k != 0x07 { }
        }
    }
}
