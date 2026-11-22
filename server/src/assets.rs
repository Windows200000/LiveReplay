//! Shared game data (map packs, icons, 3D packs), uploaded by the desktop apps.
//!
//! Anyone could upload garbage for an asset before the real data arrives, so uploads are votes:
//! the first copy is served at once, every different copy is kept as a candidate (a few per
//! asset), and each uploader key has one vote per asset — the copy it uploaded or voted for
//! last. The copy with the most votes is served. Votes only count from keys that uploaded a
//! battle, a few keys per address and day (see `guard`), and expire, so after a game update the
//! new data takes over as players' apps vote for it.
//!
//! Files: `assets/<name>.bin` (served copy) + `assets/<name>.json` ([`AssetMeta`]); candidates in
//! `assets/candidates/<name>.<hash prefix>.bin`.

use serde::{Deserialize, Serialize};

/// Copies kept per asset (the served one and the candidates).
pub const MAX_COPIES: usize = 4;
/// A vote counts for this long (votes are renewed whenever an app uploads for a new game version).
pub const VOTE_SECS: u64 = 120 * 86400;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Vote {
    /// Hash of the uploader key.
    pub key: String,
    /// Unix seconds.
    pub at: u64,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Copy {
    /// SHA-256 (hex) of the data; empty for assets stored before hashes were kept (worked out
    /// on first use).
    #[serde(default)]
    pub hash: String,
    pub version: String,
    pub size: u64,
    /// Hash of the key that first uploaded this copy (counts towards its storage).
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub votes: Vec<Vote>,
}

/// One asset: the served copy (its fields at the top, as in older files) and the candidates.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct AssetMeta {
    #[serde(flatten)]
    pub current: Copy,
    #[serde(default)]
    pub others: Vec<Copy>,
    /// The operator chose the served copy: votes are still recorded but change nothing.
    #[serde(default)]
    pub pinned: bool,
}

impl AssetMeta {
    pub fn copies(&self) -> impl Iterator<Item = &Copy> {
        std::iter::once(&self.current).chain(self.others.iter())
    }

    pub fn has(&self, hash: &str) -> bool {
        self.copies().any(|c| c.hash == hash)
    }

    /// Give `key`'s one vote to the copy with `hash` (which must be here). Returns the index in
    /// `others` of the copy that now has more votes than the served one, if any: the caller
    /// swaps the files and calls [`AssetMeta::promote`].
    pub fn vote(&mut self, key: &str, hash: &str, t: u64) -> Option<usize> {
        for c in std::iter::once(&mut self.current).chain(self.others.iter_mut()) {
            c.votes.retain(|v| v.key != key && t.saturating_sub(v.at) < VOTE_SECS);
            if c.hash == hash {
                c.votes.push(Vote { key: key.to_string(), at: t });
            }
        }
        self.leader()
    }

    /// A candidate with more votes than the served copy (strictly: ties keep what is served).
    pub fn leader(&self) -> Option<usize> {
        if self.pinned {
            return None;
        }
        let (i, best) = self.others.iter().enumerate().max_by_key(|(_, c)| c.votes.len())?;
        (best.votes.len() > self.current.votes.len()).then_some(i)
    }

    /// Serve candidate `i` from now on (the served copy becomes a candidate).
    pub fn promote(&mut self, i: usize) {
        std::mem::swap(&mut self.current, &mut self.others[i]);
    }

    /// Room for one more candidate: `Ok(None)` if there is a free place, `Ok(Some(i))` if
    /// candidate `i` (at most one vote) should go to make room, `Err` if every candidate has
    /// real support.
    pub fn room(&self) -> Result<Option<usize>, ()> {
        if self.others.len() + 1 < MAX_COPIES {
            return Ok(None);
        }
        let (i, weakest) = self.others.iter().enumerate().min_by_key(|(_, c)| c.votes.len()).ok_or(())?;
        if weakest.votes.len() <= 1 {
            Ok(Some(i))
        } else {
            Err(())
        }
    }

    /// Remove a key's votes everywhere (blocked keys).
    pub fn forget(&mut self, key: &str) {
        for c in std::iter::once(&mut self.current).chain(self.others.iter_mut()) {
            c.votes.retain(|v| v.key != key);
        }
    }

    /// Drop expired votes; true when something changed.
    pub fn expire(&mut self, t: u64) -> bool {
        let mut changed = false;
        for c in std::iter::once(&mut self.current).chain(self.others.iter_mut()) {
            let n = c.votes.len();
            c.votes.retain(|v| t.saturating_sub(v.at) < VOTE_SECS);
            changed |= n != c.votes.len();
        }
        changed
    }
}

/// File name of a candidate copy (the whole hash: copies can't be made to share a file).
pub fn candidate_file(name: &str, hash: &str) -> String {
    format!("{name}.{hash}.bin")
}

pub fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(hash: &str) -> Copy {
        Copy { hash: hash.into(), version: "1".into(), size: 10, ..Default::default() }
    }

    #[test]
    fn votes() {
        // The first copy is served; garbage first, then the real data from two players.
        let mut m = AssetMeta { current: copy("bad"), others: vec![copy("good")], pinned: false };
        assert_eq!(m.vote("attacker", "bad", 100), None);
        assert_eq!(m.vote("p1", "good", 100), None); // 1:1, ties keep what is served
        assert_eq!(m.vote("p2", "good", 100), Some(0));
        m.promote(0);
        assert_eq!(m.current.hash, "good");
        // One key, one vote: voting again moves it rather than adding one.
        m.vote("attacker", "bad", 101);
        m.vote("attacker", "bad", 102);
        assert_eq!(m.others[0].votes.len(), 1);
        m.vote("p1", "bad", 103);
        assert_eq!((m.current.votes.len(), m.others[0].votes.len()), (1, 2));
        assert_eq!(m.leader(), Some(0));
        // Votes expire.
        m.vote("p3", "good", 100 + VOTE_SECS + 5);
        assert_eq!((m.current.votes.len(), m.others[0].votes.len()), (1, 0));
        m.forget("p3");
        assert!(m.current.votes.is_empty());
    }

    #[test]
    fn room() {
        let mut m = AssetMeta { current: copy("a"), others: vec![copy("b")], pinned: false };
        assert_eq!(m.room(), Ok(None));
        m.others.push(copy("c"));
        assert_eq!(m.room(), Ok(None));
        m.others.push(copy("d"));
        m.vote("k1", "b", 1);
        m.vote("k2", "b", 1);
        m.vote("k3", "d", 1);
        m.vote("k4", "d", 1);
        // "c" has no votes: it makes room.
        assert_eq!(m.room(), Ok(Some(1)));
        m.vote("k5", "c", 1);
        m.vote("k6", "c", 1);
        assert_eq!(m.room(), Err(()));
        // Pinned by the operator: votes don't move it.
        m.pinned = true;
        for k in ["x1", "x2", "x3"] {
            assert_eq!(m.vote(k, "b", 2), None);
        }
    }

    #[test]
    fn old_files() {
        let m: AssetMeta = serde_json::from_str(r#"{"version":"1.2","size":5,"owner":"x"}"#).unwrap();
        assert_eq!((m.current.version.as_str(), m.current.size, m.current.hash.as_str(), m.others.len()), ("1.2", 5, "", 0));
        let back: AssetMeta = serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(back.current.owner, "x");
    }
}
