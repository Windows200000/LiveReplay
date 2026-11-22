//! Pacing for watching a live battle: new data arrives in chunks (the app uploads every few
//! seconds, the website fetches every second), so playing right at the newest moment would stop
//! at the end of each chunk and jump when the next one arrives.
//!
//! Instead playback stays a little behind the newest data: just far enough that the next chunk
//! normally arrives before playback gets there. How far is learnt from how far apart chunks
//! actually arrive (the largest recent gap plus a margin), so it settles at the smallest delay
//! that keeps playback smooth. Playback runs a few percent faster or slower to reach that delay
//! gradually, never jumps unless it is far off, and becomes more careful whenever it ran dry.

/// Delay before anything was measured (app upload interval + fetching).
const START_DELAY: f64 = 5.0;
const MIN_DELAY: f64 = 1.0;
const MAX_DELAY: f64 = 30.0;
/// Added to the largest recent gap between chunks.
const MARGIN: f64 = 0.75;
/// Gaps remembered (a minute or so of chunks).
const WINDOW: usize = 16;
/// Largest speed change used to reach the target delay (fraction of real time).
const MAX_ADJUST: f64 = 0.15;
/// Further than this from the target: jump there instead of catching up.
const JUMP: f64 = 15.0;

#[derive(Debug, Clone)]
pub struct LivePace {
    /// Wall-clock time (seconds) of the last chunk that brought new data.
    last_arrival: Option<f64>,
    /// Battle time covered by the data so far.
    newest: f64,
    gaps: Vec<f64>,
    /// Extra delay after playback ran out of data (decays again while things go well).
    extra: f64,
}

impl Default for LivePace {
    fn default() -> Self {
        LivePace { last_arrival: None, newest: 0.0, gaps: Vec::new(), extra: 0.0 }
    }
}

impl LivePace {
    /// How far behind the newest data playback aims to be (seconds).
    pub fn delay(&self) -> f64 {
        let base = if self.gaps.is_empty() { START_DELAY } else { self.gaps.iter().copied().fold(0.0, f64::max) + MARGIN };
        (base + self.extra).clamp(MIN_DELAY, MAX_DELAY)
    }

    /// Where to start watching when the battle so far is `duration` seconds long.
    pub fn start(&mut self, duration: f64, now: f64) -> f64 {
        self.newest = duration;
        self.last_arrival = Some(now);
        (duration - self.delay()).max(0.0)
    }

    /// New data arrived at wall-clock time `now`: the battle is now `duration` long.
    pub fn arrived(&mut self, duration: f64, now: f64) {
        if duration <= self.newest + 1e-3 {
            return;
        }
        if let Some(prev) = self.last_arrival {
            self.gaps.push((now - prev).clamp(0.0, MAX_DELAY));
            if self.gaps.len() > WINDOW {
                self.gaps.remove(0);
            }
        }
        self.last_arrival = Some(now);
        self.newest = duration;
    }

    /// Advance playback from `t` by `dt` seconds of real time; `duration` is the newest data.
    /// Returns the new time.
    pub fn advance(&mut self, t: f64, dt: f64, duration: f64) -> f64 {
        let target = (duration - self.delay()).max(0.0);
        let behind = target - t;
        if behind.abs() > JUMP {
            return target;
        }
        // Proportional: 1 s off the target -> 5 % faster / slower.
        let speed = 1.0 + (behind * 0.05).clamp(-MAX_ADJUST, MAX_ADJUST);
        let next = t + dt * speed;
        if next >= duration {
            // Ran dry: the next chunk was later than expected. Wait, and keep more in hand.
            self.extra = (self.extra + dt).min(MAX_DELAY);
            return duration;
        }
        // Going well: slowly give the extra delay back.
        self.extra = (self.extra - dt * 0.01).max(0.0);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chunks every `every` s (with `jitter`), playback simulated at 60 fps: how often it stalls
    /// and how far behind it ends up.
    fn run(every: f64, jitter: f64, secs: f64) -> (usize, f64, f64) {
        let mut p = LivePace::default();
        let (mut now, mut duration, mut next_chunk) = (0.0, 30.0, every);
        let mut t = p.start(duration, now);
        let mut stalls = 0;
        let mut k = 0u32;
        let dt = 1.0 / 60.0;
        while now < secs {
            now += dt;
            if now >= next_chunk {
                k += 1;
                // Deterministic jitter.
                let j = jitter * ((k * 7919 % 13) as f64 / 12.0 - 0.5) * 2.0;
                duration = 30.0 + now - j.abs();
                p.arrived(duration, now);
                next_chunk = now + every + j;
            }
            let before = t;
            t = p.advance(t, dt, duration);
            if now > 120.0 && t - before < dt * 0.5 {
                stalls += 1;
            }
        }
        (stalls, duration - t, p.delay())
    }

    #[test]
    fn settles_without_stalls() {
        // Website: app uploads every 4 s, fetched every 1 s -> arrivals 4-5 s apart.
        let (stalls, behind, delay) = run(4.5, 0.5, 400.0);
        assert_eq!(stalls, 0, "stalls after settling");
        assert!(behind < 7.5 && delay < 7.0, "behind {behind}, delay {delay}");
        // Desktop: data every 0.5 s -> stays close.
        let (stalls, behind, _) = run(0.5, 0.1, 300.0);
        assert_eq!(stalls, 0);
        assert!(behind < 2.0, "behind {behind}");
    }

    #[test]
    fn far_behind_jumps() {
        let mut p = LivePace::default();
        p.start(10.0, 0.0);
        assert_eq!(p.advance(0.0, 0.016, 300.0), 300.0 - p.delay());
    }
}
