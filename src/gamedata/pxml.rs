//! Decoder for BigWorld "packed XML" (magic `0x62A14E45`), used for most files in `scripts.pkg`.

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub name: String,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    pub fn text(&self) -> &str {
        self.text.trim()
    }
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text())
    }
    pub fn floats(&self) -> Vec<f32> {
        self.text().split_whitespace().filter_map(|s| s.parse().ok()).collect()
    }
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
    names: Vec<String>,
}

impl<'a> Reader<'a> {
    fn u16(&mut self) -> Option<u16> {
        let v = u16::from_le_bytes(self.b.get(self.p..self.p + 2)?.try_into().ok()?);
        self.p += 2;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_le_bytes(self.b.get(self.p..self.p + 4)?.try_into().ok()?);
        self.p += 4;
        Some(v)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }

    fn element(&mut self, node: &mut Node) -> Option<()> {
        let count = self.u16()? as usize;
        let own = self.u32()?;
        let mut kids = Vec::with_capacity(count);
        for _ in 0..count {
            let name_idx = self.u16()? as usize;
            kids.push((name_idx, self.u32()?));
        }
        let mut offset = self.data(node, 0, own)?;
        for (name_idx, desc) in kids {
            let mut child = Node { name: self.names.get(name_idx)?.clone(), ..Default::default() };
            offset = self.data(&mut child, offset, desc)?;
            node.children.push(child);
        }
        Some(())
    }

    fn data(&mut self, node: &mut Node, offset: u32, desc: u32) -> Option<u32> {
        let end = desc & 0x0fff_ffff;
        let kind = desc >> 28;
        let len = end.checked_sub(offset)? as usize;
        match kind {
            0 => self.element(node)?,
            1 => node.text = String::from_utf8_lossy(self.take(len)?).into_owned(),
            2 => {
                let raw = self.take(len)?;
                node.text = match len {
                    0 => 0,
                    1 => raw[0] as i8 as i64,
                    2 => i16::from_le_bytes(raw.try_into().ok()?) as i64,
                    4 => i32::from_le_bytes(raw.try_into().ok()?) as i64,
                    8 => i64::from_le_bytes(raw.try_into().ok()?),
                    _ => 0,
                }
                .to_string();
            }
            3 => {
                let raw = self.take(len)?;
                node.text = raw
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes(c.try_into().unwrap()).to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            4 => node.text = if self.take(len)? == [1] { "true" } else { "false" }.into(),
            _ => node.text = base64(self.take(len)?),
        }
        Some(end)
    }
}

/// Type 5 values are stored base64-decoded; re-encode them to get the original text back.
fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

pub fn parse(bytes: &[u8]) -> Option<Node> {
    if bytes.get(0..4)? != [0x45, 0x4e, 0xa1, 0x62] {
        return None;
    }
    let mut r = Reader { b: bytes, p: 5, names: Vec::new() };
    loop {
        let end = r.b[r.p..].iter().position(|&c| c == 0)?;
        let name = String::from_utf8_lossy(&r.b[r.p..r.p + end]).into_owned();
        r.p += end + 1;
        if name.is_empty() {
            break;
        }
        r.names.push(name);
    }
    let mut root = Node { name: "root".into(), ..Default::default() };
    r.element(&mut root)?;
    Some(root)
}
