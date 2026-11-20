//! Several recordings of the same battle (one per player who uploaded it) merged into one
//! timeline: a tank counts as seen when any recording saw it, positions and health come from
//! whichever recording had them. Recordings are aligned on the moment the battle started.

use super::battle::{Battle, Period};

/// Two recordings are of the same battle when they are on the same map and share tanks
/// (vehicle ids are unique per battle on a server).
pub fn same_battle(a: &Battle, b: &Battle) -> bool {
    a.map == b.map && a.server_realm == b.server_realm && a.vehicles.keys().filter(|id| b.vehicles.contains_key(id)).count() >= 3
}

fn shift(b: &mut Battle, dt: f32) {
    for v in b.vehicles.values_mut() {
        v.track.iter_mut().for_each(|s| s.t += dt);
        v.visibility.iter_mut().for_each(|s| s.0 += dt);
        v.health.iter_mut().for_each(|s| s.0 += dt);
        v.turret_yaw.iter_mut().for_each(|s| s.0 += dt);
        v.gun_codes.iter_mut().for_each(|s| s.0 += dt);
        v.shot_pitch.iter_mut().for_each(|s| s.0 += dt);
        v.crits.iter_mut().for_each(|c| c.t += dt);
        if let Some(d) = &mut v.death_time {
            *d += dt;
        }
    }
    b.events.iter_mut().for_each(|e| e.t += dt);
    b.shots.iter_mut().for_each(|s| {
        s.t += dt;
        s.end_t += dt;
    });
    b.hits.iter_mut().for_each(|h| h.t += dt);
    b.captures.iter_mut().for_each(|c| c.t += dt);
    b.periods.iter_mut().for_each(|p| p.0 += dt);
    b.duration += dt;
}

/// Merge two time-sorted lists, dropping entries of `b` that `same` says `a` already has nearby.
fn union<T: Clone>(a: &[T], b: &[T], t: impl Fn(&T) -> f32, same: impl Fn(&T, &T) -> bool) -> Vec<T> {
    let mut out: Vec<T> = a.to_vec();
    for x in b {
        let tx = t(x);
        let lo = a.partition_point(|y| t(y) < tx - 1.0);
        if !a[lo..].iter().take_while(|y| t(y) <= tx + 1.0).any(|y| same(y, x)) {
            out.push(x.clone());
        }
    }
    out.sort_by(|p, q| t(p).total_cmp(&t(q)));
    out
}

/// In view at `t` per an AoI transition list (no entry before `t` = in view, as the viewer does).
fn in_view(v: &[(f32, bool)], t: f32) -> bool {
    let n = v.partition_point(|x| x.0 <= t);
    if n == 0 {
        true
    } else {
        v[n - 1].1
    }
}

/// Seen by either recording.
fn visibility_union(a: &[(f32, bool)], b: &[(f32, bool)]) -> Vec<(f32, bool)> {
    let mut times: Vec<f32> = a.iter().chain(b).map(|x| x.0).collect();
    times.sort_by(f32::total_cmp);
    times.dedup();
    let mut out: Vec<(f32, bool)> = Vec::new();
    for t in times {
        let s = in_view(a, t) || in_view(b, t);
        if out.last().map(|l| l.1) != Some(s) {
            out.push((t, s));
        }
    }
    out
}

/// Add another player's recording of the same battle to `base` (the viewpoint shown).
pub fn add_viewpoint(base: &mut Battle, mut other: Battle) {
    let start = |b: &Battle| b.periods.iter().find(|p| p.1 == Period::Battle).map(|p| p.0);
    if let (Some(a), Some(b)) = (start(base), start(&other)) {
        shift(&mut other, a - b);
    }
    for (id, ov) in other.vehicles {
        let Some(v) = base.vehicles.get_mut(&id) else {
            base.vehicles.insert(id, ov);
            continue;
        };
        // Positions: whichever recording had one; within 50 ms they're the same sample.
        v.track = union(&v.track, &ov.track, |s| s.t, |p, q| (p.t - q.t).abs() < 0.05);
        v.visibility = visibility_union(&v.visibility, &ov.visibility);
        v.health = union(&v.health, &ov.health, |s| s.0, |p, q| p.1 == q.1);
        v.health.dedup_by(|q, p| q.1 == p.1);
        v.gun_codes = union(&v.gun_codes, &ov.gun_codes, |s| s.0, |p, q| (p.0 - q.0).abs() < 0.05);
        v.turret_yaw = union(&v.turret_yaw, &ov.turret_yaw, |s| s.0, |p, q| (p.0 - q.0).abs() < 0.05);
        v.shot_pitch = union(&v.shot_pitch, &ov.shot_pitch, |s| s.0, |p, q| (p.0 - q.0).abs() < 0.05);
        v.crits = union(&v.crits, &ov.crits, |c| c.t, |p, q| p.slot == q.slot && p.state == q.state && (p.t - q.t).abs() < 0.5);
        v.death_time = match (v.death_time, ov.death_time) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        v.max_health = v.max_health.max(ov.max_health);
        if v.gun_pitch_fit.is_none() {
            v.gun_pitch_fit = ov.gun_pitch_fit;
        }
        if v.final_stats.is_none() {
            v.final_stats = ov.final_stats;
        }
        for (mine, theirs) in [(&mut v.name, ov.name), (&mut v.clan, ov.clan), (&mut v.vehicle_type, ov.vehicle_type), (&mut v.tank_name, ov.tank_name), (&mut v.tank_short, ov.tank_short)] {
            if mine.is_empty() {
                *mine = theirs;
            }
        }
        if v.class.is_none() {
            v.class = ov.class;
        }
        if v.tier == 0 {
            v.tier = ov.tier;
        }
    }
    base.events = union(&base.events, &other.events, |e| e.t, |p, q| p.kind == q.kind && (p.t - q.t).abs() < 0.5);
    base.shots = union(&base.shots, &other.shots, |s| s.t, |p, q| p.shooter == q.shooter && (p.t - q.t).abs() < 0.3);
    base.hits = union(&base.hits, &other.hits, |h| h.t, |p, q| p.target == q.target && p.attacker == q.attacker && (p.t - q.t).abs() < 0.3);
    base.captures = union(&base.captures, &other.captures, |c| c.t, |p, q| p.team == q.team && p.base_id == q.base_id && (p.t - q.t).abs() < 0.6);
    base.duration = base.duration.max(other.duration);
    if base.winner_team.is_none() {
        base.winner_team = other.winner_team;
    }
    if let Some(r) = other.recorder_vehicle.filter(|r| Some(*r) != base.recorder_vehicle && !base.viewpoints.contains(r)) {
        base.viewpoints.push(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_is_seen_by_anyone() {
        let a = vec![(0.0, false), (10.0, true), (20.0, false)];
        let b = vec![(0.0, false), (15.0, true), (30.0, false)];
        assert_eq!(visibility_union(&a, &b), vec![(0.0, false), (10.0, true), (30.0, false)]);
    }
}
