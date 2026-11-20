//! Renderer-agnostic battle state at a point in time. Everything here is in world
//! coordinates so a 2D minimap and a future 3D view can share it.

use crate::gamedata::VehicleClass;
use crate::replay::battle::{Battle, EventKind, VehicleRec};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visibility {
    /// Currently in view (allies always are while alive).
    Visible,
    /// Not in view any more; position is where it was last seen `age` seconds ago.
    LastKnown { age: f32 },
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub id: i32,
    pub team: u8,
    pub is_ally: bool,
    pub is_recorder: bool,
    /// Another uploader's tank in a merged battle (gets its own view-range circle).
    pub is_viewpoint: bool,
    /// Damaged (1) / destroyed (2) modules and crew right now: (kind, state), alive tanks only.
    pub crits: Vec<(String, u8)>,
    pub class: VehicleClass,
    /// World position (x east, y up, z north).
    pub pos: [f32; 3],
    /// Hull yaw in radians (0 = north, positive towards east).
    pub yaw: f32,
    /// Hull pitch (+ = nose down) and roll (+ = right side up) as recorded; both 0 = unknown.
    pub pitch: f32,
    pub roll: f32,
    /// Gun pitch relative to the hull (radians, + = up).
    pub gun_pitch: f32,
    pub hp: Option<i32>,
    pub max_hp: i32,
    pub alive: bool,
    pub visibility: Visibility,
    /// 0..1 white flash after taking a shell hit.
    pub flash: f32,
    /// Damage number shown next to the tank: (accumulated damage, opacity).
    pub damage_popup: Option<(i32, f32)>,
    /// Turret yaw relative to the hull (radians).
    pub turret_yaw: f32,
    pub vehicle_type: String,
    pub player: String,
    pub tank: String,
}

#[derive(Debug, Clone, Default)]
pub struct PlayerStats {
    pub shots_seen: u32,
    pub damage_dealt: i32,
    pub damaging_hits: u32,
    pub damage_received: i32,
    pub hits_received: u32,
    pub kills: u32,
    pub killed_by: Option<i32>,
    pub death_time: Option<f32>,
}

fn last_le<T>(v: &[T], t: f32, time: impl Fn(&T) -> f32) -> Option<usize> {
    let n = v.partition_point(|x| time(x) <= t);
    n.checked_sub(1)
}

pub fn health_at(rec: &VehicleRec, t: f32) -> Option<i32> {
    last_le(&rec.health, t, |h| h.0).map(|i| rec.health[i].1)
}

fn interpolate(rec: &VehicleRec, t: f32) -> Option<([f32; 3], f32, f32, f32, f32)> {
    let i = last_le(&rec.track, t, |s| s.t)?;
    let a = rec.track[i];
    let mut pos = a.pos;
    let mut yaw = a.yaw;
    let (mut pitch, mut roll) = (a.pitch, a.roll);
    if let Some(b) = rec.track.get(i + 1) {
        let span = b.t - a.t;
        if span > 0.0 && span < 2.0 {
            let k = ((t - a.t) / span).clamp(0.0, 1.0);
            for j in 0..3 {
                pos[j] = a.pos[j] + (b.pos[j] - a.pos[j]) * k;
            }
            let mut d = b.yaw - a.yaw;
            while d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            while d < -std::f32::consts::PI {
                d += std::f32::consts::TAU;
            }
            yaw = a.yaw + d * k;
            pitch = a.pitch + wrap_angle(b.pitch - a.pitch) * k;
            roll = a.roll + wrap_angle(b.roll - a.roll) * k;
        }
    }
    Some((pos, yaw, pitch, roll, a.t))
}

/// Hull-relative gun pitch: calibrated code when available, else the pitch of a nearby shot.
pub fn gun_pitch_at(rec: &VehicleRec, t: f32) -> f32 {
    if let Some((a, b)) = rec.gun_pitch_fit {
        if let Some(i) = last_le(&rec.gun_codes, t, |c| c.0) {
            let code = |j: usize| (rec.gun_codes[j].1 & 63) as f32;
            let mut c = code(i);
            if let Some(n) = rec.gun_codes.get(i + 1) {
                let span = n.0 - rec.gun_codes[i].0;
                if span > 0.0 && span < 2.0 {
                    c += (code(i + 1) - c) * ((t - rec.gun_codes[i].0) / span).clamp(0.0, 1.0);
                }
            }
            return a + b * c;
        }
    }
    // Uncalibrated: raise the gun towards a shot from 1 s before to 1 s after it.
    rec.shot_pitch
        .iter()
        .filter_map(|&(st, p)| {
            let w = 1.0 - ((t - st).abs() - 1.0).clamp(0.0, 1.0);
            (w > 0.0).then_some((p, w))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, w)| p * w)
        .unwrap_or(0.0)
}

fn wrap_angle(mut d: f32) -> f32 {
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

/// Turret yaw relative to the hull, interpolated between updates so traverse looks smooth.
fn turret_at(rec: &VehicleRec, t: f32) -> f32 {
    let Some(i) = last_le(&rec.turret_yaw, t, |v| v.0) else { return 0.0 };
    let (t0, a0) = rec.turret_yaw[i];
    match rec.turret_yaw.get(i + 1) {
        Some(&(t1, a1)) if t1 - t0 > 0.0 && t1 - t0 < 3.0 => a0 + wrap_angle(a1 - a0) * ((t - t0) / (t1 - t0)).clamp(0.0, 1.0),
        _ => a0,
    }
}

/// Modules / crew not working at `t`: the latest state of each slot, one entry per kind (worst state).
pub fn crits_at(rec: &VehicleRec, t: f32) -> Vec<(String, u8)> {
    let mut slots: Vec<(&str, &str, u8)> = Vec::new();
    for c in rec.crits.iter().take_while(|c| c.t <= t) {
        match slots.iter_mut().find(|s| s.0 == c.slot) {
            Some(s) => s.2 = c.state,
            None => slots.push((&c.slot, &c.kind, c.state)),
        }
    }
    let mut out: Vec<(String, u8)> = Vec::new();
    for (_, kind, state) in slots.into_iter().filter(|s| s.2 > 0) {
        match out.iter_mut().find(|o| o.0 == kind) {
            Some(o) => o.1 = o.1.max(state),
            None => out.push((kind.to_string(), state)),
        }
    }
    out
}

pub fn units_at(b: &Battle, t: f32, include_last_known: bool) -> Vec<Unit> {
    let mut out = Vec::new();
    for rec in b.vehicles.values() {
        let Some((pos, yaw, pitch, roll, sample_t)) = interpolate(rec, t) else { continue };
        let alive = rec.death_time.map(|d| t < d).unwrap_or(true);
        let in_aoi = last_le(&rec.visibility, t, |v| v.0).map(|i| rec.visibility[i].1).unwrap_or(true);
        let is_ally = rec.team == b.recorder_team;
        // Positions arrive ~10/s while in view; a long gap means we lost it.
        let fresh = t - sample_t < 2.5;
        // Your own tank is always "in view" (its position stream can have gaps).
        let is_me = Some(rec.id) == b.recorder_vehicle || b.viewpoints.contains(&rec.id);
        let visibility = if (in_aoi && fresh) || !alive || is_me {
            Visibility::Visible
        } else {
            Visibility::LastKnown { age: t - sample_t }
        };
        if matches!(visibility, Visibility::LastKnown { .. }) && !include_last_known {
            continue;
        }
        out.push(Unit {
            id: rec.id,
            team: rec.team,
            is_ally,
            is_recorder: Some(rec.id) == b.recorder_vehicle,
            is_viewpoint: b.viewpoints.contains(&rec.id),
            crits: if alive { crits_at(rec, t) } else { Vec::new() },
            class: rec.class.unwrap_or(VehicleClass::Unknown),
            pos,
            yaw,
            pitch,
            roll,
            gun_pitch: gun_pitch_at(rec, t),
            hp: health_at(rec, t),
            max_hp: rec.max_health,
            alive,
            visibility,
            flash: flash_at(b, rec.id, t),
            damage_popup: damage_popup(rec, t),
            turret_yaw: turret_at(rec, t),
            vehicle_type: rec.vehicle_type.clone(),
            player: rec.name.clone(),
            tank: rec.tank_short.clone(),
        });
    }
    // Draw order: dead first, then enemies, allies, recorder on top.
    out.sort_by_key(|u| (u.alive, u.is_ally, u.is_recorder));
    out
}

/// Damage numbers stay fully visible for POP_HOLD, then fade over POP_FADE. A new hit before
/// the number has faded adds to it and restarts the timer.
const POP_HOLD: f32 = 0.8;
const POP_FADE: f32 = 1.4;

fn damage_popup(rec: &VehicleRec, t: f32) -> Option<(i32, f32)> {
    let n = rec.health.partition_point(|h| h.0 <= t);
    let mut total = 0;
    let mut newest: Option<f32> = None;
    let mut chain_t = t;
    for i in (1..n).rev() {
        let (ti, hi) = rec.health[i];
        let drop = rec.health[i - 1].1 - hi;
        if drop <= 0 {
            continue;
        }
        if chain_t - ti > POP_HOLD + POP_FADE {
            break;
        }
        newest.get_or_insert(ti);
        total += drop;
        chain_t = ti;
    }
    let last = newest?;
    let age = t - last;
    let alpha = if age <= POP_HOLD { 1.0 } else { 1.0 - (age - POP_HOLD) / POP_FADE };
    (alpha > 0.0 && total > 0).then_some((total, alpha))
}

/// How long a shot line stays visible after the shell lands, and the hit flash timing.
pub const SHOT_FADE: f32 = 1.4;
const FLASH_HOLD: f32 = 0.08;
const FLASH_FADE: f32 = 0.5;

fn flash_at(b: &Battle, id: i32, t: f32) -> f32 {
    let from = b.hits.partition_point(|h| h.t < t - FLASH_HOLD - FLASH_FADE);
    b.hits[from..]
        .iter()
        .take_while(|h| h.t <= t)
        .filter(|h| h.target == id)
        .map(|h| {
            let age = t - h.t;
            if age <= FLASH_HOLD {
                1.0
            } else {
                1.0 - (age - FLASH_HOLD) / FLASH_FADE
            }
        })
        .fold(0.0, f32::max)
}

/// A shot as drawn at time `t`: a line from the muzzle to where the shell currently is
/// (or landed), fading out after impact.
#[derive(Debug, Clone)]
pub struct ShotView {
    pub shooter: i32,
    pub is_ally: bool,
    /// Server reference start (the gun's pivot) and the recorded impact point.
    pub pivot: [f32; 3],
    pub end: [f32; 3],
    /// Launch direction (unit vector) and gravity.
    pub dir: [f32; 3],
    pub gravity: f32,
    /// Flight time of the reference trajectory to the impact (s), and progress 0..1.
    pub flight: f32,
    pub k: f32,
    pub alpha: f32,
    pub hit: bool,
}

impl ShotView {
    /// Drawn shell path like the game shows it: leaves the muzzle (`barrel` metres along the
    /// launch direction from the pivot) on a ballistic arc with the real gravity and flight time
    /// that lands exactly on the recorded impact. Ends at the shell's current position.
    pub fn path(&self, barrel: f32) -> Vec<[f32; 3]> {
        let tip = [0, 1, 2].map(|i| self.pivot[i] + self.dir[i] * barrel);
        let t_total = self.flight.max(1e-3);
        let g = self.gravity;
        // Start velocity that reaches `end` from `tip` in t_total under gravity.
        let v = [
            (self.end[0] - tip[0]) / t_total,
            (self.end[1] - tip[1]) / t_total + 0.5 * g * t_total,
            (self.end[2] - tip[2]) / t_total,
        ];
        let n = if g * t_total * t_total / 8.0 > 0.3 { 16 } else { 1 };
        (0..=n)
            .map(|i| {
                let t = t_total * self.k * i as f32 / n as f32;
                [tip[0] + v[0] * t, tip[1] + v[1] * t - 0.5 * g * t * t, tip[2] + v[2] * t]
            })
            .collect()
    }
}

pub fn shots_at(b: &Battle, t: f32) -> Vec<ShotView> {
    // Shells fly at most a couple of seconds; look back far enough to include fading ones.
    let from = b.shots.partition_point(|s| s.t < t - 6.0 - SHOT_FADE);
    b.shots[from..]
        .iter()
        .take_while(|s| s.t <= t)
        .filter_map(|s| {
            let flight = (s.end_t - s.t).max(0.01);
            let k = ((t - s.t) / flight).clamp(0.0, 1.0);
            let after = t - s.end_t;
            let alpha = if after <= 0.0 { 1.0 } else { 1.0 - after / SHOT_FADE };
            if alpha <= 0.0 {
                return None;
            }
            let is_ally = b.vehicles.get(&s.shooter).map(|v| v.team == b.recorder_team).unwrap_or(false);
            let sp = (s.vel[0] * s.vel[0] + s.vel[1] * s.vel[1] + s.vel[2] * s.vel[2]).sqrt();
            let dir = if sp > 1.0 {
                [s.vel[0] / sp, s.vel[1] / sp, s.vel[2] / sp]
            } else {
                let d = [s.end[0] - s.start[0], s.end[1] - s.start[1], s.end[2] - s.start[2]];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
                d.map(|x| x / l)
            };
            // Reference flight time to the impact, from the horizontal distance (as the game does).
            let vh = (s.vel[0] * s.vel[0] + s.vel[2] * s.vel[2]).sqrt();
            let dist = ((s.end[0] - s.start[0]).powi(2) + (s.end[2] - s.start[2]).powi(2)).sqrt();
            let flight = if vh > 1.0 { dist / vh } else { flight };
            Some(ShotView { shooter: s.shooter, is_ally, pivot: s.start, end: s.end, dir, gravity: if vh > 1.0 { s.gravity } else { 0.0 }, flight, k, alpha, hit: s.hit.is_some() })
        })
        .collect()
}

/// A base right now: where, how far captured, by whom.
#[derive(Debug, Clone)]
pub struct CaptureView {
    pub pos: [f32; 2],
    pub radius: f32,
    /// 0..100
    pub points: u8,
    /// Team capturing it (when someone is inside or points remain).
    pub capturing_team: Option<u8>,
    pub invaders: u8,
    /// Seconds until captured at the current rate.
    pub time_left: u16,
    /// Capturing is stopped (blocked): enemies of the cappers are inside too.
    pub blocked: bool,
    /// Tanks of the other team inside the circle (blocking).
    pub blockers: u8,
}

pub fn captures_at(b: &Battle, t: f32) -> Vec<CaptureView> {
    let mut out: Vec<CaptureView> = Vec::new();
    // Every base / control point gets a circle.
    for (team, list) in b.team_bases.iter().enumerate() {
        for (i, _) in list.iter().enumerate() {
            if let Some(pos) = b.base_pos(team as u8 + 1, i as u8 + 1) {
                out.push(CaptureView { pos, radius: b.capture_radius, points: 0, capturing_team: None, invaders: 0, time_left: 0, blocked: false, blockers: 0 });
            }
        }
    }
    for &pos in &b.control_points {
        out.push(CaptureView { pos, radius: b.capture_radius, points: 0, capturing_team: None, invaders: 0, time_left: 0, blocked: false, blockers: 0 });
    }
    // Latest state per (team, base), in a fixed order; per position the record with tanks inside
    // (then the most points) wins, so the two records of a base don't alternate.
    let mut latest: std::collections::BTreeMap<(u8, u8), &crate::replay::battle::CaptureSample> = std::collections::BTreeMap::new();
    for c in b.captures.iter().take_while(|c| c.t <= t) {
        latest.insert((c.team, c.base_id), c);
    }
    for c in latest.values() {
        if c.points == 0 && c.invaders == 0 {
            continue;
        }
        let Some(pos) = b.base_pos(c.team, c.base_id) else { continue };
        let capturing = if c.team == 1 { 2 } else { 1 };
        if let Some(view) = out.iter_mut().find(|v| v.pos == pos) {
            let better = (c.invaders > 0, c.points) > (view.invaders > 0, view.points);
            if view.capturing_team.is_none() || better {
                view.points = c.points;
                view.capturing_team = Some(capturing);
                view.invaders = c.invaders;
                view.time_left = c.time_left;
                view.blocked = c.stopped && c.invaders > 0;
            }
        }
    }
    // Defenders inside: the other team's record at the same base.
    for c in latest.values() {
        let Some(pos) = b.base_pos(c.team, c.base_id) else { continue };
        let capturing = if c.team == 1 { 2 } else { 1 };
        if let Some(view) = out.iter_mut().find(|v| v.pos == pos && v.capturing_team.is_some_and(|t| t != capturing)) {
            view.blockers = c.invaders;
        }
    }
    out
}

pub fn stats_at(b: &Battle, id: i32, t: f32) -> PlayerStats {
    let mut s = PlayerStats::default();
    for e in b.events.iter().take_while(|e| e.t <= t) {
        match e.kind {
            EventKind::Shot { shooter } if shooter == id => s.shots_seen += 1,
            EventKind::Damage { attacker, target, amount } => {
                if attacker == id && target != id {
                    s.damage_dealt += amount;
                    s.damaging_hits += 1;
                }
                if target == id {
                    s.damage_received += amount;
                    s.hits_received += 1;
                }
            }
            EventKind::Kill { killer, victim } => {
                if killer == id && victim != id {
                    s.kills += 1;
                }
                if victim == id {
                    s.killed_by = Some(killer);
                    s.death_time = Some(e.t);
                }
            }
            _ => {}
        }
    }
    s
}
