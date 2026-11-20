//! Minimal Python 2 pickle reader (protocols 0-2), enough for the plain data
//! (tuples, lists, dicts, numbers, strings) the server sends in `updateArena`.
//! Class instances / globals decode to `Py::Opaque`.

#[derive(Debug, Clone, PartialEq)]
pub enum Py {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// A Python 2 byte string that isn't valid UTF-8 (e.g. compressed data).
    Bytes(Vec<u8>),
    List(Vec<Py>),
    Dict(Vec<(Py, Py)>),
    Opaque,
}

impl Py {
    pub fn int(&self) -> i64 {
        match self {
            Py::Int(i) => *i,
            Py::Bool(b) => *b as i64,
            Py::Float(f) => *f as i64,
            _ => 0,
        }
    }
    pub fn float(&self) -> f64 {
        match self {
            Py::Float(f) => *f,
            Py::Int(i) => *i as f64,
            _ => 0.0,
        }
    }
    pub fn str(&self) -> &str {
        match self {
            Py::Str(s) => s,
            _ => "",
        }
    }
    pub fn items(&self) -> &[Py] {
        match self {
            Py::List(l) => l,
            _ => &[],
        }
    }
    pub fn at(&self, i: usize) -> &Py {
        self.items().get(i).unwrap_or(&Py::None)
    }
    pub fn get(&self, key: &str) -> Option<&Py> {
        match self {
            Py::Dict(d) => d.iter().find(|(k, _)| k.str() == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Byte string: text when it is UTF-8, raw bytes otherwise.
fn bytes(b: &[u8]) -> Py {
    match std::str::from_utf8(b) {
        Ok(s) => Py::Str(s.to_string()),
        Err(_) => Py::Bytes(b.to_vec()),
    }
}

pub fn loads(b: &[u8]) -> Option<Py> {
    let mut stack: Vec<Py> = Vec::new();
    let mut marks: Vec<usize> = Vec::new();
    let mut memo: std::collections::HashMap<u32, Py> = std::collections::HashMap::new();
    let mut p = 0usize;
    let take = |p: &mut usize, n: usize| -> Option<&[u8]> {
        let s = b.get(*p..*p + n)?;
        *p += n;
        Some(s)
    };
    let line = |p: &mut usize| -> Option<String> {
        let end = b[*p..].iter().position(|&c| c == b'\n')?;
        let s = String::from_utf8_lossy(&b[*p..*p + end]).into_owned();
        *p += end + 1;
        Some(s)
    };
    let u32le = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
    let since_mark = |stack: &mut Vec<Py>, marks: &mut Vec<usize>| -> Option<Vec<Py>> {
        let m = marks.pop()?;
        Some(stack.split_off(m))
    };

    loop {
        let op = *b.get(p)?;
        p += 1;
        match op {
            0x80 => p += 1,                                        // PROTO
            b'.' => return stack.pop(),                            // STOP
            b'(' => marks.push(stack.len()),                       // MARK
            b'N' => stack.push(Py::None),
            0x88 => stack.push(Py::Bool(true)),
            0x89 => stack.push(Py::Bool(false)),
            b'J' => stack.push(Py::Int(i32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as i64)),
            b'K' => stack.push(Py::Int(take(&mut p, 1)?[0] as i64)),
            b'M' => stack.push(Py::Int(u16::from_le_bytes(take(&mut p, 2)?.try_into().ok()?) as i64)),
            0x8a | 0x8b => {
                // LONG1 / LONG4: little-endian two's complement
                let n = if op == 0x8a { take(&mut p, 1)?[0] as usize } else { u32le(take(&mut p, 4)?) as usize };
                let bytes = take(&mut p, n)?;
                let mut v: i128 = 0;
                for (i, x) in bytes.iter().enumerate().take(16) {
                    v |= (*x as i128) << (8 * i);
                }
                if n > 0 && n < 16 && bytes[n - 1] & 0x80 != 0 {
                    v -= 1i128 << (8 * n);
                }
                stack.push(Py::Int(v as i64));
            }
            b'I' => {
                let s = line(&mut p)?;
                stack.push(match s.as_str() {
                    "00" => Py::Bool(false),
                    "01" => Py::Bool(true),
                    _ => Py::Int(s.trim().parse().ok()?),
                });
            }
            b'L' => stack.push(Py::Int(line(&mut p)?.trim_end_matches('L').parse().unwrap_or(0))),
            b'F' => stack.push(Py::Float(line(&mut p)?.trim().parse().ok()?)),
            b'G' => stack.push(Py::Float(f64::from_be_bytes(take(&mut p, 8)?.try_into().ok()?))),
            b'U' => {
                let n = take(&mut p, 1)?[0] as usize;
                stack.push(bytes(take(&mut p, n)?));
            }
            b'T' => {
                let n = u32le(take(&mut p, 4)?) as usize;
                stack.push(bytes(take(&mut p, n)?));
            }
            b'X' => {
                let n = u32le(take(&mut p, 4)?) as usize;
                stack.push(Py::Str(String::from_utf8_lossy(take(&mut p, n)?).into_owned()));
            }
            b'S' | b'V' => {
                let s = line(&mut p)?;
                stack.push(Py::Str(s.trim_matches(|c| c == '\'' || c == '"').to_string()));
            }
            b')' => stack.push(Py::List(vec![])),
            b']' => stack.push(Py::List(vec![])),
            b'}' => stack.push(Py::Dict(vec![])),
            b't' | b'l' => {
                let items = since_mark(&mut stack, &mut marks)?;
                stack.push(Py::List(items));
            }
            0x85..=0x87 => {
                let n = (op - 0x84) as usize;
                let items = stack.split_off(stack.len().checked_sub(n)?);
                stack.push(Py::List(items));
            }
            b'a' => {
                let v = stack.pop()?;
                if let Some(Py::List(l)) = stack.last_mut() {
                    l.push(v);
                }
            }
            b'e' => {
                let items = since_mark(&mut stack, &mut marks)?;
                if let Some(Py::List(l)) = stack.last_mut() {
                    l.extend(items);
                }
            }
            b'd' => {
                let items = since_mark(&mut stack, &mut marks)?;
                stack.push(Py::Dict(items.chunks_exact(2).map(|c| (c[0].clone(), c[1].clone())).collect()));
            }
            b's' => {
                let v = stack.pop()?;
                let k = stack.pop()?;
                if let Some(Py::Dict(d)) = stack.last_mut() {
                    d.push((k, v));
                }
            }
            b'u' => {
                let items = since_mark(&mut stack, &mut marks)?;
                if let Some(Py::Dict(d)) = stack.last_mut() {
                    d.extend(items.chunks_exact(2).map(|c| (c[0].clone(), c[1].clone())));
                }
            }
            b'q' => {
                let k = take(&mut p, 1)?[0] as u32;
                memo.insert(k, stack.last()?.clone());
            }
            b'r' => {
                let k = u32le(take(&mut p, 4)?);
                memo.insert(k, stack.last()?.clone());
            }
            b'p' => {
                let k: u32 = line(&mut p)?.trim().parse().ok()?;
                memo.insert(k, stack.last()?.clone());
            }
            b'h' => {
                let k = take(&mut p, 1)?[0] as u32;
                stack.push(memo.get(&k)?.clone());
            }
            b'j' => {
                let k = u32le(take(&mut p, 4)?);
                stack.push(memo.get(&k)?.clone());
            }
            b'g' => {
                let k: u32 = line(&mut p)?.trim().parse().ok()?;
                stack.push(memo.get(&k)?.clone());
            }
            b'c' => {
                line(&mut p)?;
                line(&mut p)?;
                stack.push(Py::Opaque);
            }
            b'R' | 0x81 => {
                stack.pop()?;
                stack.pop()?;
                stack.push(Py::Opaque);
            }
            b'b' => {
                stack.pop()?;
            }
            b'0' => {
                stack.pop()?;
            }
            b'2' => stack.push(stack.last()?.clone()),
            b'1' => {
                since_mark(&mut stack, &mut marks)?;
            }
            _ => return None,
        }
    }
}
