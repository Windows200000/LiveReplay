//! Abuse limits per visitor, kept in memory only.
//!
//! Uploader keys cost nothing to make, so limits per key alone bound nothing: these limits are per
//! network address (IPv4 address, IPv6 /64). Addresses are never stored as they are: only a hash
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
            Daily::NewAssets => env("IP_NEW_ASSETS_PER_DAY", 3000),
            Daily::Bytes => env("IP_UPLOAD_MB_PER_DAY", 4096) << 20,
        }
    }
}

/// Requests per second one address may make on average, and in a burst (env `RATE_PER_SEC`,
/// `RATE_BURST`). High on purpose: a browser loading a 3D battle fetches dozens of files at once.
fn rate() -> (f64, f64) {
    let env = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    (env("RATE_PER_SEC", 25.0), env("RATE_BURST", 600.0))
}

/// Different viewing codes that may fail per address and hour (guessing codes).
const BAD_CODES_PER_HOUR: usize = 20;
/// Different uploader keys whose asset votes count per address and day.
const VOTERS_PER_DAY: usize = 4;

pub type Visitor = u64;

struct Bucket {
    tokens: f64,
    at: Instant,
}

#[derive(Default)]
struct Inner {
    day: u64,
    hour: u64,
    buckets: HashMap<Visitor, Bucket>,
    daily: HashMap<(Visitor, Daily), u64>,
    voters: HashMap<Visitor, HashSet<String>>,
    bad_codes: HashMap<Visitor, HashSet<String>>,
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
        let mut h = sha2::Sha256::new();
        h.update(self.salt);
        match ip {
            // A whole /64 is one visitor (one connection gets one).
            Some(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_none() => h.update(&v6.octets()[..8]),
            Some(IpAddr::V6(v6)) => h.update(v6.to_ipv4_mapped().unwrap().octets()),
            Some(IpAddr::V4(v4)) => h.update(v4.octets()),
            None => h.update(b"unknown"),
        }
        u64::from_le_bytes(h.finalize()[..8].try_into().unwrap())
    }

    /// One more request; false when the visitor sends faster than [`rate`] allows.
    pub fn request(&self, v: Visitor) -> bool {
        let (per_sec, burst) = rate();
        let mut g = self.lock();
        let now = Instant::now();
        let b = g.buckets.entry(v).or_insert(Bucket { tokens: burst, at: now });
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * per_sec).min(burst);
        b.at = now;
        if b.tokens < 1.0 {
            return false;
        }
        b.tokens -= 1.0;
        true
    }

    /// Count `n` more of `what` today; false (nothing counted) when it would exceed the limit.
    pub fn take(&self, v: Visitor, what: Daily, n: u64) -> bool {
        let mut g = self.lock();
        let c = g.daily.entry((v, what)).or_insert(0);
        if *c + n > what.limit() {
            return false;
        }
        *c += n;
        true
    }

    /// Whether `n` more of `what` would still fit today (nothing counted).
    pub fn fits(&self, v: Visitor, what: Daily, n: u64) -> bool {
        let g = self.lock();
        g.daily.get(&(v, what)).copied().unwrap_or(0) + n <= what.limit()
    }

    /// Whether this visitor may still try viewing codes (not too many wrong ones this hour).
    pub fn may_try_codes(&self, v: Visitor) -> bool {
        self.lock().bad_codes.get(&v).map_or(true, |s| s.len() < BAD_CODES_PER_HOUR)
    }

    /// A code that matched nothing (each different code counts once: a browser that keeps
    /// sending an expired one isn't guessing).
    pub fn bad_code(&self, v: Visitor, code_hash: &str) {
        let mut g = self.lock();
        let s = g.bad_codes.entry(v).or_default();
        if s.len() < BAD_CODES_PER_HOUR {
            s.insert(code_hash.to_string());
        }
    }

    /// Whether this uploader key's asset votes count from this visitor (at most a few keys per
    /// address and day, so making many keys doesn't make many votes).
    pub fn voter(&self, v: Visitor, key: &str) -> bool {
        let mut g = self.lock();
        let s = g.voters.entry(v).or_default();
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
    }
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
        // One IPv6 /64 is one visitor.
        assert_eq!(g.visitor(Some(&proxy), Some("2001:db8:1:2::1")), g.visitor(Some(&proxy), Some("2001:db8:1:2:ffff::9")));
        assert_ne!(g.visitor(Some(&proxy), Some("2001:db8:1:2::1")), g.visitor(Some(&proxy), Some("2001:db8:1:3::1")));
    }

    #[test]
    fn limits() {
        let g = Guard::new();
        assert!(g.take(1, Daily::Claims, 10));
        assert!(!g.take(1, Daily::Claims, 1));
        assert!(g.take(2, Daily::Claims, 1));
        for k in 0..VOTERS_PER_DAY {
            assert!(g.voter(1, &k.to_string()));
        }
        assert!(!g.voter(1, "another"));
        assert!(g.voter(1, "0"));
        for c in 0..BAD_CODES_PER_HOUR {
            assert!(g.may_try_codes(1));
            g.bad_code(1, &c.to_string());
        }
        assert!(!g.may_try_codes(1));
        let mut n = 0;
        while g.request(3) {
            n += 1;
        }
        assert!(n >= 600);
    }
}
