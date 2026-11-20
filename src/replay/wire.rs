//! Compact binary form of a [`Battle`], used between the desktop client, the server and the
//! web viewer. The desktop client decodes the replay (it has the game files), the server only
//! stores what it gets, the browser only decodes this.
//!
//! A battle file is a sequence of frames: `u32 length (LE)` + format byte + deflate(CBOR([`Chunk`])).
//! CBOR names every field, so data stays readable when fields are added or removed later.
//! A finished replay is one frame. A live battle grows by one frame every few seconds; each frame
//! carries everything from its `from` time on, so a viewer drops what it had from then and appends.
//! Movement tracks and gun codes are quantised and delta-coded (cm, ms, 1e-4 rad).

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use super::battle::{Battle, Sample};

#[derive(Serialize, Deserialize)]
pub struct Chunk {
    /// Replay time this chunk's time series start at (0 = complete battle).
    pub from: f32,
    pub battle: Battle,
}

/// `t` is at or after `from` (compared in whole ms, as stored).
fn ge(t: f32, from: f32) -> bool {
    q(t, MS) >= q(from, MS)
}

/// The part of `b` a viewer that already has everything before `from` needs: all time series
/// cut to `t >= from`, everything else (names, bases, periods, events' state) in full.
pub fn slice(b: &Battle, from: f32) -> Battle {
    let mut out = b.clone();
    if from <= 0.0 {
        return out;
    }
    for v in out.vehicles.values_mut() {
        v.track.retain(|s| ge(s.t, from));
        v.visibility.retain(|s| ge(s.0, from));
        v.health.retain(|s| ge(s.0, from));
        v.turret_yaw.retain(|s| ge(s.0, from));
        v.gun_codes.retain(|s| ge(s.0, from));
        v.shot_pitch.retain(|s| ge(s.0, from));
        v.crits.retain(|c| ge(c.t, from));
    }
    out.events.retain(|e| ge(e.t, from));
    out.shots.retain(|s| ge(s.t, from));
    out.hits.retain(|h| ge(h.t, from));
    out.captures.retain(|c| ge(c.t, from));
    out
}

/// Add a chunk to what a viewer has: keep its own data before `chunk.from`, take the rest from the chunk.
pub fn apply(base: Option<Battle>, chunk: Chunk) -> Battle {
    let Some(mut old) = base.filter(|_| chunk.from > 0.0) else { return chunk.battle };
    let from = chunk.from;
    let mut b = chunk.battle;
    fn join<T: Clone>(old: &mut Vec<T>, new: &mut Vec<T>, keep: impl Fn(&T) -> bool) {
        let mut v: Vec<T> = old.drain(..).filter(|x| keep(x)).collect();
        v.append(new);
        *new = v;
    }
    for (id, ov) in old.vehicles.iter_mut() {
        let Some(v) = b.vehicles.get_mut(id) else { continue };
        join(&mut ov.track, &mut v.track, |s| !ge(s.t, from));
        join(&mut ov.visibility, &mut v.visibility, |s| !ge(s.0, from));
        join(&mut ov.health, &mut v.health, |s| !ge(s.0, from));
        join(&mut ov.turret_yaw, &mut v.turret_yaw, |s| !ge(s.0, from));
        join(&mut ov.gun_codes, &mut v.gun_codes, |s| !ge(s.0, from));
        join(&mut ov.shot_pitch, &mut v.shot_pitch, |s| !ge(s.0, from));
        join(&mut ov.crits, &mut v.crits, |c| !ge(c.t, from));
    }
    for (id, ov) in old.vehicles {
        b.vehicles.entry(id).or_insert(ov);
    }
    join(&mut old.events, &mut b.events, |e| !ge(e.t, from));
    join(&mut old.shots, &mut b.shots, |s| !ge(s.t, from));
    join(&mut old.hits, &mut b.hits, |h| !ge(h.t, from));
    join(&mut old.captures, &mut b.captures, |c| !ge(c.t, from));
    b
}

/// One frame (length + compressed chunk) holding the battle from `from` on.
pub fn encode_frame(b: &Battle, from: f32) -> Vec<u8> {
    let chunk = Chunk { from, battle: slice(b, from) };
    let body = pack(&chunk);
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Format byte of everything this module writes (frames, packs).
const FORMAT: u8 = 1;

/// format byte + deflate(CBOR(v)).
pub fn pack<T: Serialize>(v: &T) -> Vec<u8> {
    let mut raw = Vec::new();
    let _ = ciborium::into_writer(v, &mut raw);
    let mut e = flate2::write::DeflateEncoder::new(vec![FORMAT], flate2::Compression::best());
    let _ = e.write_all(&raw);
    e.finish().unwrap_or_default()
}

pub fn unpack<T: for<'de> Deserialize<'de>>(b: &[u8]) -> Result<T, String> {
    match b.first() {
        Some(&FORMAT) => {}
        Some(f) if *f > FORMAT => return Err("made by a newer LiveReplay version (reload the page / update the app)".into()),
        _ => return Err("unknown data format".into()),
    }
    let mut raw = Vec::new();
    flate2::read::DeflateDecoder::new(&b[1..]).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    ciborium::from_reader(raw.as_slice()).map_err(|e| e.to_string())
}

/// Decode the complete frames at the start of `data`; returns them and how many bytes they used
/// (a trailing partial frame is left for later). Unreadable frames are skipped.
pub fn decode_frames(data: &[u8]) -> (Vec<Chunk>, usize) {
    let (chunks, used, _) = decode_frames_checked(data);
    (chunks, used)
}

/// Like [`decode_frames`], plus the error of the first frame that could not be read.
pub fn decode_frames_checked(data: &[u8]) -> (Vec<Chunk>, usize, Option<String>) {
    let mut error = None;
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(len) = data.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize) {
        let Some(body) = data.get(at + 4..at + 4 + len) else { break };
        at += 4 + len;
        match unpack::<Chunk>(body) {
            Ok(mut c) => {
                for v in c.battle.vehicles.values_mut() {
                    v.turret_yaw = v.gun_codes.iter().map(|&(t, c)| (t, super::battle::decode_turret_yaw(c))).collect();
                }
                out.push(c);
            }
            Err(e) => {
                error.get_or_insert(e);
            }
        }
    }
    (out, at, error)
}

/// Decode a whole battle file.
pub fn decode_battle(data: &[u8]) -> Option<Battle> {
    decode_battle_checked(data).ok()
}

/// Decode a whole battle file, or say why it can't be read.
pub fn decode_battle_checked(data: &[u8]) -> Result<Battle, String> {
    let (chunks, _, err) = decode_frames_checked(data);
    match chunks.into_iter().fold(None, |b, c| Some(apply(b, c))) {
        Some(b) => Ok(b),
        None => Err(err.unwrap_or_else(|| "no battle data yet".into())),
    }
}

// --- varints -------------------------------------------------------------------------------

fn put(out: &mut Vec<u8>, v: i64) {
    let mut z = ((v << 1) ^ (v >> 63)) as u64;
    loop {
        let b = (z & 0x7f) as u8;
        z >>= 7;
        if z == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn get(d: &[u8], at: &mut usize) -> Option<i64> {
    let mut z = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *d.get(*at)?;
        *at += 1;
        z |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((z >> 1) as i64 ^ -((z & 1) as i64));
        }
    }
    None
}

/// Delta coder: each value is stored as the difference to the previous quantised value.
struct Delta<const N: usize>([i64; N]);
impl<const N: usize> Delta<N> {
    fn put(&mut self, out: &mut Vec<u8>, q: [i64; N]) {
        for i in 0..N {
            put(out, q[i] - self.0[i]);
        }
        self.0 = q;
    }
    fn get(&mut self, d: &[u8], at: &mut usize) -> Option<[i64; N]> {
        for i in 0..N {
            self.0[i] += get(d, at)?;
        }
        Some(self.0)
    }
}

const MS: f32 = 1000.0;
const CM: f32 = 100.0;
const RAD: f32 = 10000.0;

fn q(v: f32, s: f32) -> i64 {
    (v * s).round() as i64
}

/// `#[serde(with = "wire::track")]`: positions in cm, angles in 1e-4 rad, times in ms, delta-coded.
pub mod track {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &[Sample], s: S) -> Result<S::Ok, S::Error> {
        let mut out = Vec::with_capacity(v.len() * 10 + 4);
        put(&mut out, v.len() as i64);
        let mut d = Delta([0i64; 7]);
        for p in v {
            d.put(&mut out, [q(p.t, MS), q(p.pos[0], CM), q(p.pos[1], CM), q(p.pos[2], CM), q(p.yaw, RAD), q(p.pitch, RAD), q(p.roll, RAD)]);
        }
        serde_bytes_like(out, s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<Sample>, D::Error> {
        let b = Bytes::deserialize(de)?.0;
        let mut at = 0;
        let n = get(&b, &mut at).unwrap_or(0).max(0) as usize;
        let mut d = Delta([0i64; 7]);
        let mut out = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            let Some(v) = d.get(&b, &mut at) else { break };
            let f = |i: usize, s: f32| v[i] as f32 / s;
            out.push(Sample { t: f(0, MS), pos: [f(1, CM), f(2, CM), f(3, CM)], yaw: f(4, RAD), pitch: f(5, RAD), roll: f(6, RAD) });
        }
        Ok(out)
    }
}

/// `#[serde(with = "wire::codes")]`: (time, u16 code) pairs, delta-coded.
pub mod codes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &[(f32, u16)], s: S) -> Result<S::Ok, S::Error> {
        let mut out = Vec::with_capacity(v.len() * 3 + 4);
        put(&mut out, v.len() as i64);
        let mut d = Delta([0i64; 2]);
        for &(t, c) in v {
            d.put(&mut out, [q(t, MS), c as i64]);
        }
        serde_bytes_like(out, s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<(f32, u16)>, D::Error> {
        let b = Bytes::deserialize(de)?.0;
        let mut at = 0;
        let n = get(&b, &mut at).unwrap_or(0).max(0) as usize;
        let mut d = Delta([0i64; 2]);
        let mut out = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            let Some(v) = d.get(&b, &mut at) else { break };
            out.push((v[0] as f32 / MS, v[1] as u16));
        }
        Ok(out)
    }
}

/// `#[serde(with = "wire::json_text")]`: arbitrary JSON kept as text (binary formats can't hold `Value`).
pub mod json_text {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &Option<serde_json::Value>, s: S) -> Result<S::Ok, S::Error> {
        v.as_ref().map(|v| v.to_string()).serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<serde_json::Value>, D::Error> {
        let t: Option<String> = Deserialize::deserialize(de)?;
        Ok(t.and_then(|t| serde_json::from_str(&t).ok()))
    }
}

fn serde_bytes_like<S: serde::Serializer>(b: Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bytes(&b)
}

/// Reads a byte string (or, from other formats, a sequence of bytes).
struct Bytes(Vec<u8>);
impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Bytes;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E>(self, v: &[u8]) -> Result<Bytes, E> {
                Ok(Bytes(v.to_vec()))
            }
            fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Bytes, E> {
                Ok(Bytes(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Bytes, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element::<u8>()? {
                    v.push(x);
                }
                Ok(Bytes(v))
            }
        }
        // byte_buf: large byte strings (ciborium only lends small ones).
        de.deserialize_byte_buf(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::battle::VehicleRec;

    #[test]
    fn roundtrip_and_chunks() {
        let mut b = Battle { duration: 10.0, ..Default::default() };
        let mut v = VehicleRec { id: 7, name: "x".into(), ..Default::default() };
        for i in 0..100 {
            let t = i as f32 * 0.1;
            v.track.push(Sample { t, pos: [i as f32 * 0.37 - 500.0, 12.5, -300.0], yaw: 3.1, pitch: -0.02, roll: 0.01 });
            v.gun_codes.push((t, (i * 64) as u16));
        }
        b.vehicles.insert(7, v);
        let full = decode_battle(&encode_frame(&b, 0.0)).unwrap();
        let tr = &full.vehicles[&7].track;
        assert_eq!(tr.len(), 100);
        assert!((tr[99].pos[0] - b.vehicles[&7].track[99].pos[0]).abs() < 0.01);
        assert!((tr[50].t - 5.0).abs() < 0.001);
        // live: first half, then an overlapping chunk from 4.0 s
        let mut half = b.clone();
        half.vehicles.get_mut(&7).unwrap().track.truncate(60);
        let mut file = encode_frame(&half, 0.0);
        file.extend(encode_frame(&b, 4.0));
        let merged = decode_battle(&file).unwrap();
        assert_eq!(merged.vehicles[&7].track.len(), 100);
        assert_eq!(merged.vehicles[&7].turret_yaw.len(), 100);
    }
}

pub fn put_varint(out: &mut Vec<u8>, v: i64) {
    put(out, v)
}

pub fn get_varint(d: &[u8], at: &mut usize) -> Option<i64> {
    get(d, at)
}

/// `#[serde(with = "wire::bytes")]`: a `Vec<u8>` stored as one byte string (not a list of numbers).
pub mod bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(b)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<u8>, D::Error> {
        Ok(Bytes::deserialize(de)?.0)
    }
}
