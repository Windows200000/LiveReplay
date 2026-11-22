//! Abuse limits per visitor, kept in memory only.
//!
//! Uploader keys cost nothing to make, so limits per key alone bound nothing: these limits are per
//! network address. IPv4: one address. IPv6: request rates per /64 (one connection), daily counts
//! per /48 (one customer; a /48 is easy to get, a /64 trivial). Addresses are never stored as they are: only a hash
//! with a random salt made at each start, kept in memory, and every count is forgotten after a
//! day (request rates after a few minutes). Nothing here is written to disk.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Instant;

use sha2::Digest;

/// Things counted per address and day.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Daily {
    /// New battle entries.
    Battles,
    /// Player names claimed (the first upload for a name).
    Claims,
    /// Viewing codes created.
    Codes,
    /// Shared game data names that didn't exist before.
    NewAssets,
    /// Bytes uploaded (battle data, replays, game data).
    Bytes,
}

impl Daily {
    fn limit(self) -> u64 {
        let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        match self {
            Daily::Battles => env("IP_BATTLES_PER_DAY", 400),
            Daily::Claims => env("IP_NAMES_PER_DAY", 10),
            Daily::Codes => env("IP_CODES_PER_DAY", 60),
            Daily::NewAssets => env("IP_NEW_ASSETS_PER_DAY", 500),
            Daily::Bytes => env("IP_UPLOAD_MB_PER_DAY", 1024) << 20,
        }
    }
}

/// Requests per second one address may make on average, and in a burst (env `RATE_PER_SEC`,
/// `RATE_BURST`). High on purpose: a browser loading a 3D battle fetches dozens of files at once.
fn rate() -> (f64, f64) {
    let env = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    (env("RATE_PER_SEC", 25.0), env("RATE_BURST", 600.0))
}

/// Requests one visitor may have running at once (slow senders / readers can't take every worker).
fn max_in_flight() -> u32 {
    std::env::var("MAX_IN_FLIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(8)
}

/// Requests that make the server look through everything (a key's data, new battles, votes,
/// uncached lists): per second and burst, per visitor.
const HEAVY_PER_SEC: f64 = 2.0;
const HEAVY_BURST: f64 = 30.0;

/// Different viewing codes that may fail per address and hour (guessing codes).
const BAD_CODES_PER_HOUR: usize = 20;
/// Different uploader keys whose asset votes count per address and day.
const VOTERS_PER_DAY: usize = 4;

/// One visitor: `rate` (request rate, requests at once) and `quota` (daily counts; the same as
/// `rate` for IPv4, the /48 for IPv6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Visitor {
    pub rate: u64,
    pub quota: u64,
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

#[derive(Default)]
struct Inner {
    day: u64,
    hour: u64,
    buckets: HashMap<u64, Bucket>,
    heavy: HashMap<u64, Bucket>,
    in_flight: HashMap<u64, u32>,
    daily: HashMap<(u64, Daily), u64>,
    voters: HashMap<u64, HashSet<String>>,
    bad_codes: HashMap<u64, HashSet<String>>,
}

pub struct Guard {
    salt: [u8; 32],
    inner: Mutex<Inner>,
}

fn day_hour() -> (u64, u64) {
    let h = super::now() / 3600;
    (h / 24, h)
}

impl Guard {
    pub fn new() -> Self {
        let mut salt = [0u8; 32];
        getrandom::fill(&mut salt).expect("os random");
        Guard { salt, inner: Mutex::new(Inner::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (day, hour) = day_hour();
        if g.day != day {
            g.day = day;
            g.daily.clear();
            g.voters.clear();
        }
        if g.hour != hour {
            g.hour = hour;
            g.bad_codes.clear();
        }
        g
    }

    /// The visitor behind a request: the connecting address, or, when that is the reverse proxy
    /// (a private or loopback address), the last `X-Forwarded-For` entry it set.
    pub fn visitor(&self, remote: Option<&SocketAddr>, forwarded: Option<&str>) -> Visitor {
        let direct = remote.map(|a| a.ip());
        let ip = match direct {
            Some(ip) if is_private(ip) => forwarded.and_then(|f| f.rsplit(',').next()).and_then(|s| s.trim().parse::<IpAddr>().ok()).or(direct),
            other => other,
        };
        let hash = |b: &[u8]| {
            let mut h = sha2::Sha256::new();
            h.update(self.salt);
            h.update(b);
            u64::from_le_bytes(h.finalize()[..8].try_into().unwrap())
        };
        match ip {
            Some(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_none() => Visitor { rate: hash(&v6.octets()[..8]), quota: hash(&v6.octets()[..6]) },
            Some(IpAddr::V6(v6)) => {
                let v = hash(&v6.to_ipv4_mapped().unwrap().octets());
                Visitor { rate: v, quota: v }
            }
            Some(IpAddr::V4(v4)) => {
                let v = hash(&v4.octets());
                Visitor { rate: v, quota: v }
            }
            None => Visitor { rate: 0, quota: 0 },
        }
    }

    /// One more request; false when the visitor sends faster than [`rate`] allows.
    pub fn request(&self, v: Visitor) -> bool {
        let (per_sec, burst) = rate();
        take_token(&mut self.lock().buckets, v.rate, per_sec, burst)
    }

    /// One more expensive request (see [`HEAVY_PER_SEC`]); false when there were too many.
    pub fn heavy(&self, v: Visitor) -> bool {
        take_token(&mut self.lock().heavy, v.rate, HEAVY_PER_SEC, HEAVY_BURST)
    }

    /// A request starts: `None` when this visitor already has [`max_in_flight`] running. The
    /// returned guard ends it.
    pub fn enter(&self, v: Visitor) -> Option<InFlight<'_>> {
        let mut g = self.lock();
        let n = g.in_flight.entry(v.rate).or_insert(0);
        if *n >= max_in_flight() {
            return None;
        }
        *n += 1;
        Some(InFlight(self, v.rate))
    }

    /// Count `n` more of `what` today; false (nothing counted) when it would exceed the limit.
    pub fn take(&self, v: Visitor, what: Daily, n: u64) -> bool {
        let mut g = self.lock();
        let c = g.daily.entry((v.quota, what)).or_insert(0);
        if *c + n > what.limit() {
            return false;
        }
        *c += n;
        true
    }

    /// Whether `n` more of `what` would still fit today (nothing counted).
    pub fn fits(&self, v: Visitor, what: Daily, n: u64) -> bool {
        let g = self.lock();
        g.daily.get(&(v.quota, what)).copied().unwrap_or(0) + n <= what.limit()
    }

    /// Whether this visitor may still try viewing codes (not too many wrong ones this hour).
    pub fn may_try_codes(&self, v: Visitor) -> bool {
        self.lock().bad_codes.get(&v.quota).map_or(true, |s| s.len() < BAD_CODES_PER_HOUR)
    }

    /// A code that matched nothing (each different code counts once: a browser that keeps
    /// sending an expired one isn't guessing).
    pub fn bad_code(&self, v: Visitor, code_hash: &str) {
        let mut g = self.lock();
        let s = g.bad_codes.entry(v.quota).or_default();
        if s.len() < BAD_CODES_PER_HOUR {
            s.insert(code_hash.to_string());
        }
    }

    /// Whether this uploader key's asset votes count from this visitor (at most a few keys per
    /// address and day, so making many keys doesn't make many votes).
    pub fn voter(&self, v: Visitor, key: &str) -> bool {
        let mut g = self.lock();
        let s = g.voters.entry(v.quota).or_default();
        if s.contains(key) {
            return true;
        }
        if s.len() >= VOTERS_PER_DAY {
            return false;
        }
        s.insert(key.to_string());
        true
    }

    /// Drop rate buckets that are full again (memory stays small).
    pub fn tidy(&self) {
        let (per_sec, burst) = rate();
        let mut g = self.lock();
        let now = Instant::now();
        g.buckets.retain(|_, b| b.tokens + now.duration_since(b.at).as_secs_f64() * per_sec < burst);
        g.heavy.retain(|_, b| b.tokens + now.duration_since(b.at).as_secs_f64() * HEAVY_PER_SEC < HEAVY_BURST);
        g.in_flight.retain(|_, n| *n > 0);
    }
}

/// A running request (see [`Guard::enter`]).
pub struct InFlight<'a>(&'a Guard, u64);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Some(n) = self.0.lock().in_flight.get_mut(&self.1) {
            *n = n.saturating_sub(1);
        }
    }
}

/// Token bucket: one token per request, `per_sec` back every second, at most `burst`.
fn take_token(buckets: &mut HashMap<u64, Bucket>, key: u64, per_sec: f64, burst: f64) -> bool {
    let now = Instant::now();
    let b = buckets.entry(key).or_insert(Bucket { tokens: burst, at: now });
    b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * per_sec).min(burst);
    b.at = now;
    if b.tokens < 1.0 {
        return false;
    }
    b.tokens -= 1.0;
    true
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || v.to_ipv4_mapped().is_some_and(|m| m.is_loopback() || m.is_private()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visitors() {
        let g = Guard::new();
        let proxy: SocketAddr = "172.18.0.3:5000".parse().unwrap();
        let public: SocketAddr = "203.0.113.9:5000".parse().unwrap();
        // Behind the proxy: its last X-Forwarded-For entry (the one Caddy adds).
        assert_eq!(g.visitor(Some(&proxy), Some("1.2.3.4, 198.51.100.7")), g.visitor(Some(&proxy), Some("198.51.100.7")));
        assert_ne!(g.visitor(Some(&proxy), Some("198.51.100.7")), g.visitor(Some(&proxy), Some("198.51.100.8")));
        // A direct public connection can't pretend to be someone else.
        assert_eq!(g.visitor(Some(&public), Some("198.51.100.7")), g.visitor(Some(&public), None));
        // IPv6: rates per /64, daily counts per /48.
        let (a, b, c) = (g.visitor(Some(&proxy), Some("2001:db8:1:2::1")), g.visitor(Some(&proxy), Some("2001:db8:1:2:ffff::9")), g.visitor(Some(&proxy), Some("2001:db8:1:3::1")));
        assert_eq!(a, b);
        assert_ne!(a.rate, c.rate);
        assert_eq!(a.quota, c.quota);
        assert_ne!(a.quota, g.visitor(Some(&proxy), Some("2001:db8:2:3::1")).quota);
    }

    fn v(n: u64) -> Visitor {
        Visitor { rate: n, quota: n }
    }

    #[test]
    fn limits() {
        let g = Guard::new();
        // Requests at once.
        let held: Vec<_> = (0..8).map(|_| g.enter(v(5)).unwrap()).collect();
        assert!(g.enter(v(5)).is_none() && g.enter(v(6)).is_some());
        drop(held);
        assert!(g.enter(v(5)).is_some());
        assert!(g.take(v(1), Daily::Claims, 10));
        assert!(!g.take(v(1), Daily::Claims, 1));
        assert!(g.take(v(2), Daily::Claims, 1));
        for k in 0..VOTERS_PER_DAY {
            assert!(g.voter(v(1), &k.to_string()));
        }
        assert!(!g.voter(v(1), "another"));
        assert!(g.voter(v(1), "0"));
        for c in 0..BAD_CODES_PER_HOUR {
            assert!(g.may_try_codes(v(1)));
            g.bad_code(v(1), &c.to_string());
        }
        assert!(!g.may_try_codes(v(1)));
        let mut n = 0;
        while g.request(v(3)) {
            n += 1;
        }
        assert!(n >= 600);
    }
}
