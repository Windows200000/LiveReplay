//! Damage dealt, blocked and missed per team, as far as the recording saw it (several merged
//! viewpoints see more).
//!
//! - Dealt: every health loss the recording saw, by the team that caused it (shells, splash,
//!   fire, ramming). Exact.
//! - Blocked: shells that hit a tank and did no damage (bounced or didn't penetrate).
//! - Missed: shells that hit no tank.
//!
//! The replay says which shells hit and which did damage, but not how much damage a shell would
//! have done, so blocked and missed are estimates: the number of such shells times the shooter's
//! typical damage per shell in this battle (median of its shells that did damage; else the same
//! tank type's, else tanks of its tier, else all). A shell the recording saw fired but whose
//! target it couldn't see counts as missed.

use std::collections::HashMap;

use super::battle::{Battle, EventKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// Damage dealt (any cause).
    Dealt(i32),
    /// A shell that hit without damage, valued at the shooter's typical damage.
    Blocked(f32),
    /// A shell that hit nothing, valued the same way.
    Missed(f32),
}

/// One outcome at a time, for a team.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub t: f32,
    pub team: u8,
    pub outcome: Outcome,
}

/// Totals for one team over some time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Totals {
    pub dealt: i32,
    pub blocked: f32,
    pub missed: f32,
}

/// Everything that happened, in time order (made once per battle version; see [`ShotStats::key`]).
#[derive(Debug, Clone, Default)]
pub struct ShotStats {
    pub entries: Vec<Entry>,
    key: (usize, usize, usize, usize),
}

fn median(v: &mut [i32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2] as f32)
}

impl ShotStats {
    /// What the stats were made from: when it changes (live data, another viewpoint), make them again.
    pub fn key(b: &Battle) -> (usize, usize, usize, usize) {
        (b.events.len(), b.shots.len(), b.hits.len(), b.vehicles.len())
    }

    pub fn is_for(&self, b: &Battle) -> bool {
        self.key == Self::key(b) && (self.key != (0, 0, 0, 0) || b.vehicles.is_empty())
    }

    pub fn new(b: &Battle) -> Self {
        let team_of = |id: i32| b.vehicles.get(&id).map(|v| v.team);
        let mut entries = Vec::new();
        // Damage events, by attacker and target, in time order: shells are matched to them.
        let mut damage: HashMap<(i32, i32), Vec<(f32, i32, bool)>> = HashMap::new();
        for e in &b.events {
            if let EventKind::Damage { attacker, target, amount } = e.kind {
                if let Some(team) = team_of(attacker) {
                    entries.push(Entry { t: e.t, team, outcome: Outcome::Dealt(amount) });
                }
                damage.entry((attacker, target)).or_default().push((e.t, amount, false));
            }
        }
        // Each shell: did damage (the first unused damage by its shooter to the tank it hit,
        // within a few seconds), hit without damage, or hit nothing.
        enum Kind {
            Pen,
            Blocked,
            Missed,
        }
        let mut kinds = Vec::with_capacity(b.shots.len());
        let mut pens: HashMap<i32, Vec<i32>> = HashMap::new();
        for s in &b.shots {
            let kind = match s.hit {
                Some(target) => {
                    let found = damage.get_mut(&(s.shooter, target)).and_then(|list| list.iter_mut().find(|d| !d.2 && d.0 >= s.t - 0.05 && d.0 <= s.t.max(s.end_t) + 1.0));
                    match found {
                        Some(d) => {
                            d.2 = true;
                            pens.entry(s.shooter).or_default().push(d.1);
                            Kind::Pen
                        }
                        None => Kind::Blocked,
                    }
                }
                None => Kind::Missed,
            };
            kinds.push(kind);
        }
        // Typical damage per shell: the shooter's own, else its tank type's, its tier's, everyone's.
        let vt = |id: i32| b.vehicles.get(&id).map(|v| (v.vehicle_type.clone(), v.tier));
        let mut by_type: HashMap<String, Vec<i32>> = HashMap::new();
        let mut by_tier: HashMap<u8, Vec<i32>> = HashMap::new();
        let mut all = Vec::new();
        for (shooter, amounts) in &pens {
            if let Some((ty, tier)) = vt(*shooter) {
                by_type.entry(ty).or_default().extend(amounts);
                by_tier.entry(tier).or_default().extend(amounts);
            }
            all.extend(amounts);
        }
        let all = median(&mut all).unwrap_or(0.0);
        let mut alpha_cache: HashMap<i32, f32> = HashMap::new();
        let mut alpha = |shooter: i32| -> f32 {
            *alpha_cache.entry(shooter).or_insert_with(|| {
                let own = pens.get(&shooter).and_then(|v| median(&mut v.clone()));
                let (ty, tier) = vt(shooter).unwrap_or_default();
                own.or_else(|| by_type.get(&ty).and_then(|v| median(&mut v.clone())))
                    .or_else(|| by_tier.get(&tier).and_then(|v| median(&mut v.clone())))
                    .unwrap_or(all)
            })
        };
        for (s, kind) in b.shots.iter().zip(kinds) {
            let Some(team) = team_of(s.shooter) else { continue };
            let outcome = match kind {
                Kind::Pen => continue, // counted with the damage itself
                Kind::Blocked => Outcome::Blocked(alpha(s.shooter)),
                Kind::Missed => Outcome::Missed(alpha(s.shooter)),
            };
            entries.push(Entry { t: s.t, team, outcome });
        }
        entries.sort_by(|a, b| a.t.total_cmp(&b.t));
        ShotStats { entries, key: Self::key(b) }
    }

    /// A team's totals from `from` to `to` (battle time).
    pub fn totals(&self, team: u8, from: f32, to: f32) -> Totals {
        let start = self.entries.partition_point(|e| e.t < from);
        let mut out = Totals::default();
        for e in self.entries[start..].iter().take_while(|e| e.t <= to).filter(|e| e.team == team) {
            match e.outcome {
                Outcome::Dealt(n) => out.dealt += n,
                Outcome::Blocked(v) => out.blocked += v,
                Outcome::Missed(v) => out.missed += v,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::battle::{Event, Shot, VehicleRec};

    #[test]
    fn dealt_blocked_missed() {
        let mut b = Battle::default();
        for (id, team) in [(1, 1), (2, 2), (3, 1)] {
            b.vehicles.insert(id, VehicleRec { id, team, vehicle_type: format!("t{id}"), tier: 10, ..Default::default() });
        }
        let shot = |t: f32, shooter: i32, hit: Option<i32>| Shot { t, shooter, end_t: t + 0.3, hit, ..Default::default() };
        let dmg = |t: f32, attacker: i32, target: i32, amount: i32| Event { t, kind: EventKind::Damage { attacker, target, amount } };
        // Tank 1 (team 1): two shells that did 400 and 380, one that bounced, one that missed.
        b.shots = vec![shot(10.0, 1, Some(2)), shot(20.0, 1, Some(2)), shot(30.0, 1, Some(2)), shot(40.0, 1, None)];
        b.events = vec![dmg(10.3, 1, 2, 400), dmg(20.3, 1, 2, 380)];
        // Tank 3 (team 1) never did damage: valued like everyone else (median 400).
        b.shots.push(shot(50.0, 3, None));
        // Fire damage (no shell) still counts as dealt.
        b.events.push(dmg(60.0, 3, 2, 50));
        // Tank 2 (team 2) rammed: dealt, no shells.
        b.events.push(dmg(70.0, 2, 1, 30));
        b.events.sort_by(|a, c| a.t.total_cmp(&c.t));
        let s = ShotStats::new(&b);
        assert!(s.is_for(&b));
        assert_eq!(s.totals(1, 0.0, 100.0), Totals { dealt: 830, blocked: 400.0, missed: 800.0 });
        assert_eq!(s.totals(2, 0.0, 100.0), Totals { dealt: 30, blocked: 0.0, missed: 0.0 });
        // The last minute only.
        assert_eq!(s.totals(1, 40.0, 100.0), Totals { dealt: 50, blocked: 0.0, missed: 800.0 });
        b.shots.push(shot(80.0, 2, None));
        assert!(!s.is_for(&b));
    }
}
