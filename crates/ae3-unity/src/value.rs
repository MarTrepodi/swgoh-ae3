//! A generic, ordered value tree produced by TypeTree-driven reading.

use serde::ser::{SerializeMap, SerializeSeq};

/// A reference from one Asset to another: `file_id` 0 is the same
/// SerializedFile, `n > 0` is external number `n - 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct PPtr {
    /// 0 for the same file, otherwise 1 + the index into the externals.
    pub file_id: i32,
    /// The target object's path id; 0 is a null reference.
    pub path_id: i64,
}

impl PPtr {
    /// Whether this is a null reference.
    #[must_use]
    pub fn is_null(&self) -> bool {
        self.path_id == 0
    }

    /// Whether the target lives in the same SerializedFile.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.file_id == 0
    }
}

/// One deserialized Asset field.
///
/// Mapping from TypeTree types:
/// - signed integers -> [`Value::Int`], unsigned -> [`Value::UInt`], `bool` -> [`Value::Bool`]
/// - `float` -> [`Value::Float`], `double` -> [`Value::Double`]
/// - `string` -> [`Value::String`] (or [`Value::Bytes`] when not valid UTF-8, as
///   with binary `TextAsset` scripts)
/// - `TypelessData` and arrays of `UInt8`/`SInt8`/`char` -> [`Value::Bytes`]
/// - other arrays -> [`Value::Array`]
/// - `map` -> [`Value::Array`] of [`Value::Map`]s with keys `first` and `second`
/// - structs (including `PPtr<T>`, with `m_FileID`/`m_PathID`) -> [`Value::Map`]
/// - leaf nodes of unknown type -> [`Value::Null`]
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// No value.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer of any width.
    Int(i64),
    /// An unsigned integer of any width.
    UInt(u64),
    /// A 32-bit float.
    Float(f32),
    /// A 64-bit float.
    Double(f64),
    /// A UTF-8 string.
    String(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
    /// A sequence.
    Array(Vec<Value>),
    /// A struct, in field order.
    Map(Map),
}

/// Struct fields in TypeTree order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    /// An empty map with room for `n` fields.
    #[must_use]
    pub fn with_capacity(n: usize) -> Self {
        Self {
            entries: Vec::with_capacity(n),
        }
    }

    /// Appends a field.
    pub fn push(&mut self, key: impl Into<String>, value: Value) {
        self.entries.push((key.into(), value));
    }

    /// The first field named `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Fields in order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Field names in order.
    pub fn keys(&self) -> impl ExactSizeIterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// Number of fields.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Consumes the map into its fields.
    #[must_use]
    pub fn into_entries(self) -> Vec<(String, Value)> {
        self.entries
    }
}

impl<'a> IntoIterator for &'a Map {
    type Item = (&'a str, &'a Value);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, Value)>,
        fn(&'a (String, Value)) -> (&'a str, &'a Value),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(iter: I) -> Self {
        Self {
            entries: iter.into_iter().collect(),
        }
    }
}

impl Value {
    /// The field `key` of a [`Value::Map`].
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_map()?.get(key)
    }

    /// Follows a `/`-separated path of field names and array indices, such as
    /// `"mSprites/0/name"`.
    #[must_use]
    pub fn pointer(&self, path: &str) -> Option<&Value> {
        path.split('/')
            .filter(|s| !s.is_empty())
            .try_fold(self, |v, seg| match v {
                Value::Map(m) => m.get(seg),
                Value::Array(a) => a.get(seg.parse::<usize>().ok()?),
                _ => None,
            })
    }

    /// The value as a signed integer, if it is an integer that fits.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Int(i) => Some(i),
            Value::UInt(u) => i64::try_from(u).ok(),
            _ => None,
        }
    }

    /// The value as an unsigned integer, if it is a non-negative integer.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Value::Int(i) => u64::try_from(i).ok(),
            Value::UInt(u) => Some(u),
            _ => None,
        }
    }

    /// The value as a float; integers convert too.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Float(f) => Some(f64::from(f)),
            Value::Double(d) => Some(d),
            Value::Int(i) => Some(i as f64),
            Value::UInt(u) => Some(u as f64),
            _ => None,
        }
    }

    /// The value as a boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// The value as a string slice.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The value as raw bytes; strings yield their UTF-8 bytes.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            Value::String(s) => Some(s.as_bytes()),
            _ => None,
        }
    }

    /// The value as an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The value as a map.
    #[must_use]
    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The value as a [`PPtr`], if it is a map with `m_FileID` and `m_PathID`.
    #[must_use]
    pub fn as_pptr(&self) -> Option<PPtr> {
        let file_id = i32::try_from(self.get("m_FileID")?.as_i64()?).ok()?;
        let path_id = self.get("m_PathID")?.as_i64()?;
        Some(PPtr { file_id, path_id })
    }

    /// Whether this is [`Value::Null`].
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

/// Serializes as the natural JSON shape: maps as objects in field order,
/// bytes through `serialize_bytes` (an array of numbers in `serde_json`).
impl serde::Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => s.serialize_unit(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(i) => s.serialize_i64(*i),
            Value::UInt(u) => s.serialize_u64(*u),
            Value::Float(f) => s.serialize_f32(*f),
            Value::Double(d) => s.serialize_f64(*d),
            Value::String(v) => s.serialize_str(v),
            Value::Bytes(b) => s.serialize_bytes(b),
            Value::Array(a) => {
                let mut seq = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Value::Map(m) => m.serialize(s),
        }
    }
}

impl serde::Serialize for Map {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.len()))?;
        for (k, v) in self {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pptr(file_id: i64, path_id: i64) -> Value {
        let mut m = Map::default();
        m.push("m_FileID", Value::Int(file_id));
        m.push("m_PathID", Value::Int(path_id));
        Value::Map(m)
    }

    #[test]
    fn pointer_walks_maps_and_arrays() {
        let mut sprite = Map::default();
        sprite.push("name", Value::String("a".into()));
        let mut root = Map::default();
        root.push("mSprites", Value::Array(vec![Value::Map(sprite)]));
        let root = Value::Map(root);
        assert_eq!(
            root.pointer("mSprites/0/name").and_then(Value::as_str),
            Some("a")
        );
        assert!(root.pointer("mSprites/1/name").is_none());
        assert!(root.pointer("missing").is_none());
    }

    #[test]
    fn pptr_from_value() {
        assert_eq!(
            pptr(1, -42).as_pptr(),
            Some(PPtr {
                file_id: 1,
                path_id: -42
            })
        );
        assert_eq!(Value::Int(3).as_pptr(), None);
    }

    #[test]
    fn serializes_in_field_order() {
        let mut m = Map::default();
        m.push("z", Value::UInt(1));
        m.push("a", Value::Bool(true));
        m.push("b", Value::Bytes(vec![1, 2]));
        let json = serde_json::to_string(&Value::Map(m)).unwrap();
        assert_eq!(json, r#"{"z":1,"a":true,"b":[1,2]}"#);
    }

    #[test]
    fn integer_conversions() {
        assert_eq!(Value::UInt(u64::MAX).as_i64(), None);
        assert_eq!(Value::Int(-1).as_u64(), None);
        assert_eq!(Value::Int(5).as_u64(), Some(5));
        assert_eq!(Value::Float(0.5).as_f64(), Some(0.5));
    }
}
