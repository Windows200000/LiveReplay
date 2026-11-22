//! `.wotreplay` container: JSON blocks followed by a Blowfish-encrypted, zlib-compressed packet stream.
//! While a battle is running the client writes `temp.wotreplay` as a plain, unencrypted packet
//! stream (no JSON header); that "live" form is supported too.

use blowfish::cipher::{Array, BlockCipherDecrypt, KeyInit};
use std::io::Read;

const MAGIC: [u8; 4] = [0x12, 0x32, 0x34, 0x11];
const KEY: [u8; 16] = [
    0xDE, 0x72, 0xBE, 0xA0, 0xDE, 0x04, 0xBE, 0xB1, 0xDE, 0xFE, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE, 0xEF,
];

pub struct ReplayFile {
    /// Block 0: battle start info. Block 1 (if the battle was watched to the end): battle results.
    pub blocks: Vec<serde_json::Value>,
    encrypted: Vec<u8>,
    /// Unencrypted packet stream of an in-progress battle.
    raw: Option<Vec<u8>>,
}

impl ReplayFile {
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> anyhow::Result<Self> {
        anyhow::ensure!(bytes.len() > 12, "replay file is empty");
        if bytes[0..4] != MAGIC {
            // In-progress replay: starts directly with a packet (normally 0x18 = game version).
            let kind = u32::from_le_bytes(bytes[4..8].try_into()?);
            anyhow::ensure!(kind < 0x100, "not a World of Tanks replay");
            return Ok(Self { blocks: Vec::new(), encrypted: Vec::new(), raw: Some(bytes) });
        }
        let rd = |o: usize| -> anyhow::Result<usize> {
            Ok(u32::from_le_bytes(bytes.get(o..o + 4).ok_or_else(|| anyhow::anyhow!("truncated replay"))?.try_into()?) as usize)
        };
        let count = rd(4)?;
        let mut p = 8;
        let mut blocks = Vec::new();
        for _ in 0..count {
            let len = rd(p)?;
            let data = bytes.get(p + 4..p + 4 + len).ok_or_else(|| anyhow::anyhow!("truncated replay block"))?;
            blocks.push(serde_json::from_slice(data).unwrap_or(serde_json::Value::Null));
            p += 4 + len;
        }
        // 8 bytes (two u32 sizes) precede the encrypted data.
        let encrypted = bytes.get(p + 8..).map(|s| s.to_vec()).unwrap_or_default();
        Ok(Self { blocks, encrypted, raw: None })
    }

    pub fn is_live(&self) -> bool {
        self.raw.is_some()
    }

    pub fn start_info(&self) -> &serde_json::Value {
        self.blocks.first().unwrap_or(&serde_json::Value::Null)
    }

    /// Battle results (present only for complete replays).
    pub fn results(&self) -> Option<&serde_json::Value> {
        self.blocks.get(1).filter(|v| !v.is_null())
    }

    /// Decrypt and decompress the packet stream.
    pub fn packet_stream(&self) -> anyhow::Result<Vec<u8>> {
        if let Some(raw) = &self.raw {
            return Ok(raw.clone());
        }
        let cipher: blowfish::Blowfish = blowfish::Blowfish::new_from_slice(&KEY).map_err(|_| anyhow::anyhow!("bad key"))?;
        let mut out = Vec::with_capacity(self.encrypted.len());
        let mut prev = [0u8; 8];
        for chunk in self.encrypted.chunks_exact(8) {
            let Ok(mut block) = Array::try_from(chunk) else { continue };
            cipher.decrypt_block(&mut block);
            for i in 0..8 {
                prev[i] ^= block[i];
            }
            out.extend_from_slice(&prev);
        }
        let mut data = Vec::new();
        flate2::read::ZlibDecoder::new(&out[..]).read_to_end(&mut data)?;
        Ok(data)
    }
}

/// One record of the decrypted stream.
pub struct Packet<'a> {
    pub kind: u32,
    pub time: f32,
    pub payload: &'a [u8],
}

pub fn packets(stream: &[u8]) -> impl Iterator<Item = Packet<'_>> {
    let mut p = 0usize;
    std::iter::from_fn(move || {
        let hdr = stream.get(p..p + 12)?;
        let size = u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize;
        let kind = u32::from_le_bytes(hdr[4..8].try_into().unwrap());
        let time = f32::from_le_bytes(hdr[8..12].try_into().unwrap());
        let payload = stream.get(p + 12..p + 12 + size)?;
        p += 12 + size;
        Some(Packet { kind, time, payload })
    })
}
