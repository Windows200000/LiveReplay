//! BigWorld entity definitions (`scripts/entity_defs/*.def`) read from the installed client,
//! plus a decoder for the binary values they describe. Method/property IDs in the replay
//! stream are indices into the "exposed" lists, which BigWorld sorts by fixed stream size.
//! (Ordering rules follow Monstrofil/replays_unpack; see README credits.)

use std::collections::HashMap;

use crate::gamedata::pxml::Node;
use crate::gamedata::GameData;

const INFINITY: usize = 0xFFFF;

#[derive(Debug, Clone)]
pub enum Ty {
    Int { size: u8, signed: bool },
    F32,
    F64,
    Vector(u8),
    Str,
    Blob,
    Mailbox,
    Dict { fields: Vec<(String, Ty)>, allow_none: bool },
    Array { of: Box<Ty>, size: Option<usize> },
    User(Box<Ty>),
}

impl Ty {
    fn size(&self) -> usize {
        match self {
            Ty::Int { size, .. } => *size as usize,
            Ty::F32 => 4,
            Ty::F64 => 8,
            Ty::Vector(n) => 4 * *n as usize,
            Ty::Str | Ty::Blob | Ty::Mailbox | Ty::User(_) => INFINITY,
            Ty::Dict { fields, allow_none } => {
                if *allow_none {
                    INFINITY
                } else {
                    fields.iter().map(|(_, t)| t.size()).sum()
                }
            }
            Ty::Array { of, size } => size.map(|n| of.size() * n).unwrap_or(INFINITY),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Val {
    Int(i64),
    Float(f64),
    Vector(Vec<f32>),
    Bytes(Vec<u8>),
    List(Vec<Val>),
    Dict(Vec<(String, Val)>),
    None,
}

impl Val {
    pub fn int(&self) -> i64 {
        match self {
            Val::Int(i) => *i,
            Val::Float(f) => *f as i64,
            _ => 0,
        }
    }
    pub fn float(&self) -> f64 {
        match self {
            Val::Int(i) => *i as f64,
            Val::Float(f) => *f,
            _ => 0.0,
        }
    }
    pub fn bytes(&self) -> &[u8] {
        match self {
            Val::Bytes(b) => b,
            _ => &[],
        }
    }
    pub fn list(&self) -> &[Val] {
        match self {
            Val::List(l) => l,
            _ => &[],
        }
    }
    pub fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

pub struct Cursor<'a> {
    pub b: &'a [u8],
    pub p: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn i16(&mut self) -> Option<i16> {
        Some(i16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.b[self.p.min(self.b.len())..]
    }
    /// BigWorld packed length: u8, or 0xFF followed by a 24-bit little-endian length.
    fn packed_len(&mut self) -> Option<usize> {
        let n = self.u8()? as usize;
        if n == 0xFF {
            let b = self.take(3)?;
            Some(b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16)
        } else {
            Some(n)
        }
    }

    pub fn read(&mut self, ty: &Ty) -> Option<Val> {
        Some(match ty {
            Ty::Int { size, signed } => {
                let b = self.take(*size as usize)?;
                let mut v: u64 = 0;
                for (i, x) in b.iter().enumerate() {
                    v |= (*x as u64) << (8 * i);
                }
                if *signed && *size < 8 {
                    let shift = 64 - 8 * *size as u32;
                    Val::Int(((v << shift) as i64) >> shift)
                } else {
                    Val::Int(v as i64)
                }
            }
            Ty::F32 => Val::Float(self.f32()? as f64),
            Ty::F64 => Val::Float(f64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            Ty::Vector(n) => Val::Vector((0..*n).map(|_| self.f32()).collect::<Option<_>>()?),
            Ty::Str | Ty::Blob => {
                let n = self.packed_len()?;
                Val::Bytes(self.take(n)?.to_vec())
            }
            Ty::Mailbox => {
                self.take(6)?;
                Val::None
            }
            Ty::Dict { fields, allow_none } => {
                if *allow_none {
                    match self.b.get(self.p) {
                        Some(0) => {
                            self.p += 1;
                            return Some(Val::None);
                        }
                        Some(1) => self.p += 1,
                        _ => {}
                    }
                }
                let mut out = Vec::with_capacity(fields.len());
                for (k, t) in fields {
                    out.push((k.clone(), self.read(t)?));
                }
                Val::Dict(out)
            }
            Ty::Array { of, size } => {
                let n = match size {
                    Some(n) => *n,
                    None => self.packed_len()?,
                };
                Val::List((0..n).map(|_| self.read(of)).collect::<Option<_>>()?)
            }
            Ty::User(inner) => {
                if !matches!(**inner, Ty::Blob) {
                    self.u8()?;
                }
                self.read(inner)?
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct Method {
    pub name: String,
    pub args: Vec<Ty>,
    header: usize,
}

impl Method {
    fn stream_size(&self) -> usize {
        let s: usize = self.args.iter().map(Ty::size).sum();
        if s >= INFINITY {
            INFINITY + self.header
        } else {
            s + self.header
        }
    }
    pub fn decode(&self, payload: &[u8]) -> Option<Vec<Val>> {
        let mut c = Cursor::new(payload);
        self.args.iter().map(|a| c.read(a)).collect()
    }
}

#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub ty: Ty,
    flags: String,
}

#[derive(Debug, Default)]
pub struct EntityDef {
    pub name: String,
    methods: Vec<Method>,
    props: Vec<Property>,
    /// Exposed (wire-order) lists.
    pub client_methods: Vec<Method>,
    pub client_props: Vec<Property>,
}

impl EntityDef {
    pub fn method_index(&self, name: &str) -> Option<usize> {
        self.client_methods.iter().position(|m| m.name == name)
    }
    pub fn prop_index(&self, name: &str) -> Option<usize> {
        self.client_props.iter().position(|p| p.name == name)
    }
}

pub struct Definitions {
    /// Index 0 = entity type 1 on the wire.
    pub entities: Vec<EntityDef>,
}

struct Aliases {
    raw: HashMap<String, Node>,
}

impl Aliases {
    fn resolve(&self, node: &Node, depth: usize) -> Ty {
        let name = node.text().split_whitespace().next().unwrap_or("").to_string();
        if depth > 32 {
            return Ty::Blob;
        }
        if let Some(alias) = self.raw.get(&name) {
            return self.resolve(alias, depth + 1);
        }
        let int = |size, signed| Ty::Int { size, signed };
        match name.as_str() {
            "INT8" => int(1, true),
            "INT16" => int(2, true),
            "INT32" => int(4, true),
            "INT64" => int(8, true),
            "UINT8" => int(1, false),
            "UINT16" => int(2, false),
            "UINT32" => int(4, false),
            "UINT64" => int(8, false),
            "FLOAT" | "FLOAT32" => Ty::F32,
            "FLOAT64" => Ty::F64,
            "VECTOR2" => Ty::Vector(2),
            "VECTOR3" => Ty::Vector(3),
            "VECTOR4" => Ty::Vector(4),
            "STRING" | "UNICODE_STRING" => Ty::Str,
            "BLOB" | "PYTHON" => Ty::Blob,
            "MAILBOX" => Ty::Mailbox,
            "FIXED_DICT" => {
                let fields = node
                    .child("Properties")
                    .map(|p| {
                        p.children
                            .iter()
                            .map(|f| (f.name.clone(), f.child("Type").map(|t| self.resolve(t, depth + 1)).unwrap_or(Ty::Blob)))
                            .collect()
                    })
                    .unwrap_or_default();
                Ty::Dict { fields, allow_none: node.child_text("AllowNone") == Some("true") }
            }
            "ARRAY" | "TUPLE" => Ty::Array {
                of: Box::new(node.child("of").map(|t| self.resolve(t, depth + 1)).unwrap_or(Ty::Blob)),
                size: node.child_text("size").and_then(|s| s.parse().ok()),
            },
            "USER_TYPE" => Ty::User(Box::new(node.child("Type").map(|t| self.resolve(t, depth + 1)).unwrap_or(Ty::Blob))),
            _ => Ty::Blob,
        }
    }
}

const CLIENT_FLAGS: [&str; 5] = ["ALL_CLIENTS", "BASE_AND_CLIENT", "OTHER_CLIENTS", "OWN_CLIENT", "CELL_PUBLIC_AND_OWN"];

impl Definitions {
    pub fn load(gd: &GameData) -> anyhow::Result<Self> {
        let mut raw = HashMap::new();
        for file in ["scripts/entity_defs/alias.xml", "scripts/entity_defs/alias_ext.xml"] {
            if let Some(n) = gd.script_xml(file) {
                for c in n.children {
                    raw.insert(c.name.clone(), c);
                }
            }
        }
        let aliases = Aliases { raw };
        let list = gd.script_xml("scripts/entities.xml").ok_or_else(|| anyhow::anyhow!("entities.xml not found in game files"))?;
        let list = list.child("ClientServerEntities").unwrap_or(&list);
        let mut entities = Vec::new();
        for e in &list.children {
            let mut def = EntityDef { name: e.name.clone(), ..Default::default() };
            if let Some(n) = gd.script_xml(&format!("scripts/entity_defs/{}.def", e.name)) {
                parse_section(gd, &aliases, &n, &mut def, 0);
            }
            def.client_methods = def.methods.clone();
            def.client_methods.sort_by_key(Method::stream_size);
            def.client_props = def.props.iter().filter(|p| CLIENT_FLAGS.contains(&p.flags.as_str())).cloned().collect();
            def.client_props.sort_by_key(|p| p.ty.size().min(INFINITY));
            entities.push(def);
        }
        Ok(Self { entities })
    }

    pub fn by_type(&self, wire_type: u16) -> Option<&EntityDef> {
        self.entities.get((wire_type as usize).checked_sub(1)?)
    }
    pub fn by_name(&self, name: &str) -> Option<&EntityDef> {
        self.entities.iter().find(|e| e.name == name)
    }
}

fn parse_section(gd: &GameData, aliases: &Aliases, n: &Node, def: &mut EntityDef, depth: usize) {
    if depth > 16 {
        return;
    }
    if let Some(imp) = n.child("Implements") {
        for i in &imp.children {
            if let Some(iface) = gd.script_xml(&format!("scripts/entity_defs/interfaces/{}.def", i.text())) {
                parse_section(gd, aliases, &iface, def, depth + 1);
            }
        }
    }
    if let Some(props) = n.child("Properties") {
        for p in &props.children {
            def.props.retain(|x| x.name != p.name);
            def.props.push(Property {
                name: p.name.clone(),
                ty: p.child("Type").map(|t| aliases.resolve(t, 0)).unwrap_or(Ty::Blob),
                flags: p.child_text("Flags").unwrap_or("").to_string(),
            });
        }
    }
    if let Some(methods) = n.child("ClientMethods") {
        for m in &methods.children {
            if def.methods.iter().any(|x| x.name == m.name) {
                continue;
            }
            let args = if let Some(args) = m.child("Args") {
                args.children.iter().map(|a| aliases.resolve(a, 0)).collect()
            } else {
                m.children_named("Arg").map(|a| aliases.resolve(a, 0)).collect()
            };
            let header = m.child("VariableLengthHeaderSize").and_then(|h| h.text().parse().ok()).unwrap_or(1);
            def.methods.push(Method { name: m.name.clone(), args, header });
        }
    }
}
