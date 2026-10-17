//! TypeTree-driven reading of object data into [`Value`]s, plus fast
//! skipping for reading a single field (such as `m_Name`) without decoding
//! everything before it.

use rabex::typetree::TypeTreeNode;

use crate::error::{Error, ReadError};
use crate::reader::{ReadResult, Reader};
use crate::value::{Map, Value};

/// `kAlignBytes`: the field is followed by padding to a 4-byte boundary.
const ALIGN_BYTES: i32 = 0x4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prim {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl Prim {
    fn of(ty: &str) -> Option<Self> {
        Some(match ty {
            "bool" => Self::Bool,
            "SInt8" => Self::I8,
            "UInt8" | "char" => Self::U8,
            "SInt16" | "short" => Self::I16,
            "UInt16" | "unsigned short" => Self::U16,
            "SInt32" | "int" => Self::I32,
            "UInt32" | "unsigned int" | "Type*" => Self::U32,
            "SInt64" | "long long" => Self::I64,
            "UInt64" | "unsigned long long" | "FileSize" => Self::U64,
            "float" => Self::F32,
            "double" => Self::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Self::Bool | Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }

    fn read(self, r: &mut Reader<'_>) -> ReadResult<Value> {
        Ok(match self {
            Self::Bool => Value::Bool(r.u8()? != 0),
            Self::I8 => Value::Int(r.i8()?.into()),
            Self::U8 => Value::UInt(r.u8()?.into()),
            Self::I16 => Value::Int(r.i16()?.into()),
            Self::U16 => Value::UInt(r.u16()?.into()),
            Self::I32 => Value::Int(r.i32()?.into()),
            Self::U32 => Value::UInt(r.u32()?.into()),
            Self::I64 => Value::Int(r.i64()?),
            Self::U64 => Value::UInt(r.u64()?),
            Self::F32 => Value::Float(r.f32()?),
            Self::F64 => Value::Double(r.f64()?),
        })
    }
}

/// How a node is laid out in the data stream.
enum Shape<'t> {
    Prim(Prim),
    /// `string`: an `i32` length and that many bytes.
    String,
    /// `TypelessData`: an `i32` length and that many bytes.
    Typeless,
    /// An `Array` node (children: size, data).
    Array(&'t TypeTreeNode),
    /// Fields in order.
    Struct,
    /// A leaf of unknown type that occupies no bytes.
    Empty,
}

fn shape(node: &TypeTreeNode) -> Shape<'_> {
    if let Some(p) = Prim::of(&node.m_Type) {
        return Shape::Prim(p);
    }
    match node.m_Type.as_str() {
        "string" => Shape::String,
        "TypelessData" => Shape::Typeless,
        "Array" => Shape::Array(node),
        _ => match node.children.as_slice() {
            [] => Shape::Empty,
            [only] if only.m_Type == "Array" => Shape::Array(only),
            _ => Shape::Struct,
        },
    }
}

fn aligns(node: &TypeTreeNode) -> bool {
    node.m_MetaFlag.unwrap_or(0) & ALIGN_BYTES != 0
}

/// The encoded size of `node` when it is the same for every instance and
/// involves no alignment padding.
fn fixed_size(node: &TypeTreeNode) -> Option<usize> {
    if aligns(node) {
        return None;
    }
    match shape(node) {
        Shape::Prim(p) => Some(p.size()),
        Shape::Empty => Some(0),
        Shape::Struct => node
            .children
            .iter()
            .try_fold(0usize, |acc, c| acc.checked_add(fixed_size(c)?)),
        Shape::String | Shape::Typeless | Shape::Array(_) => None,
    }
}

fn length(r: &mut Reader<'_>) -> ReadResult<usize> {
    let n = r.i32()?;
    usize::try_from(n)
        .map_err(|_| Error::malformed("object data", format!("negative length {n}")).into())
}

/// The element node of an `Array` node.
fn element(array: &TypeTreeNode) -> ReadResult<&TypeTreeNode> {
    array
        .children
        .get(1)
        .ok_or_else(|| Error::malformed("TypeTree", "Array node without a data child").into())
}

/// Reads an array's count and checks it against the bytes left, so a corrupt
/// count fails fast instead of allocating.
fn array_count(r: &mut Reader<'_>, elem_min: usize) -> ReadResult<usize> {
    let count = length(r)?;
    let min_bytes = count.saturating_mul(elem_min.max(1));
    if min_bytes > r.remaining() {
        return Err(ReadError::Short {
            needed: (r.pos() as u64).saturating_add(min_bytes as u64),
        });
    }
    Ok(count)
}

/// Whether the node itself, or the `Array` inside a `string`/`vector`, asks
/// for alignment after it.
fn aligns_after(node: &TypeTreeNode) -> bool {
    aligns(node)
        || node
            .children
            .first()
            .is_some_and(|c| c.m_Type == "Array" && aligns(c))
}

/// Reads one node into a [`Value`].
pub(crate) fn read_value(node: &TypeTreeNode, r: &mut Reader<'_>) -> ReadResult<Value> {
    let value = match shape(node) {
        Shape::Prim(p) => p.read(r)?,
        Shape::String => {
            let len = length(r)?;
            let bytes = r.bytes(len)?;
            match std::str::from_utf8(bytes) {
                Ok(s) => Value::String(s.to_owned()),
                Err(_) => Value::Bytes(bytes.to_vec()),
            }
        }
        Shape::Typeless => {
            let len = length(r)?;
            Value::Bytes(r.bytes(len)?.to_vec())
        }
        Shape::Array(array) => read_array(array, r)?,
        Shape::Struct => {
            let mut map = Map::with_capacity(node.children.len());
            for child in &node.children {
                map.push(child.m_Name.clone(), read_value(child, r)?);
            }
            Value::Map(map)
        }
        Shape::Empty => Value::Null,
    };
    if aligns_after(node) {
        r.align(4);
    }
    Ok(value)
}

fn read_array(array: &TypeTreeNode, r: &mut Reader<'_>) -> ReadResult<Value> {
    let elem = element(array)?;
    let byte_like = elem.children.is_empty()
        && matches!(Prim::of(&elem.m_Type), Some(Prim::U8 | Prim::I8))
        && !aligns(elem);
    if byte_like {
        let count = length(r)?;
        return Ok(Value::Bytes(r.bytes(count)?.to_vec()));
    }
    let count = array_count(r, fixed_size(elem).unwrap_or(1))?;
    let items = (0..count)
        .map(|_| read_value(elem, r))
        .collect::<ReadResult<Vec<_>>>()?;
    Ok(Value::Array(items))
}

/// Advances past one node without building a value.
pub(crate) fn skip_value(node: &TypeTreeNode, r: &mut Reader<'_>) -> ReadResult<()> {
    if let Some(size) = fixed_size(node) {
        return r.skip(size);
    }
    match shape(node) {
        Shape::Prim(p) => r.skip(p.size())?,
        Shape::String | Shape::Typeless => {
            let len = length(r)?;
            r.skip(len)?;
        }
        Shape::Array(array) => {
            let elem = element(array)?;
            if let Some(size) = fixed_size(elem) {
                let count = length(r)?;
                r.skip(count.saturating_mul(size))?;
            } else {
                let count = array_count(r, 1)?;
                for _ in 0..count {
                    skip_value(elem, r)?;
                }
            }
        }
        Shape::Struct => {
            for child in &node.children {
                skip_value(child, r)?;
            }
        }
        Shape::Empty => {}
    }
    if aligns_after(node) {
        r.align(4);
    }
    Ok(())
}

/// Reads the top-level field `name` of an object, skipping the fields before
/// it. `Ok(None)` when the type has no such field.
pub(crate) fn read_field(
    root: &TypeTreeNode,
    r: &mut Reader<'_>,
    name: &str,
) -> ReadResult<Option<Value>> {
    for child in &root.children {
        if child.m_Name == name {
            return read_value(child, r).map(Some);
        }
        skip_value(child, r)?;
    }
    Ok(None)
}

/// Whether the type has a top-level field `name`.
pub(crate) fn has_field(root: &TypeTreeNode, name: &str) -> bool {
    root.children.iter().any(|c| c.m_Name == name)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn node(
        ty: &str,
        name: &str,
        flags: i32,
        children: Vec<TypeTreeNode>,
    ) -> TypeTreeNode {
        TypeTreeNode {
            m_Type: ty.into(),
            m_Name: name.into(),
            m_MetaFlag: Some(flags),
            m_ByteSize: -1,
            children,
            ..Default::default()
        }
    }

    pub(crate) fn string(name: &str) -> TypeTreeNode {
        node(
            "string",
            name,
            0,
            vec![node(
                "Array",
                "Array",
                ALIGN_BYTES,
                vec![
                    node("int", "size", 0, vec![]),
                    node("char", "data", 0, vec![]),
                ],
            )],
        )
    }

    pub(crate) fn vector(name: &str, elem: TypeTreeNode) -> TypeTreeNode {
        node(
            "vector",
            name,
            0,
            vec![node(
                "Array",
                "Array",
                0,
                vec![node("int", "size", 0, vec![]), elem],
            )],
        )
    }

    fn sample_tree() -> TypeTreeNode {
        node(
            "Thing",
            "Base",
            0,
            vec![
                string("m_Name"),
                node("bool", "m_Flag", ALIGN_BYTES, vec![]),
                vector(
                    "m_Points",
                    node(
                        "Vector2f",
                        "data",
                        0,
                        vec![node("float", "x", 0, vec![]), node("float", "y", 0, vec![])],
                    ),
                ),
                vector("m_Tags", string("data")),
                node("SInt64", "m_Last", 0, vec![]),
            ],
        )
    }

    fn sample_data() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&5i32.to_le_bytes());
        d.extend_from_slice(b"hello\0\0\0"); // 5 bytes + 3 padding
        d.extend_from_slice(&[1, 0, 0, 0]); // bool + padding
        d.extend_from_slice(&2i32.to_le_bytes());
        for f in [1.0f32, 2.0, 3.0, 4.0] {
            d.extend_from_slice(&f.to_le_bytes());
        }
        d.extend_from_slice(&1i32.to_le_bytes());
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(b"ab\0\0");
        d.extend_from_slice(&(-7i64).to_le_bytes());
        d
    }

    #[test]
    fn reads_structs_arrays_strings_and_alignment() {
        let tree = sample_tree();
        let data = sample_data();
        let mut r = Reader::new(&data, false);
        let v = read_value(&tree, &mut r).unwrap();
        assert_eq!(r.pos(), data.len());
        assert_eq!(v.get("m_Name").and_then(Value::as_str), Some("hello"));
        assert_eq!(v.get("m_Flag").and_then(Value::as_bool), Some(true));
        assert_eq!(v.pointer("m_Points/1/y").and_then(Value::as_f64), Some(4.0));
        assert_eq!(v.pointer("m_Tags/0").and_then(Value::as_str), Some("ab"));
        assert_eq!(v.get("m_Last").and_then(Value::as_i64), Some(-7));
        let keys: Vec<_> = v.as_map().unwrap().keys().collect();
        assert_eq!(keys, ["m_Name", "m_Flag", "m_Points", "m_Tags", "m_Last"]);
    }

    #[test]
    fn read_field_skips_to_the_field() {
        let tree = sample_tree();
        let data = sample_data();
        let mut r = Reader::new(&data, false);
        let v = read_field(&tree, &mut r, "m_Last").unwrap();
        assert_eq!(v, Some(Value::Int(-7)));
        let mut r = Reader::new(&data, false);
        assert_eq!(read_field(&tree, &mut r, "m_Missing").unwrap(), None);
    }

    #[test]
    fn truncated_data_reports_needed_length() {
        let tree = sample_tree();
        let data = sample_data();
        let mut r = Reader::new(&data[..6], false);
        match read_field(&tree, &mut r, "m_Name") {
            Err(ReadError::Short { needed }) => assert_eq!(needed, 9),
            other => panic!("expected short read, got {other:?}"),
        }
        for len in 0..data.len() {
            let mut r = Reader::new(&data[..len], false);
            assert!(
                read_value(&tree, &mut r).is_err(),
                "prefix of {len} bytes parsed"
            );
        }
    }

    #[test]
    fn corrupt_counts_fail_without_allocating() {
        let tree = vector(
            "m_Points",
            node("Vector2f", "data", 0, vec![node("float", "x", 0, vec![])]),
        );
        let data = i32::MAX.to_le_bytes();
        let mut r = Reader::new(&data, false);
        assert!(matches!(
            read_value(&tree, &mut r),
            Err(ReadError::Short { .. })
        ));
        let data = (-1i32).to_le_bytes();
        let mut r = Reader::new(&data, false);
        assert!(matches!(read_value(&tree, &mut r), Err(ReadError::Bad(_))));
    }

    #[test]
    fn byte_arrays_become_bytes() {
        let tree = vector("m_Data", node("UInt8", "data", 0, vec![]));
        let mut data = 3i32.to_le_bytes().to_vec();
        data.extend_from_slice(&[9, 8, 7]);
        let mut r = Reader::new(&data, false);
        assert_eq!(
            read_value(&tree, &mut r).unwrap(),
            Value::Bytes(vec![9, 8, 7])
        );
    }
}
