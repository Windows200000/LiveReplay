//! Minimal reader for the game's `.pkg` archives (plain ZIP files, almost always "stored").
//! Also used for the small zips nested inside map packages (`.cdata_processed`).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

struct Entry {
    header_offset: u64,
    method: u16,
    compressed_size: u64,
}

enum Source {
    File(PathBuf),
    Memory(Vec<u8>),
}

pub struct Pkg {
    source: Source,
    entries: HashMap<String, Entry>,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Parse the central directory given the archive's tail and a way to fetch bytes.
fn parse_directory(tail: &[u8], tail_start: u64, fetch: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Option<HashMap<String, Entry>> {
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| u32_at(tail, i) == 0x0605_4b50)?;
    let cd_size = u32_at(tail, eocd + 12) as usize;
    let cd_offset = u32_at(tail, eocd + 16) as u64;
    let cd = if cd_offset >= tail_start {
        let s = (cd_offset - tail_start) as usize;
        tail.get(s..s + cd_size)?.to_vec()
    } else {
        fetch(cd_offset, cd_size)?
    };
    let mut entries = HashMap::new();
    let mut p = 0usize;
    while p + 46 <= cd.len() && u32_at(&cd, p) == 0x0201_4b50 {
        let method = u16_at(&cd, p + 10);
        let compressed_size = u32_at(&cd, p + 20) as u64;
        let name_len = u16_at(&cd, p + 28) as usize;
        let extra_len = u16_at(&cd, p + 30) as usize;
        let comment_len = u16_at(&cd, p + 32) as usize;
        let header_offset = u32_at(&cd, p + 42) as u64;
        let name = String::from_utf8_lossy(&cd[p + 46..p + 46 + name_len]).to_lowercase();
        entries.insert(name, Entry { header_offset, method, compressed_size });
        p += 46 + name_len + extra_len + comment_len;
    }
    Some(entries)
}

impl Pkg {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut f = File::open(path)?;
        let len = f.metadata()?.len();
        let tail_len = len.min(66_000);
        f.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        f.read_exact(&mut tail)?;
        let mut fetch = |off: u64, n: usize| -> Option<Vec<u8>> {
            let mut buf = vec![0u8; n];
            f.seek(SeekFrom::Start(off)).ok()?;
            f.read_exact(&mut buf).ok()?;
            Some(buf)
        };
        let entries = parse_directory(&tail, len - tail_len, &mut fetch)
            .ok_or_else(|| anyhow::anyhow!("no zip directory in {}", path.display()))?;
        Ok(Self { source: Source::File(path.to_path_buf()), entries })
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let entries = parse_directory(bytes, 0, &mut |o, n| bytes.get(o as usize..o as usize + n).map(|s| s.to_vec()))?;
        Some(Self { source: Source::Memory(bytes.to_vec()), entries })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(&name.to_lowercase())
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let e = self.entries.get(&name.to_lowercase())?;
        let raw = match &self.source {
            Source::File(path) => {
                let mut f = File::open(path).ok()?;
                f.seek(SeekFrom::Start(e.header_offset)).ok()?;
                let mut hdr = [0u8; 30];
                f.read_exact(&mut hdr).ok()?;
                let skip = u16_at(&hdr, 26) as i64 + u16_at(&hdr, 28) as i64;
                f.seek(SeekFrom::Current(skip)).ok()?;
                let mut raw = vec![0u8; e.compressed_size as usize];
                f.read_exact(&mut raw).ok()?;
                raw
            }
            Source::Memory(b) => {
                let h = e.header_offset as usize;
                let start = h + 30 + u16_at(b, h + 26) as usize + u16_at(b, h + 28) as usize;
                b.get(start..start + e.compressed_size as usize)?.to_vec()
            }
        };
        match e.method {
            0 => Some(raw),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(&raw[..]).read_to_end(&mut out).ok()?;
                Some(out)
            }
            _ => None,
        }
    }
}
