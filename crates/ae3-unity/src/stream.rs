//! StreamingInfo: where an Asset's bulk data (texture pixels, audio) lives in
//! a resource node such as `CAB-<md5>.resS`.

use crate::value::Value;

/// A reference to bulk data stored outside the SerializedFile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamingInfo {
    /// Resource path, such as `archive:/CAB-<md5>/CAB-<md5>.resS`.
    pub path: String,
    /// Byte offset within the resource node.
    pub offset: u64,
    /// Byte length.
    pub size: u64,
}

impl StreamingInfo {
    /// Builds a StreamingInfo from either Unity shape: `StreamingInfo`
    /// (`path`, `offset`, `size`, as in `Texture2D.m_StreamData`) or
    /// `StreamedResource` (`m_Source`, `m_Offset`, `m_Size`, as in
    /// `AudioClip.m_Resource`).
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let (path, offset, size) = if value.get("path").is_some() {
            (value.get("path")?, value.get("offset")?, value.get("size")?)
        } else {
            (
                value.get("m_Source")?,
                value.get("m_Offset")?,
                value.get("m_Size")?,
            )
        };
        Some(Self {
            path: path.as_str()?.to_owned(),
            offset: offset.as_u64()?,
            size: size.as_u64()?,
        })
    }

    /// Whether the data is stored inline instead (an empty path or size).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.path.is_empty() || self.size == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Map;

    #[test]
    fn both_shapes() {
        let mut m = Map::default();
        m.push("offset", Value::UInt(16));
        m.push("size", Value::UInt(32));
        m.push("path", Value::String("archive:/CAB-a/CAB-a.resS".into()));
        let s = StreamingInfo::from_value(&Value::Map(m)).unwrap();
        assert_eq!((s.offset, s.size), (16, 32));
        assert!(!s.is_empty());

        let mut m = Map::default();
        m.push("m_Source", Value::String(String::new()));
        m.push("m_Offset", Value::UInt(0));
        m.push("m_Size", Value::UInt(0));
        assert!(
            StreamingInfo::from_value(&Value::Map(m))
                .unwrap()
                .is_empty()
        );
        assert!(StreamingInfo::from_value(&Value::Null).is_none());
    }
}
