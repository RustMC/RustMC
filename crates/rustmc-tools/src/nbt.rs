//! Minimal read-only NBT parser for Anvil region data (big-endian, named
//! root tag). Only the tag kinds that appear in 26.3 chunk storage are
//! supported; the parser never allocates beyond the input size.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Tag>),
    Compound(BTreeMap<String, Tag>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    pub fn get<'a>(&'a self, key: &str) -> Option<&'a Tag> {
        match self {
            Self::Compound(map) => map.get(key),
            _ => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Byte(v) => Some(i32::from(*v)),
            Self::Short(v) => Some(i32::from(*v)),
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Self::List(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_long_array(&self) -> Option<&[i64]> {
        match self {
            Self::LongArray(v) => Some(v),
            _ => None,
        }
    }
}

pub fn parse_root(bytes: &[u8]) -> Result<Tag, String> {
    let mut cur = Reader::new(bytes);
    let kind = cur.u8()?;
    if kind != 10 {
        return Err(format!("root tag must be compound, found type {kind}"));
    }
    cur.skip_string()?;
    Ok(Tag::Compound(cur.compound()?))
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or("length overflow")?;
        if end > self.bytes.len() {
            return Err("unexpected end of NBT input".to_string());
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> Result<i16, String> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> Result<i32, String> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Result<i64, String> {
        let b = self.take(8)?;
        let mut v = [0u8; 8];
        v.copy_from_slice(b);
        Ok(i64::from_be_bytes(v))
    }
    fn string(&mut self) -> Result<String, String> {
        let len = usize::from(self.u16()?);
        let b = self.take(len)?;
        String::from_utf8(b.to_vec()).map_err(|e| format!("invalid utf8 in NBT: {e}"))
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn skip_string(&mut self) -> Result<(), String> {
        let len = usize::from(self.u16()?);
        self.take(len)?;
        Ok(())
    }
    fn payload(&mut self, kind: u8) -> Result<Tag, String> {
        Ok(match kind {
            1 => Tag::Byte(self.i8()?),
            2 => Tag::Short(self.i16()?),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(f32::from_bits(self.i32()? as u32)),
            6 => Tag::Double(f64::from_bits(self.i64()? as u64)),
            7 => {
                let len = self.checked_len("byte array")?;
                let b = self.take(len)?;
                Tag::ByteArray(b.iter().map(|v| *v as i8).collect())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let inner = self.u8()?;
                let len = self.checked_len("list")?;
                if inner == 0 && len > 0 {
                    return Err("empty list element type with entries".to_string());
                }
                let mut items = Vec::with_capacity(len.min(1 << 16));
                for _ in 0..len {
                    items.push(self.anonymous(inner)?);
                }
                Tag::List(items)
            }
            10 => Tag::Compound(self.compound()?),
            11 => {
                let len = self.checked_len("int array")?;
                Tag::IntArray((0..len).map(|_| self.i32()).collect::<Result<_, _>>()?)
            }
            12 => {
                let len = self.checked_len("long array")?;
                Tag::LongArray((0..len).map(|_| self.i64()).collect::<Result<_, _>>()?)
            }
            other => return Err(format!("unknown NBT tag type {other}")),
        })
    }
    fn checked_len(&mut self, what: &str) -> Result<usize, String> {
        let len = self.i32()?;
        usize::try_from(len).map_err(|_| format!("negative {what} length"))
    }
    fn i8(&mut self) -> Result<i8, String> {
        Ok(self.u8()? as i8)
    }
    fn anonymous(&mut self, kind: u8) -> Result<Tag, String> {
        self.payload(kind)
    }
    fn compound(&mut self) -> Result<BTreeMap<String, Tag>, String> {
        let mut map = BTreeMap::new();
        loop {
            let kind = self.u8()?;
            if kind == 0 {
                return Ok(map);
            }
            let name = self.string()?;
            map.insert(name, self.payload(kind)?);
        }
    }
}

#[cfg(test)]
pub fn write_root(compound: &Tag, out: &mut Vec<u8>) {
    out.push(10);
    out.extend_from_slice(&0u16.to_be_bytes());
    write_payload(compound, out);
}

#[cfg(test)]
pub fn write_payload(tag: &Tag, out: &mut Vec<u8>) {
    match tag {
        Tag::Byte(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::ByteArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.extend(v.iter().map(|b| *b as u8));
        }
        Tag::String(v) => {
            out.extend_from_slice(&(v.len() as u16).to_be_bytes());
            out.extend_from_slice(v.as_bytes());
        }
        Tag::List(items) => {
            let kind = match items.first() {
                Some(Tag::Byte(_)) => 1,
                Some(Tag::Short(_)) => 2,
                Some(Tag::Int(_)) => 3,
                Some(Tag::Long(_)) => 4,
                Some(Tag::Float(_)) => 5,
                Some(Tag::Double(_)) => 6,
                Some(Tag::ByteArray(_)) => 7,
                Some(Tag::String(_)) => 8,
                Some(Tag::Compound(_)) => 10,
                Some(Tag::IntArray(_)) => 11,
                Some(Tag::LongArray(_)) => 12,
                Some(Tag::List(_)) => 9,
                None => {
                    out.extend_from_slice(&[0, 0, 0, 0]);
                    return;
                }
            };
            out.push(kind);
            out.extend_from_slice(&(items.len() as i32).to_be_bytes());
            for item in items {
                write_payload(item, out);
            }
        }
        Tag::Compound(map) => {
            for (name, value) in map {
                out.push(tag_kind(value));
                out.extend_from_slice(&(name.len() as u16).to_be_bytes());
                out.extend_from_slice(name.as_bytes());
                write_payload(value, out);
            }
            out.push(0);
        }
        Tag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        Tag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

#[cfg(test)]
fn tag_kind(tag: &Tag) -> u8 {
    match tag {
        Tag::Byte(_) => 1,
        Tag::Short(_) => 2,
        Tag::Int(_) => 3,
        Tag::Long(_) => 4,
        Tag::Float(_) => 5,
        Tag::Double(_) => 6,
        Tag::ByteArray(_) => 7,
        Tag::String(_) => 8,
        Tag::List(_) => 9,
        Tag::Compound(_) => 10,
        Tag::IntArray(_) => 11,
        Tag::LongArray(_) => 12,
    }
}

#[cfg(test)]
pub fn compound(pairs: &[(&str, Tag)]) -> Tag {
    let mut map = BTreeMap::new();
    for (k, v) in pairs {
        map.insert((*k).to_string(), v.clone());
    }
    Tag::Compound(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_supported_tag() {
        let root = compound(&[
            ("b", Tag::Byte(-1)),
            ("s", Tag::Short(257)),
            ("i", Tag::Int(-123456)),
            ("l", Tag::Long(i64::MAX)),
            ("f", Tag::Float(0.5)),
            ("d", Tag::Double(-2.25)),
            ("ba", Tag::ByteArray(vec![1, -2, 3])),
            ("st", Tag::String("minecraft:plains".into())),
            ("li", Tag::List(vec![Tag::Int(7), Tag::Int(8)])),
            ("ia", Tag::IntArray(vec![-1, 0, 1])),
            ("la", Tag::LongArray(vec![i64::MIN, 9])),
            ("sub", compound(&[("inner", Tag::Byte(4))])),
        ]);
        let mut bytes = Vec::new();
        write_root(&root, &mut bytes);
        assert_eq!(parse_root(&bytes).unwrap(), root);
    }

    #[test]
    fn rejects_truncated_and_bad_root_input() {
        assert!(parse_root(&[]).is_err());
        assert!(parse_root(&[2, 0, 0]).is_err());
        let mut bytes = Vec::new();
        write_root(&compound(&[("x", Tag::Int(1))]), &mut bytes);
        bytes.truncate(bytes.len() - 2);
        assert!(parse_root(&bytes).is_err());
    }
}
