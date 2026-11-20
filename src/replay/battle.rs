//! Turns a decoded packet stream into a renderer-agnostic battle timeline:
//! per-vehicle position tracks, visibility, health and a list of combat events.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;

use super::defs::{Cursor, Definitions, Val};
use super::file::{packets, ReplayFile};
use super::pickle;
use crate::gamedata::{GameData, VehicleClass};

#[derive(Debug, Clone, Default, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Sample {
    pub t: f32,
    /// World position (x = east, y = up, z = north).
    pub pos: [f32; 3],
    pub yaw: f32,
    /// Hull pitch and roll as sent by the server (radians).
    #[serde(default)]
    pub pitch: f32,
    #[serde(default)]
    pub roll: f32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VehicleRec {
    pub id: i32,
    pub name: String,
    pub clan: String,
    pub account_id: i64,
    pub team: u8,
    pub vehicle_type: String,
    pub tank_name: String,
    pub tank_short: String,
    pub tier: u8,
    pub class: Option<VehicleClass>,
    pub max_health: i32,
    #[serde(with = "super::wire::track")]
    pub track: Vec<Sample>,
    /// AoI enter/leave transitions (time, in view).
    pub visibility: Vec<(f32, bool)>,
    /// Known health values over time.
    pub health: Vec<(f32, i32)>,
    /// Turret yaw relative to the hull (radians) over time (derived from `gun_codes`, not stored).
    #[serde(skip)]
    pub turret_yaw: Vec<(f32, f32)>,
    /// Raw `gunAnglesPacked` values over time (low 6 bits = gun pitch, per-gun scale).
    #[serde(default, with = "super::wire::codes")]
    pub gun_codes: Vec<(f32, u16)>,
    /// Gun pitch relative to the hull (radians, + = up) = a + b * (code & 63), calibrated from
    /// this vehicle's shots. None when it fired too little to calibrate.
    #[serde(default)]
    pub gun_pitch_fit: Option<(f32, f32)>,
    /// Hull-relative gun pitch at each of its shots (fallback around shot times).
    #[serde(default)]
    pub shot_pitch: Vec<(f32, f32)>,
    pub death_time: Option<f32>,
    /// End-of-battle stats from the results block (complete replays only).
    #[serde(with = "super::wire::json_text")]
    pub final_stats: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum EventKind {
    Shot { shooter: i32 },
    Damage { attacker: i32, target: i32, amount: i32 },
    Kill { killer: i32, victim: i32 },
}

/// A shell in flight, from the tracer the server sent to the recording client.
#[derive(Debug, Clone, Default, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Shot {
    pub t: f32,
    pub shooter: i32,
    pub id: i32,
    /// World positions (x, y, z).
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// Time the shell landed (or best estimate).
    pub end_t: f32,
    /// Vehicle the shell hit, if any.
    pub hit: Option<i32>,
    /// Muzzle velocity and gravity from the tracer: the path is
    /// `start + vel*t + (0, -gravity*t^2/2, 0)` (matches the recorded impact points).
    #[serde(default)]
    pub vel: [f32; 3],
    #[serde(default)]
    pub gravity: f32,
}

impl Shot {
    /// Point on the shell's path at fraction `u` (0 = muzzle, 1 = impact) of the horizontal
    /// distance. Follows the ballistic arc and lands exactly on `end`.
    pub fn point(&self, u: f32) -> [f32; 3] {
        let x = self.start[0] + (self.end[0] - self.start[0]) * u;
        let z = self.start[2] + (self.end[2] - self.start[2]) * u;
        let vh = (self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2]).sqrt();
        if vh < 1.0 || self.gravity <= 0.0 {
            return [x, self.start[1] + (self.end[1] - self.start[1]) * u, z];
        }
        let d = ((self.end[0] - self.start[0]).powi(2) + (self.end[2] - self.start[2]).powi(2)).sqrt();
        let arc_y = |tau: f32| self.start[1] + self.vel[1] * tau - 0.5 * self.gravity * tau * tau;
        let total = d / vh;
        // Arc, with the (small) mismatch at the end spread linearly so it ends on the impact point.
        let y = arc_y(total * u) + (self.end[1] - arc_y(total)) * u;
        [x, y, z]
    }

    /// How far the shell sags below a straight line over its flight (picks the line detail).
    pub fn sag(&self) -> f32 {
        let vh = (self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2]).sqrt();
        if vh < 1.0 {
            return 0.0;
        }
        let d = ((self.end[0] - self.start[0]).powi(2) + (self.end[2] - self.start[2]).powi(2)).sqrt();
        let t = d / vh;
        self.gravity * t * t / 8.0
    }
}

/// Hull axes (right, up, forward) in world space from the server's yaw / pitch / roll.
/// Server pitch is positive nose-down, roll positive right-side-up.
pub fn hull_axes(yaw: f32, pitch: f32, roll: f32) -> [[f32; 3]; 3] {
    let (s, c) = yaw.sin_cos();
    let (st, ct) = (-pitch).sin_cos();
    let (sr, cr) = roll.sin_cos();
    let fwd = [s * ct, st, c * ct];
    let up0 = [-s * st, ct, -c * st];
    let right0 = [c, 0.0, -s];
    let right = [right0[0] * cr + up0[0] * sr, right0[1] * cr + up0[1] * sr, right0[2] * cr + up0[2] * sr];
    let up = [up0[0] * cr - right0[0] * sr, up0[1] * cr - right0[1] * sr, up0[2] * cr - right0[2] * sr];
    [right, up, fwd]
}

/// Height of the ballistic path after travelling `dist` metres horizontally.
fn arc_height(start: [f32; 3], vel: [f32; 3], g: f32, dist: f32) -> f32 {
    let vh = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt().max(1.0);
    let tau = dist / vh;
    start[1] + vel[1] * tau - 0.5 * g * tau * tau
}

/// A vehicle taking a shell hit (used for the white hit flash).
#[derive(Debug, Clone, Default, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Hit {
    pub t: f32,
    pub target: i32,
    pub attacker: i32,
}

/// A map-scenario object (live map event state such as `08_bigCityBombing_cornerBuilding_02_normal.prefab`)
/// and the time span it existed in the battle.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ScenarioObject {
    pub prefab: String,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub created: f32,
    pub removed: Option<f32>,
}

/// Capture state of one base at one moment.
#[derive(Debug, Clone, Default, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CaptureSample {
    pub t: f32,
    /// Team owning the base (the other team is the one capturing it).
    pub team: u8,
    pub base_id: u8,
    /// 0..100
    pub points: u8,
    pub invaders: u8,
    pub stopped: bool,
    /// Seconds until captured at the current rate.
    #[serde(default)]
    pub time_left: u16,
}

impl Battle {
    /// Where a base is: the owner team's base, or the neutral control point (Encounter).
    pub fn base_pos(&self, team: u8, base_id: u8) -> Option<[f32; 2]> {
        let own = self.team_bases.get((team as usize).wrapping_sub(1)).filter(|v| !v.is_empty());
        let list = own.unwrap_or(&self.control_points);
        list.get((base_id as usize).saturating_sub(1)).or(list.first()).copied()
    }
}

/// A random event (WG "random events": airship crash, train crash, city bombing, ...).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RandomEvent {
    /// Scenario name without map prefix and suffix, e.g. `airshipCrash01`.
    pub name: String,
    /// When the game announced it (danger zone shown from here).
    pub warned: f32,
    /// First and last moment something of it broke / appeared destroyed.
    pub start: Option<f32>,
    pub end: Option<f32>,
    /// Where it happens (centre of its destroyed objects, x/z).
    pub pos: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub t: f32,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Period {
    Waiting,
    Prebattle,
    Battle,
    AfterBattle,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Battle {
    pub map: String,
    pub map_display: String,
    pub gameplay: String,
    pub battle_type: i64,
    pub player_name: String,
    pub recorder_vehicle: Option<i32>,
    /// Tanks of other players whose recordings were merged in (web viewer: several viewpoints).
    #[serde(default)]
    pub viewpoints: Vec<i32>,
    pub recorder_team: u8,
    pub server_realm: String,
    pub vehicles: BTreeMap<i32, VehicleRec>,
    pub events: Vec<Event>,
    pub shots: Vec<Shot>,
    pub hits: Vec<Hit>,
    /// Numeric gameplay type (arenaTypeID >> 16): 0 = standard, 1 = encounter, 2 = assault, ...
    pub gameplay_id: i32,
    pub scenario: Vec<ScenarioObject>,
    #[serde(default)]
    pub random_events: Vec<RandomEvent>,
    pub duration: f32,
    /// (replay time the period started, period, length in seconds)
    pub periods: Vec<(f32, Period, f32)>,
    /// Base capture state over time (arena update BASE_POINTS).
    #[serde(default)]
    pub captures: Vec<CaptureSample>,
    /// Capture circle radius (m), estimated from when tanks entered it; 50 if nobody capped.
    #[serde(default)]
    pub capture_radius: f32,
    /// Base positions per owning team (Standard / Assault) and neutral control points (Encounter).
    #[serde(default)]
    pub team_bases: [Vec<[f32; 2]>; 2],
    #[serde(default)]
    pub control_points: Vec<[f32; 2]>,
    pub winner_team: Option<u8>,
    /// Loaded from an in-progress `temp.wotreplay`.
    pub live: bool,
}

/// Clock state shown at the top of the minimap.
pub struct Clock {
    pub period: Period,
    /// Seconds remaining in the current period, if it has a timer.
    pub remaining: Option<f32>,
}

impl Battle {
    pub fn clock(&self, t: f32) -> Clock {
        let cur = self.periods.iter().rev().find(|(start, _, _)| *start <= t).or(self.periods.first());
        match cur {
            Some(&(start, period, length)) => Clock {
                period,
                remaining: match period {
                    Period::Prebattle | Period::Battle => Some((length - (t - start)).max(0.0)),
                    _ => None,
                },
            },
            None => Clock { period: Period::Battle, remaining: None },
        }
    }

    pub fn battle_start(&self) -> f32 {
        self.periods.iter().find(|p| p.1 == Period::Battle).map(|p| p.0).unwrap_or(0.0)
    }
}

/// Fit each vehicle's 6-bit gun pitch code to real angles: at every shot the shell's launch
/// direction, seen from the hull, is the gun's pitch.
fn calibrate_gun_pitch(b: &mut Battle) {
    let shots = b.shots.clone();
    for v in b.vehicles.values_mut() {
        let mut pts: Vec<(f32, f32)> = Vec::new();
        for s in shots.iter().filter(|s| s.shooter == v.id) {
            let speed = (s.vel[0] * s.vel[0] + s.vel[1] * s.vel[1] + s.vel[2] * s.vel[2]).sqrt();
            if speed < 1.0 {
                continue;
            }
            let Some(i) = v.track.iter().rposition(|p| p.t <= s.t) else { continue };
            let p = v.track[i];
            let [right, up, fwd] = hull_axes(p.yaw, p.pitch, p.roll);
            let d = [s.vel[0] / speed, s.vel[1] / speed, s.vel[2] / speed];
            let dot = |a: [f32; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
            let rel = dot(up).atan2((dot(right).powi(2) + dot(fwd).powi(2)).sqrt());
            v.shot_pitch.push((s.t, rel));
            // Only codes sent just before the shot describe the gun at that moment.
            if let Some(&(ct, code)) = v.gun_codes.iter().rev().find(|c| c.0 <= s.t) {
                if s.t - ct < 1.0 {
                    pts.push(((code & 63) as f32, rel));
                }
            }
        }
        let n = pts.len() as f32;
        if pts.len() < 2 {
            continue;
        }
        let (mx, my) = (pts.iter().map(|p| p.0).sum::<f32>() / n, pts.iter().map(|p| p.1).sum::<f32>() / n);
        let sxx: f32 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f32 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        // Need a spread of codes; the gun lowers as the code rises.
        if sxx < 4.0 * n {
            continue;
        }
        let slope = sxy / sxx;
        if slope < 0.0 && slope > -0.05 {
            v.gun_pitch_fit = Some((my - slope * mx, slope));
        }
    }
}

/// `gunAnglesPacked`: turret yaw in the top 10 bits (full circle), gun pitch in the low 6.
pub(crate) fn decode_turret_yaw(code: u16) -> f32 {
    let yaw = ((code >> 6) & 1023) as f32;
    std::f32::consts::TAU * yaw / 1023.0 - std::f32::consts::PI
}

/// `arenaTypeID >> 16` → gameplay name (ArenaType.GAMEPLAY_NAMES order).
fn gameplay_name(id: i32) -> &'static str {
    match id {
        1 => "domination",
        2 => "assault",
        3 => "nations",
        4 => "ctf2",
        5 => "domination2",
        6 => "assault2",
        _ => "ctf",
    }
}

/// Placeholder team for tanks first seen in an enemy info record (resolved after loading).
const TEAM_FROM_INFO: u8 = 255;

const NATIONS: [&str; 11] = ["ussr", "germany", "usa", "china", "france", "uk", "japan", "czech", "sweden", "poland", "italy"];

fn vehicle_type_from_compdescr(gd: &GameData, cd: &[u8]) -> Option<String> {
    // header: nation << 4 | item type (vehicle = 1), bit 1 set = vehicle id is 2 bytes wide.
    let header = *cd.first()?;
    if header & 0x01 == 0 {
        return None;
    }
    let nation = *NATIONS.get((header >> 4) as usize)?;
    let id = if header & 0x02 != 0 { u16::from_le_bytes([*cd.get(1)?, *cd.get(2)?]) } else { *cd.get(1)? as u16 };
    gd.vehicle_tag_by_id(nation, id).map(|tag| format!("{nation}:{tag}"))
}

fn json_str(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

impl Battle {
    pub fn load(path: &std::path::Path, gd: &GameData, defs: &Definitions) -> anyhow::Result<Self> {
        let rf = ReplayFile::open(path)?;
        let si = rf.start_info().clone();
        let mut b = Battle {
            map: json_str(&si, "mapName"),
            map_display: json_str(&si, "mapDisplayName"),
            gameplay: json_str(&si, "gameplayID"),
            battle_type: si.get("battleType").and_then(|v| v.as_i64()).unwrap_or(0),
            player_name: json_str(&si, "playerName"),
            server_realm: json_str(&si, "regionCode"),
            ..Default::default()
        };

        let add_roster = |b: &mut Battle, id: i32, v: &serde_json::Value| {
            let rec = b.vehicles.entry(id).or_insert_with(|| VehicleRec { id, ..Default::default() });
            if rec.name.is_empty() {
                rec.name = json_str(v, "name");
            }
            if rec.clan.is_empty() {
                rec.clan = json_str(v, "clanAbbrev");
            }
            if let Some(t) = v.get("team").and_then(|x| x.as_u64()) {
                rec.team = t as u8;
            }
            if rec.vehicle_type.is_empty() {
                rec.vehicle_type = json_str(v, "vehicleType");
            }
            if let Some(h) = v.get("maxHealth").and_then(|x| x.as_i64()) {
                rec.max_health = h as i32;
            }
            if let Some(a) = v.get("accountDBID").and_then(|x| x.as_i64()) {
                rec.account_id = a;
            }
        };

        // Debug: behave like a live battle (no roster / results blocks).
        let like_live = std::env::var_os("LIVEREPLAY_LIKE_LIVE").is_some();
        if let Some(vs) = si.get("vehicles").and_then(|v| v.as_object()).filter(|_| !like_live) {
            for (k, v) in vs {
                if let Ok(id) = k.parse() {
                    add_roster(&mut b, id, v);
                }
            }
        }
        // Results block: [results, vehicles (full roster), frags]
        if let Some(res) = rf.results().and_then(|r| r.as_array()).filter(|_| !like_live) {
            if let Some(vs) = res.get(1).and_then(|v| v.as_object()) {
                for (k, v) in vs {
                    if let Ok(id) = k.parse() {
                        add_roster(&mut b, id, v);
                    }
                }
            }
            if let Some(r) = res.first() {
                let players = r.get("players").cloned().unwrap_or_default();
                if let Some(vs) = r.get("vehicles").and_then(|v| v.as_object()) {
                    for (k, list) in vs {
                        let (Ok(id), Some(stats)) = (k.parse::<i32>(), list.as_array().and_then(|l| l.first())) else { continue };
                        let rec = b.vehicles.entry(id).or_insert_with(|| VehicleRec { id, ..Default::default() });
                        rec.final_stats = Some(stats.clone());
                        if let Some(a) = stats.get("accountDBID").and_then(|x| x.as_i64()) {
                            rec.account_id = a;
                            if rec.clan.is_empty() {
                                rec.clan = players.get(a.to_string()).map(|p| json_str(p, "clanAbbrev")).unwrap_or_default();
                            }
                        }
                    }
                }
                if let Some(w) = r.get("common").and_then(|c| c.get("winnerTeam")).and_then(|w| w.as_u64()) {
                    b.winner_team = Some(w as u8);
                }
            }
        }

        let stream = rf.packet_stream()?;
        let avatar = defs.by_name("Avatar");
        let vehicle_def = defs.by_name("Vehicle");
        let vehicle_type_id = defs.entities.iter().position(|e| e.name == "Vehicle").map(|i| i as u16 + 1);
        let network_type_id = defs.entities.iter().position(|e| e.name == "NetworkEntity").map(|i| i as u16 + 1);
        let network_def = defs.by_name("NetworkEntity");
        let mut scenario_ids: HashMap<i32, usize> = HashMap::new();
        let m_update_arena = avatar.and_then(|a| a.method_index("updateArena"));
        let m_avatar_health = avatar.and_then(|a| a.method_index("onVehicleHealthChanged"));
        let m_health = vehicle_def.and_then(|v| v.method_index("onHealthChanged"));
        let m_shoot = vehicle_def.and_then(|v| v.method_index("showShooting"));
        let m_hit = vehicle_def.and_then(|v| v.method_index("showDamageFromShot"));
        let m_tracer = avatar.and_then(|a| a.method_index("showTracer"));
        let m_random_event = avatar.and_then(|a| a.method_index("onRandomEvent"));
        let m_stop_tracer = avatar.and_then(|a| a.method_index("stopTracer"));
        let m_explode = avatar.and_then(|a| a.method_index("explodeProjectile"));
        // shot id -> index into b.shots
        let mut open_shots: HashMap<i32, usize> = HashMap::new();
        // Shots whose exact impact point came from stopTracer / explodeProjectile.
        let mut impact_known: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let p_health = vehicle_def.and_then(|v| v.prop_index("health"));
        let p_gun_angles = vehicle_def.and_then(|v| v.prop_index("gunAnglesPacked"));
        let p_avatar_vehicle = avatar.and_then(|a| a.prop_index("playerVehicleID"));

        let mut entity_types: HashMap<i32, u16> = HashMap::new();
        let mut last_health_event: HashMap<i32, (f32, i32)> = HashMap::new();

        for p in packets(&stream) {
            b.duration = b.duration.max(p.time);
            let mut c = Cursor::new(p.payload);
            match p.kind {
                // Avatar (base player) create: id, type, 4 bytes, u32 len, then base props: name, sessionID, arenaUniqueID, arenaTypeID, ...
                0x00 => {
                    c.take(14);
                    let n = c.u8().unwrap_or(0) as usize;
                    let name = c.take(n).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                    if b.player_name.is_empty() {
                        b.player_name = name;
                    }
                    let n = c.u8().unwrap_or(0) as usize;
                    c.take(n + 8);
                    if let Some(arena_type) = c.i32() {
                        b.gameplay_id = arena_type >> 16;
                        if b.gameplay.is_empty() {
                            b.gameplay = gameplay_name(arena_type >> 16).to_string();
                        }
                    }
                }
                // Space / map: space id, arena id, u8 len + "spaces/<map>"
                0x0f => {
                    c.take(8);
                    let n = c.u8().unwrap_or(0) as usize;
                    if let Some(name) = c.take(n) {
                        let name = String::from_utf8_lossy(name);
                        if b.map.is_empty() {
                            b.map = name.trim_start_matches("spaces/").to_string();
                        }
                    }
                }
                // Recording player's vehicle id
                0x22 => {
                    if let Some(id) = c.i32() {
                        b.recorder_vehicle.get_or_insert(id);
                    }
                }
                // Entity enters AoI: id, type, 3 x i32, position, direction, state (u32 length + props)
                0x05 => {
                    let (Some(id), Some(ty)) = (c.i32(), c.i16()) else { continue };
                    let ty = ty as u16;
                    entity_types.insert(id, ty);
                    if Some(ty) == network_type_id {
                        // Map scenario object: id, type, 3 x i32, position, direction, state (name, prefab_path, ...).
                        c.take(12);
                        let pos = [c.f32().unwrap_or(0.0), c.f32().unwrap_or(0.0), c.f32().unwrap_or(0.0)];
                        let yaw = c.f32().unwrap_or(0.0);
                        c.take(8);
                        let (Some(nd), Some(_), Some(count)) = (network_def, c.u32(), c.u8()) else { continue };
                        let mut name = String::new();
                        for _ in 0..count {
                            let Some(idx) = c.u8() else { break };
                            let Some(prop) = nd.client_props.get(idx as usize) else { break };
                            let Some(val) = c.read(&prop.ty) else { break };
                            if prop.name == "name" || (prop.name == "prefab_path" && name.is_empty()) {
                                let s = String::from_utf8_lossy(val.bytes()).into_owned();
                                if !s.is_empty() {
                                    name = s.rsplit('/').next().unwrap_or(&s).to_string();
                                }
                            }
                        }
                        let keep = name.ends_with(".prefab") && (name.contains("_normal") || name.contains("_destroyed") || name.contains("_complete"))
                            && !name.contains("effects") && !name.contains("sfx") && !name.contains("minimap");
                        if keep {
                            scenario_ids.insert(id, b.scenario.len());
                            b.scenario.push(ScenarioObject { prefab: name, pos, yaw, created: p.time, removed: None });
                        }
                        continue;
                    }
                    if Some(ty) != vehicle_type_id {
                        continue;
                    }
                    c.take(12);
                    let pos = [c.f32().unwrap_or(0.0), c.f32().unwrap_or(0.0), c.f32().unwrap_or(0.0)];
                    // direction = (yaw, pitch, roll)
                    let dir = [c.f32().unwrap_or(0.0), c.f32().unwrap_or(0.0), c.f32().unwrap_or(0.0)];
                    let rec = b.vehicles.entry(id).or_insert_with(|| VehicleRec { id, ..Default::default() });
                    rec.visibility.push((p.time, true));
                    rec.track.push(Sample { t: p.time, pos, yaw: dir[0], pitch: dir[1], roll: dir[2] });
                    // Initial property state.
                    let Some(vd) = vehicle_def else { continue };
                    if c.u32().is_none() {
                        continue;
                    }
                    let Some(count) = c.u8() else { continue };
                    for _ in 0..count {
                        let Some(idx) = c.u8() else { break };
                        let Some(prop) = vd.client_props.get(idx as usize) else { break };
                        let Some(val) = c.read(&prop.ty) else { break };
                        match prop.name.as_str() {
                            "health" => rec.health.push((p.time, val.int() as i32)),
                            "gunAnglesPacked" => {
                                rec.turret_yaw.push((p.time, decode_turret_yaw(val.int() as u16)));
                                rec.gun_codes.push((p.time, val.int() as u16));
                            }
                            "publicInfo" => {
                                if rec.name.is_empty() {
                                    rec.name = String::from_utf8_lossy(val.get("name").map(Val::bytes).unwrap_or(&[])).into_owned();
                                }
                                if let Some(t) = val.get("team") {
                                    rec.team = t.int() as u8;
                                }
                                if rec.max_health == 0 {
                                    rec.max_health = val.get("maxHealth").map(Val::int).unwrap_or(0) as i32;
                                }
                                if rec.vehicle_type.is_empty() {
                                    if let Some(vt) = val.get("compDescr").and_then(|cd| vehicle_type_from_compdescr(gd, cd.bytes())) {
                                        rec.vehicle_type = vt;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                // Entity leaves AoI
                0x04 | 0x06 => {
                    if let Some(id) = c.i32() {
                        if let Some(&i) = scenario_ids.get(&id) {
                            b.scenario[i].removed.get_or_insert(p.time);
                        }
                        if let Some(rec) = b.vehicles.get_mut(&id) {
                            rec.visibility.push((p.time, false));
                        }
                    }
                }
                // Position: entity id, space id, position, position error, yaw, pitch, roll
                0x0a => {
                    let (Some(id), Some(_), Some(x), Some(y), Some(z)) = (c.i32(), c.i32(), c.f32(), c.f32(), c.f32()) else { continue };
                    let Some(rec) = b.vehicles.get_mut(&id) else { continue };
                    c.take(12);
                    let yaw = c.f32().unwrap_or(0.0);
                    let pitch = c.f32().filter(|v| v.is_finite()).unwrap_or(0.0);
                    let roll = c.f32().filter(|v| v.is_finite()).unwrap_or(0.0);
                    if x.is_finite() && z.is_finite() {
                        rec.track.push(Sample { t: p.time, pos: [x, y, z], yaw, pitch, roll });
                    }
                }
                // Arena-wide state updates: kills of every tank, also ones out of view.
                0x37 => {
                    // Enemies seen for the first time (also ones never near you): add to the roster.
                    if p.payload.len() > 40 {
                        let ids: Vec<i32> = b.vehicles.keys().copied().collect();
                        for (id, name, vt) in vehicle_info_records(gd, p.payload, &ids) {
                            let rec = b.vehicles.entry(id).or_insert_with(|| VehicleRec { id, team: TEAM_FROM_INFO, ..Default::default() });
                            if rec.name.is_empty() {
                                rec.name = name;
                            }
                            if rec.vehicle_type.is_empty() {
                                rec.vehicle_type = vt;
                            }
                        }
                    }
                    for (victim, killer) in kill_records(p.payload) {
                        let Some(rec) = b.vehicles.get_mut(&victim) else { continue };
                        if rec.death_time.is_some() {
                            continue;
                        }
                        rec.death_time = Some(p.time);
                        rec.health.push((p.time, 0));
                        b.events.push(Event { t: p.time, kind: EventKind::Kill { killer, victim } });
                    }
                }
                // Entity property update
                0x07 => {
                    let (Some(id), Some(mid), Some(_)) = (c.i32(), c.u32(), c.u32()) else { continue };
                    let ty = entity_types.get(&id).copied();
                    if ty == vehicle_type_id && Some(mid as usize) == p_gun_angles {
                        if let (Some(rec), Some(v)) = (b.vehicles.get_mut(&id), c.take(2)) {
                            rec.turret_yaw.push((p.time, decode_turret_yaw(u16::from_le_bytes([v[0], v[1]]))));
                            rec.gun_codes.push((p.time, u16::from_le_bytes([v[0], v[1]])));
                        }
                    } else if ty == vehicle_type_id && Some(mid as usize) == p_health {
                        if let (Some(rec), Some(h)) = (b.vehicles.get_mut(&id), c.i16()) {
                            rec.health.push((p.time, h as i32));
                        }
                    } else if ty != vehicle_type_id && Some(mid as usize) == p_avatar_vehicle {
                        if let Some(v) = c.i32() {
                            b.recorder_vehicle = Some(v);
                        }
                    }
                }
                // Entity method call
                0x08 => {
                    let (Some(id), Some(mid), Some(_)) = (c.i32(), c.u32(), c.u32()) else { continue };
                    let body = c.rest();
                    let mid = Some(mid as usize);
                    let is_vehicle = entity_types.get(&id).copied() == vehicle_type_id;
                    if is_vehicle {
                        let vd = vehicle_def.unwrap();
                        if mid == m_health {
                            let Some(args) = vd.client_methods[mid.unwrap()].decode(body) else { continue };
                            push_health(&mut b, &mut last_health_event, p.time, id, args[0].int() as i32, args[1].int() as i32, args[2].int() as i32);
                        } else if mid == m_shoot {
                            b.events.push(Event { t: p.time, kind: EventKind::Shot { shooter: id } });
                        } else if mid == m_hit {
                            // showDamageFromShot(attackerID, points, ...): this vehicle was hit.
                            let mut c2 = Cursor::new(body);
                            if let Some(attacker) = c2.i32() {
                                b.hits.push(Hit { t: p.time, target: id, attacker });
                                // Attach to that attacker's most recent unfinished shot.
                                let n = b.shots.len();
                                if let Some(i) = (n.saturating_sub(40)..n).rev().find(|&i| b.shots[i].shooter == attacker && b.shots[i].hit.is_none() && p.time - b.shots[i].t < 3.0) {
                                    let target_pos = b.vehicles.get(&id).and_then(|v| v.track.last()).map(|s| s.pos);
                                    let shot = &mut b.shots[i];
                                    shot.hit = Some(id);
                                    // Keep the exact impact point if we have it; otherwise end where the arc
                                    // reaches the target.
                                    if let (false, Some(pos)) = (impact_known.contains(&i), target_pos) {
                                        let d = ((pos[0] - shot.start[0]).powi(2) + (pos[2] - shot.start[2]).powi(2)).sqrt();
                                        shot.end = [pos[0], arc_height(shot.start, shot.vel, shot.gravity, d).max(pos[1] + 0.5), pos[2]];
                                        shot.end_t = p.time;
                                    }
                                }
                            }
                        }
                    } else if let Some(av) = avatar {
                        if mid == m_avatar_health {
                            let Some(args) = av.client_methods[mid.unwrap()].decode(body) else { continue };
                            let target = args[0].int() as i32;
                            let new_hp = args[1].int() as i32;
                            let old = b.vehicles.get(&target).and_then(|r| r.health.last()).map(|h| h.1).unwrap_or(new_hp);
                            push_health(&mut b, &mut last_health_event, p.time, target, new_hp, old, args[2].int() as i32);
                        } else if mid == m_update_arena {
                            let Some(args) = av.client_methods[mid.unwrap()].decode(body) else { continue };
                            handle_arena_update(&mut b, p.time, args[0].int(), args[1].bytes());
                        } else if mid == m_random_event && m_random_event.is_some() {
                            // onRandomEvent("04_airshipCrash01_scenario"): the warning for a random event.
                            let Some(args) = av.client_methods[mid.unwrap()].decode(body) else { continue };
                            let raw = String::from_utf8_lossy(args.first().map(Val::bytes).unwrap_or(&[])).into_owned();
                            let core = raw.trim_end_matches("_scenario");
                            let core = core.split_once('_').filter(|(p, _)| p.chars().all(|c| c.is_ascii_digit())).map(|(_, r)| r).unwrap_or(core);
                            if !core.is_empty() && !b.random_events.iter().any(|e| e.name == core) {
                                b.random_events.push(RandomEvent { name: core.to_string(), warned: p.time, start: None, end: None, pos: None });
                            }
                        } else if mid == m_tracer {
                            // showTracer(attacker, shotID, isRicochet, effects, ?, ?, ?, refStartPoint, velocity, gravity, maxShotDist, ...)
                            let Some(args) = av.client_methods[mid.unwrap()].decode(body) else { continue };
                            let v3 = |v: &Val| -> [f32; 3] {
                                match v {
                                    Val::Vector(x) if x.len() == 3 => [x[0], x[1], x[2]],
                                    _ => [0.0; 3],
                                }
                            };
                            // args[7] is the muzzle (the shell's start point); fall back to the shooter's tank
                            // in the rare case it is far away from it.
                            let shooter = args[0].int() as i32;
                            let ref_start = v3(&args[7]);
                            let shooter_pos = b.vehicles.get(&shooter).and_then(|v| v.track.last()).filter(|s| p.time - s.t < 2.0).map(|s| s.pos);
                            let start = match shooter_pos {
                                Some(sp) if (sp[0] - ref_start[0]).powi(2) + (sp[2] - ref_start[2]).powi(2) > 15.0 * 15.0 => [sp[0], sp[1] + 2.5, sp[2]],
                                _ => ref_start,
                            };
                            let vel = v3(&args[8]);
                            let gravity = (args[9].float() as f32).max(0.0);
                            let max_dist = (args[10].float() as f32).clamp(50.0, 720.0);
                            let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt().max(1.0);
                            let vh = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt().max(1.0);
                            // Until the impact arrives, assume it flies to max range along its arc.
                            let end = [start[0] + vel[0] / vh * max_dist, arc_height(start, vel, gravity, max_dist), start[2] + vel[2] / vh * max_dist];
                            let shot_id = args[1].int() as i32;
                            open_shots.insert(shot_id, b.shots.len());
                            b.shots.push(Shot {
                                t: p.time,
                                shooter,
                                id: shot_id,
                                start,
                                end,
                                end_t: p.time + max_dist / speed,
                                hit: None,
                                vel,
                                gravity,
                            });
                        } else if mid == m_stop_tracer || mid == m_explode {
                            // stopTracer(shotID, endPoint) / explodeProjectile(shotID, ..., endPoint, ...)
                            let Some(args) = av.client_methods[mid.unwrap()].decode(body) else { continue };
                            let shot_id = args[0].int() as i32;
                            let end = args.iter().skip(1).find_map(|v| match v {
                                Val::Vector(x) if x.len() == 3 => Some([x[0], x[1], x[2]]),
                                _ => None,
                            });
                            if let (Some(&i), Some(mut end)) = (open_shots.get(&shot_id), end) {
                                let shot = &mut b.shots[i];
                                // The exact impact point (on a tank or the ground). A height of exactly 0
                                // means "unknown" (left the map / max range): take it from the arc.
                                if end[1] == 0.0 {
                                    let d = ((end[0] - shot.start[0]).powi(2) + (end[2] - shot.start[2]).powi(2)).sqrt();
                                    end[1] = arc_height(shot.start, shot.vel, shot.gravity, d);
                                }
                                shot.end = end;
                                shot.end_t = p.time;
                                impact_known.insert(i);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Fill in names / classes, recorder.
        for rec in b.vehicles.values_mut() {
            if let Some(info) = gd.vehicle(&rec.vehicle_type) {
                rec.class = Some(info.class);
                rec.tank_name = info.name;
                rec.tank_short = info.short_name;
                rec.tier = info.tier;
            } else {
                rec.tank_name = rec.vehicle_type.split(':').nth(1).unwrap_or("?").to_string();
                rec.tank_short = rec.tank_name.clone();
            }
            if rec.max_health == 0 {
                // Not given by the replay (e.g. an enemy only known from its info record): the
                // client's vehicle data, else the highest HP seen.
                rec.max_health = gd.vehicle_max_health(&rec.vehicle_type).unwrap_or_else(|| rec.health.iter().map(|h| h.1).max().unwrap_or(0));
            }
            rec.track.sort_by(|a, b| a.t.total_cmp(&b.t));
        }
        if b.map_display.is_empty() {
            b.map_display = gd.arena(&b.map).map(|a| a.display_name).unwrap_or_else(|| b.map.clone());
        }
        if b.server_realm.is_empty() {
            b.server_realm = gd.realm();
        }
        b.live = rf.is_live();
        if b.recorder_vehicle.is_none() {
            b.recorder_vehicle = b.vehicles.values().find(|v| v.name == b.player_name).map(|v| v.id);
        }
        b.recorder_team = b.recorder_vehicle.and_then(|id| b.vehicles.get(&id)).map(|v| v.team).unwrap_or(1);
        calibrate_gun_pitch(&mut b);
        // Tanks known only from enemy info records: the recorder's enemies.
        let enemy = if b.recorder_team == 1 { 2 } else { 1 };
        for rec in b.vehicles.values_mut().filter(|r| r.team == TEAM_FROM_INFO) {
            rec.team = enemy;
        }
        // Drop roster entries that never got a team (non-player entities that slipped through).
        b.vehicles.retain(|_, v| v.team != 0);
        b.events.sort_by(|a, c| a.t.total_cmp(&c.t));
        b.shots.sort_by(|a, c| a.t.total_cmp(&c.t));
        // Random events: when each one broke things, and where.
        for ev in b.random_events.iter_mut() {
            let tag = format!("_{}_", ev.name);
            let mine: Vec<&ScenarioObject> = b.scenario.iter().filter(|s| s.prefab.contains(&tag)).collect();
            let times: Vec<f32> = mine
                .iter()
                .filter_map(|s| if s.prefab.contains("_destroyed") { Some(s.created) } else { s.removed })
                .filter(|&t| t >= ev.warned)
                .collect();
            ev.start = times.iter().copied().reduce(f32::min);
            ev.end = times.iter().copied().reduce(f32::max);
            let wrecks: Vec<[f32; 3]> = mine.iter().filter(|s| s.prefab.contains("_destroyed")).map(|s| s.pos).collect();
            if !wrecks.is_empty() {
                let n = wrecks.len() as f32;
                ev.pos = Some([wrecks.iter().map(|p| p[0]).sum::<f32>() / n, wrecks.iter().map(|p| p[2]).sum::<f32>() / n]);
            }
        }
        // Destroyed states placed in advance (e.g. the train wreck exists from the start, hidden):
        // visible only once their event has actually broken something; never if it didn't happen.
        for s in b.scenario.iter_mut().filter(|s| s.prefab.contains("_destroyed")) {
            let ev = b.random_events.iter().find(|e| s.prefab.contains(&format!("_{}_", e.name)));
            let since = match ev {
                Some(e) if s.created >= e.warned => continue,
                Some(e) => e.start.unwrap_or(f32::MAX),
                None if s.created > 1.0 => continue,
                None => f32::MAX,
            };
            s.created = s.created.max(since);
        }
        // Hide the intact state the moment its destroyed state appears at the same place.
        let snapshot = b.scenario.clone();
        for s in b.scenario.iter_mut() {
            let next = snapshot
                .iter()
                .filter(|o| o.created > s.created && o.prefab != s.prefab)
                .filter(|o| (o.pos[0] - s.pos[0]).abs() < 1.0 && (o.pos[2] - s.pos[2]).abs() < 1.0)
                .map(|o| o.created)
                .fold(f32::MAX, f32::min);
            if next < f32::MAX {
                s.removed = Some(s.removed.map_or(next, |r| r.min(next)));
            }
        }
        b.hits.sort_by(|a, c| a.t.total_cmp(&c.t));
        if b.periods.is_empty() {
            b.periods.push((0.0, Period::Battle, 900.0));
        }
        // Bases from the arena definition, capture radius estimated from the replay.
        if let Some(gp) = gd.arena(&b.map).and_then(|a| a.gameplay.get(&b.gameplay).cloned()) {
            b.team_bases = gp.bases.clone();
            b.control_points = gp.control_points.clone();
        }
        b.capture_radius = estimate_capture_radius(&b).unwrap_or(50.0);
        // Deaths the stream never showed (tank out of view): the results block has every tank's
        // lifetime (seconds after battle start), killer and death reason.
        let start = b.battle_start();
        let mut late_kills = Vec::new();
        for rec in b.vehicles.values_mut() {
            let Some(f) = &rec.final_stats else { continue };
            let reason = f.get("deathReason").and_then(|v| v.as_i64()).unwrap_or(-1);
            let life = f.get("lifeTime").and_then(|v| v.as_f64());
            if let (true, None, Some(life)) = (reason >= 0, rec.death_time, life) {
                let t = start + life as f32;
                rec.death_time = Some(t);
                rec.health.push((t, 0));
                rec.health.sort_by(|a, c| a.0.total_cmp(&c.0));
                let killer = f.get("killerID").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                late_kills.push(Event { t, kind: EventKind::Kill { killer, victim: rec.id } });
            }
        }
        if !late_kills.is_empty() {
            b.events.extend(late_kills);
            b.events.sort_by(|a, c| a.t.total_cmp(&c.t));
        }
        Ok(b)
    }
}

fn push_health(b: &mut Battle, last: &mut HashMap<i32, (f32, i32)>, t: f32, target: i32, new_hp: i32, old_hp: i32, attacker: i32) {
    // Vehicle.onHealthChanged and Avatar.onVehicleHealthChanged can both report the same hit.
    if let Some(&(lt, lh)) = last.get(&target) {
        if (t - lt).abs() < 0.05 && lh == new_hp {
            return;
        }
    }
    last.insert(target, (t, new_hp));
    let amount = (old_hp - new_hp.max(0)).max(0);
    if amount > 0 && attacker > 0 {
        b.events.push(Event { t, kind: EventKind::Damage { attacker, target, amount } });
    }
    if let Some(rec) = b.vehicles.get_mut(&target) {
        rec.health.push((t, new_hp.max(0)));
        if new_hp <= 0 && rec.death_time.is_none() {
            rec.death_time = Some(t);
            b.events.push(Event { t, kind: EventKind::Kill { killer: attacker, victim: target } });
        }
    }
}

/// Kill records in a state-update packet (type 0x37): a record holding the pickled
/// `{'deathOrder': n}` is followed by a 16-byte record of (3 path bytes, victim id, killer id, ...),
/// both with the same 3-byte record header. Byte values of the header differ between battles,
/// so only the structure is matched.
pub fn kill_records(d: &[u8]) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let needle = b"deathOrder";
    let mut from = 0;
    while let Some(j) = d.get(from..).and_then(|s| s.windows(needle.len()).position(|w| w == needle)).map(|p| p + from) {
        from = j + needle.len();
        // Start of the record containing the pickle: header (3 bytes) + length 0x1a.
        let Some(r) = (j.saturating_sub(40)..j).rev().find(|&r| d.get(r + 3) == Some(&0x1a) && r + 30 + 4 <= d.len() && d[r..r + 3] == d[r + 30..r + 33]) else { continue };
        let n = r + 30;
        if d.get(n + 3) != Some(&0x10) || d.len() < n + 4 + 11 {
            continue;
        }
        let id = |o: usize| i32::from_le_bytes(d[o..o + 4].try_into().unwrap());
        out.push((id(n + 7), id(n + 11)));
    }
    out
}

/// Enemy vehicle info records in a state-update packet (type 0x37), sent when an enemy is first
/// spotted (by anyone): vehicle id, a length-prefixed vehicle descriptor and the player name
/// (length-prefixed, twice). Returns (vehicle id, player name, vehicle type).
pub fn vehicle_info_records(gd: &GameData, d: &[u8], known_ids: &[i32]) -> Vec<(i32, String, String)> {
    let mut out = Vec::new();
    let near_known = |id: i32| known_ids.iter().any(|&k| (k - id).abs() < 400 && k != id || k == id);
    let mut i = 0;
    while i + 2 < d.len() {
        // name pair: [n][name][n][name]
        let n = d[i] as usize;
        let ok = (2..=24).contains(&n)
            && i + 2 + 2 * n <= d.len()
            && d[i + 1..i + 1 + n].iter().all(|c| c.is_ascii_graphic())
            && d[i + 1 + n] as usize == n
            && d[i + 1..i + 1 + n] == d[i + 2 + n..i + 2 + 2 * n];
        if !ok {
            i += 1;
            continue;
        }
        let name = String::from_utf8_lossy(&d[i + 1..i + 1 + n]).into_owned();
        let lo = i.saturating_sub(90);
        // vehicle id: the last plausible id before the name
        let id = (lo..i.saturating_sub(4)).rev().map(|o| i32::from_le_bytes(d[o..o + 4].try_into().unwrap())).find(|&v| v > 0 && near_known(v));
        // descriptor: [len][header][id...] resolving to a known vehicle
        // It ends exactly where the name starts; real ones are 15-16 bytes, so among the
        // candidates take the one closest to that.
        let vt = (lo..i.saturating_sub(3))
            .filter_map(|o| {
                let len = d[o] as usize;
                if !(10..=40).contains(&len) || o + 1 + len != i {
                    return None;
                }
                vehicle_type_from_compdescr(gd, &d[o + 1..o + 1 + len]).map(|vt| (len.abs_diff(16), vt))
            })
            .min_by_key(|c| c.0)
            .map(|c| c.1);
        if let (Some(id), Some(vt)) = (id, vt) {
            out.push((id, name, vt));
        }
        i += 2 + 2 * n;
    }
    out
}

/// The circle radius isn't in the client data: when the invader count of a base goes from 0 to
/// 1, the closest capturing tank has just crossed the edge.
fn estimate_capture_radius(b: &Battle) -> Option<f32> {
    let mut est = Vec::new();
    let mut prev: HashMap<(u8, u8), u8> = HashMap::new();
    for c in &b.captures {
        let before = prev.insert((c.team, c.base_id), c.invaders).unwrap_or(0);
        if before != 0 || c.invaders == 0 {
            continue;
        }
        let Some(p) = b.base_pos(c.team, c.base_id) else { continue };
        let d = b
            .vehicles
            .values()
            .filter(|v| v.team != c.team)
            .filter_map(|v| v.track.iter().rev().find(|s| s.t <= c.t).filter(|s| c.t - s.t < 1.0))
            .map(|s| ((s.pos[0] - p[0]).powi(2) + (s.pos[2] - p[1]).powi(2)).sqrt())
            .fold(f32::MAX, f32::min);
        if (25.0..120.0).contains(&d) {
            est.push(d);
        }
    }
    est.sort_by(|a, c| a.total_cmp(c));
    est.get(est.len() / 2).copied()
}

fn handle_arena_update(b: &mut Battle, t: f32, kind: i64, raw: &[u8]) {
    if std::env::var_os("LIVEREPLAY_DEBUG_ARENA").is_some() {
        eprintln!("ARENARAW t={t:.1} kind={kind} len={} head={:?}", raw.len(), &raw[..raw.len().min(12)]);
    }
    let mut data = Vec::new();
    let bytes = if flate2::read::ZlibDecoder::new(raw).read_to_end(&mut data).is_ok() { &data[..] } else { raw };
    let Some(v) = pickle::loads(bytes) else { return };
    if std::env::var_os("LIVEREPLAY_DEBUG_ARENA").is_some() {
        eprintln!("ARENA t={t:.1} kind={kind} {}", format!("{v:?}").chars().take(600).collect::<String>());
    }
    // ARENA_UPDATE.PERIOD = 3: (period, endTime, length, additionalInfo)
    // ARENA_UPDATE.BASE_POINTS = 8: (team, baseID, points, timeLeft, invadersCount, capturingStopped)
    if kind == 8 {
        let n = |i: usize| v.at(i).int();
        b.captures.push(CaptureSample {
            t,
            team: n(0) as u8,
            base_id: n(1) as u8,
            points: n(2).clamp(0, 100) as u8,
            invaders: n(4).clamp(0, 255) as u8,
            time_left: n(3).clamp(0, 65535) as u16,
            stopped: n(5) != 0,
        });
        return;
    }
    if kind == 3 {
        let period = match v.at(0).int() {
            1 => Period::Waiting,
            2 => Period::Prebattle,
            3 => Period::Battle,
            _ => Period::AfterBattle,
        };
        let length = v.at(2).float() as f32;
        b.periods.push((t, period, length));
        if period == Period::AfterBattle {
            let w = v.at(3).at(0).int();
            if w > 0 && b.winner_team.is_none() {
                b.winner_team = Some(w as u8);
            }
        }
    }
}
