//! Tiny DDS decoder (top mip level only): DXT1, DXT3, DXT5 and uncompressed 32-bit BGRA.
//! Enough for the minimap textures (`spaces/<map>/mmap.dds`).

#[derive(Debug, Clone, Default)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
}

fn color_block(b: &[u8], opaque_only: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (a, bb) = (rgb565(c0), rgb565(c1));
    let mix = |w0: u32, w1: u32, d: u32| -> [u8; 4] {
        [
            ((a[0] as u32 * w0 + bb[0] as u32 * w1) / d) as u8,
            ((a[1] as u32 * w0 + bb[1] as u32 * w1) / d) as u8,
            ((a[2] as u32 * w0 + bb[2] as u32 * w1) / d) as u8,
            255,
        ]
    };
    let pal = if c0 > c1 || opaque_only {
        [[a[0], a[1], a[2], 255], [bb[0], bb[1], bb[2], 255], mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [[a[0], a[1], a[2], 255], [bb[0], bb[1], bb[2], 255], mix(1, 1, 2), [0, 0, 0, 0]]
    };
    let bits = u32_at(b, 4);
    let mut out = [[0u8; 4]; 16];
    for (i, px) in out.iter_mut().enumerate() {
        *px = pal[((bits >> (2 * i)) & 3) as usize];
    }
    out
}

fn alpha_dxt5(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = ((7 - i as u32) * a0 + i as u32 * a1) / 7;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = ((5 - i as u32) * a0 + i as u32 * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for i in 0..6 {
        bits |= (b[2 + i] as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, a) in out.iter_mut().enumerate() {
        *a = pal[((bits >> (3 * i)) & 7) as usize] as u8;
    }
    out
}

pub fn decode(bytes: &[u8]) -> Option<Image> {
    decode_level(bytes, 0)
}

/// Raw block data of each mip level of a DXT1 DDS: (width, height, blocks).
pub fn dxt1_levels(bytes: &[u8]) -> Option<Vec<(usize, usize, &[u8])>> {
    if bytes.len() < 128 || &bytes[0..4] != b"DDS " || &bytes[84..88] != b"DXT1" {
        return None;
    }
    let mips = (u32_at(bytes, 28) as usize).max(1);
    let (h0, w0) = (u32_at(bytes, 12) as usize, u32_at(bytes, 16) as usize);
    let mut off = 128;
    let mut out = Vec::new();
    for l in 0..mips {
        let (w, h) = ((w0 >> l).max(1), (h0 >> l).max(1));
        let size = w.div_ceil(4) * h.div_ceil(4) * 8;
        out.push((w, h, bytes.get(off..off + size)?));
        off += size;
    }
    Some(out)
}

/// Decode one mip level (0 = full size), e.g. level 2 of a 4096² texture = 1024².
pub fn decode_level(bytes: &[u8], level: usize) -> Option<Image> {
    if bytes.len() < 128 || &bytes[0..4] != b"DDS " {
        return None;
    }
    let mips = (u32_at(bytes, 28) as usize).max(1);
    let level = level.min(mips - 1);
    let (h0, w0) = (u32_at(bytes, 12) as usize, u32_at(bytes, 16) as usize);
    let pf_flags = u32_at(bytes, 80);
    let fourcc = &bytes[84..88];
    let block = match fourcc {
        b"DXT1" => 8,
        b"DXT3" | b"DXT5" => 16,
        _ if pf_flags & 0x4 == 0 => 0,
        _ => return None,
    };
    let mut offset = 128;
    for l in 0..level {
        let (w, h) = ((w0 >> l).max(1), (h0 >> l).max(1));
        offset += if block > 0 { w.div_ceil(4) * h.div_ceil(4) * block } else { w * h * 4 };
    }
    let width = (w0 >> level).max(1);
    let height = (h0 >> level).max(1);
    let data = bytes.get(offset..)?;
    let mut rgba = vec![0u8; width * height * 4];

    if pf_flags & 0x4 == 0 {
        // Uncompressed, assume 32-bit BGRA.
        if u32_at(bytes, 88) != 32 || data.len() < width * height * 4 {
            return None;
        }
        for (i, px) in data.chunks_exact(4).take(width * height).enumerate() {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        return Some(Image { width, height, rgba });
    }

    let block_size = match fourcc {
        b"DXT1" => 8,
        b"DXT3" | b"DXT5" => 16,
        _ => return None,
    };
    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
    if data.len() < bw * bh * block_size {
        return None;
    }
    for by in 0..bh {
        for bx in 0..bw {
            let blk = &data[(by * bw + bx) * block_size..][..block_size];
            let (colors, alpha): ([[u8; 4]; 16], Option<[u8; 16]>) = match fourcc {
                b"DXT1" => (color_block(blk, false), None),
                b"DXT3" => {
                    let mut a = [0u8; 16];
                    for (i, v) in a.iter_mut().enumerate() {
                        *v = ((blk[i / 2] >> (4 * (i % 2))) & 15) * 17;
                    }
                    (color_block(&blk[8..], true), Some(a))
                }
                _ => (color_block(&blk[8..], true), Some(alpha_dxt5(blk))),
            };
            for i in 0..16 {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < width && y < height {
                    let mut c = colors[i];
                    if let Some(a) = alpha {
                        c[3] = a[i];
                    }
                    rgba[(y * width + x) * 4..][..4].copy_from_slice(&c);
                }
            }
        }
    }
    Some(Image { width, height, rgba })
}
