//! Unity Mesh Assets: geometry read from a Mesh's TypeTree value, and
//! Wavefront OBJ output.
//!
//! SWGOH ships most Meshes as an `m_CompressedMesh`: positions, UVs and
//! normals quantized into bit-packed vectors. Meshes kept readable carry
//! plain `m_VertexData` instead: interleaved vertex streams described by 14
//! channels (the layout of Unity 2019 and later), stored inline or streamed
//! from a `.resS` node. Both decode to the same [`Mesh`].
//!
//! The OBJ output matches swgoh-ae2: X is negated to turn Unity's
//! left-handed space into OBJ's right-handed one, and each triangle's
//! winding is reversed so faces keep pointing outwards.

use std::fmt::Write as _;

use ae3_unity::Value;

/// The vertex channels of Unity 2019 and later, by index.
const CHANNEL_COUNT: usize = 14;
const CHANNEL_POSITION: usize = 0;
const CHANNEL_NORMAL: usize = 1;
const CHANNEL_UV0: usize = 4;
/// Vertex streams start on 16-byte boundaries.
const STREAM_ALIGN: usize = 16;
/// `m_CompressedMesh.m_UVInfo`: 4 bits per UV set, the low two holding its
/// dimension minus one and the third whether it exists.
const UV_INFO_BITS: u64 = 4;
const UV_DIMENSION_MASK: u64 = 3;
const UV_EXISTS: u64 = 4;

/// A Mesh's geometry, in Unity's left-handed space with Unity's winding.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    /// `m_Name`.
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    /// One per position, or empty when the Mesh has none.
    pub normals: Vec<[f32; 3]>,
    /// The first texture coordinate set: one per position, or empty.
    pub uvs: Vec<[f32; 2]>,
    pub submeshes: Vec<SubMesh>,
}

/// The triangles drawn with one material.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubMesh {
    /// Vertex indices (into [`Mesh::positions`]), wound clockwise as Unity
    /// draws front faces.
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// Triangles over all submeshes.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.submeshes.iter().map(|s| s.triangles.len()).sum()
    }

    /// The smallest and largest coordinates of the finite positions, or
    /// `None` when there are none.
    #[must_use]
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        self.positions
            .iter()
            .filter(|p| p.iter().all(|c| c.is_finite()))
            .fold(None, |acc, p| {
                let (mut min, mut max) = acc.unwrap_or((*p, *p));
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
                Some((min, max))
            })
    }

    /// The Mesh as Wavefront OBJ text, in swgoh-ae2's form: a group for the
    /// Mesh, then one per submesh (`{name}_{i}`); X negated and windings
    /// reversed. Non-finite numbers are written as 0.
    #[must_use]
    pub fn to_obj(&self) -> String {
        let (uvs, normals) = (!self.uvs.is_empty(), !self.normals.is_empty());
        let mut out =
            String::with_capacity(64 + self.positions.len() * 96 + self.triangle_count() * 48);
        // Writing to a String cannot fail.
        let _ = writeln!(out, "g {}", self.name);
        for [x, y, z] in &self.positions {
            let _ = writeln!(out, "v {} {} {}", finite(-x), finite(*y), finite(*z));
        }
        for [u, v] in &self.uvs {
            let _ = writeln!(out, "vt {} {}", finite(*u), finite(*v));
        }
        for [x, y, z] in &self.normals {
            let _ = writeln!(out, "vn {} {} {}", finite(-x), finite(*y), finite(*z));
        }
        for (i, submesh) in self.submeshes.iter().enumerate() {
            let _ = writeln!(out, "g {}_{i}", self.name);
            for &[a, b, c] in &submesh.triangles {
                out.push('f');
                for index in [c, b, a] {
                    let n = u64::from(index) + 1;
                    let _ = match (uvs, normals) {
                        (true, true) => write!(out, " {n}/{n}/{n}"),
                        (true, false) => write!(out, " {n}/{n}"),
                        (false, true) => write!(out, " {n}//{n}"),
                        (false, false) => write!(out, " {n}"),
                    };
                }
                out.push('\n');
            }
        }
        out
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Decodes a Mesh's TypeTree value. `stream` is the data its
/// `m_StreamData` points at, when that is not empty.
pub(crate) fn decode(value: &Value, stream: Option<&[u8]>) -> Result<Mesh, String> {
    let name = value
        .get("m_Name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let index_size = match value.get("m_IndexFormat").and_then(Value::as_u64) {
        None | Some(0) => 2,
        Some(1) => 4,
        Some(n) => return Err(format!("unknown index format {n}")),
    };
    let compressed = value
        .get("m_CompressedMesh")
        .filter(|c| c.get("m_Vertices").is_some_and(|v| item_count(v) > 0));
    let (vertices, indices) = match compressed {
        Some(c) => decode_compressed(c)?,
        None => decode_vertex_data(value, stream, index_size)?,
    };
    let submeshes = submeshes(value, &indices, index_size, vertices.positions.len())?;
    Ok(Mesh {
        name,
        positions: vertices.positions,
        normals: vertices.normals,
        uvs: vertices.uvs,
        submeshes,
    })
}

#[derive(Default)]
struct Vertices {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
}

fn usize_of(value: Option<&Value>) -> Option<usize> {
    value
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

fn item_count(packed: &Value) -> usize {
    usize_of(packed.get("m_NumItems")).unwrap_or(0)
}

/// Splits each submesh's indices into triangles.
fn submeshes(
    value: &Value,
    indices: &[u32],
    index_size: u64,
    vertex_count: usize,
) -> Result<Vec<SubMesh>, String> {
    let Some(list) = value.get("m_SubMeshes").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    list.iter()
        .enumerate()
        .map(|(i, sub)| {
            let first = sub
                .get("firstByte")
                .and_then(Value::as_u64)
                .map_or(0, |b| b / index_size);
            let count = usize_of(sub.get("indexCount")).unwrap_or(0);
            let base = sub
                .get("baseVertex")
                .and_then(Value::as_u64)
                .and_then(|b| u32::try_from(b).ok())
                .unwrap_or(0);
            let slice = usize::try_from(first)
                .ok()
                .and_then(|first| indices.get(first..first.checked_add(count)?))
                .ok_or_else(|| format!("submesh {i} reaches past the index buffer"))?;
            let shifted: Vec<u32> = slice
                .iter()
                .map(|&v| v.checked_add(base))
                .collect::<Option<_>>()
                .ok_or_else(|| format!("submesh {i} has a vertex index overflow"))?;
            let triangles = match sub.get("topology").and_then(Value::as_u64).unwrap_or(0) {
                0 => shifted.as_chunks::<3>().0.to_vec(),
                1 => destrip(&shifted),
                2 => shifted
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]])
                    .collect(),
                // Lines and points have no faces.
                _ => Vec::new(),
            };
            if let Some(&bad) = triangles
                .iter()
                .flatten()
                .find(|&&v| v as usize >= vertex_count)
            {
                return Err(format!(
                    "submesh {i} refers to vertex {bad} of {vertex_count}"
                ));
            }
            Ok(SubMesh { triangles })
        })
        .collect()
}

/// A triangle strip as triangles, flipping every other one to keep the
/// winding and dropping degenerate ones.
fn destrip(strip: &[u32]) -> Vec<[u32; 3]> {
    strip
        .windows(3)
        .enumerate()
        .filter(|(_, w)| w[0] != w[1] && w[0] != w[2] && w[1] != w[2])
        .map(|(i, w)| {
            if i % 2 == 1 {
                [w[1], w[0], w[2]]
            } else {
                [w[0], w[1], w[2]]
            }
        })
        .collect()
}

// --- m_CompressedMesh ---

/// A `PackedBitVector`: `m_NumItems` values of `m_BitSize` bits each,
/// least significant bit first. Float vectors map `0..=2^bits-1` onto
/// `m_Start..=m_Start+m_Range`.
struct Packed<'a> {
    items: usize,
    bits: u32,
    data: &'a [u8],
    start: f32,
    range: f32,
}

impl<'a> Packed<'a> {
    fn from_value(compressed: &'a Value, key: &str) -> Result<Option<Self>, String> {
        let Some(v) = compressed.get(key) else {
            return Ok(None);
        };
        let items = item_count(v);
        if items == 0 {
            return Ok(None);
        }
        let bits = v
            .get("m_BitSize")
            .and_then(Value::as_u64)
            .and_then(|b| u32::try_from(b).ok())
            .filter(|&b| b <= 32)
            .ok_or_else(|| format!("{key} has an invalid bit size"))?;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the TypeTree declares these as 32-bit floats"
        )]
        let float = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        Ok(Some(Self {
            items,
            bits,
            data: v
                .get("m_Data")
                .and_then(Value::as_bytes)
                .unwrap_or_default(),
            start: float("m_Start"),
            range: float("m_Range"),
        }))
    }

    /// `count` raw values from item `first` on.
    fn ints(&self, first: usize, count: usize) -> Result<Vec<u32>, String> {
        let bits = self.bits as usize;
        let end = first
            .checked_add(count)
            .filter(|&end| end <= self.items)
            .ok_or("a packed vector is shorter than its Mesh needs")?;
        if end.saturating_mul(bits).div_ceil(8) > self.data.len() {
            return Err("a packed vector's data is shorter than its item count".into());
        }
        let mask = (1u64 << self.bits) - 1;
        Ok((first..end)
            .map(|item| {
                let pos = item * bits;
                // Up to 32 bits at any bit offset span at most 5 bytes.
                let acc = self.data[pos / 8..]
                    .iter()
                    .take(5)
                    .enumerate()
                    .fold(0u64, |acc, (i, &b)| acc | u64::from(b) << (8 * i));
                #[expect(clippy::cast_possible_truncation, reason = "masked to 32 bits")]
                let value = ((acc >> (pos % 8)) & mask) as u32;
                value
            })
            .collect())
    }

    /// `count` values from item `first` on, dequantized as Unity does.
    #[expect(
        clippy::cast_precision_loss,
        reason = "quantized values have at most 32 bits, as in Unity's f32 math"
    )]
    fn floats(&self, first: usize, count: usize) -> Result<Vec<f32>, String> {
        let ints = self.ints(first, count)?;
        if self.bits == 0 {
            return Ok(vec![self.start; ints.len()]);
        }
        let scale = 1.0 / self.range;
        let max = ((1u64 << self.bits) - 1) as f32;
        Ok(ints
            .into_iter()
            .map(|x| x as f32 / (scale * max) + self.start)
            .collect())
    }
}

fn decode_compressed(c: &Value) -> Result<(Vertices, Vec<u32>), String> {
    let mut vertices = Vertices::default();
    if let Some(packed) = Packed::from_value(c, "m_Vertices")? {
        vertices.positions = packed
            .floats(0, packed.items / 3 * 3)?
            .as_chunks::<3>()
            .0
            .to_vec();
    }
    let n = vertices.positions.len();
    vertices.uvs = compressed_uv0(c, n)?;
    vertices.normals = compressed_normals(c, n)?;
    let indices = match Packed::from_value(c, "m_Triangles")? {
        Some(packed) => packed.ints(0, packed.items)?,
        None => Vec::new(),
    };
    Ok((vertices, indices))
}

/// The first UV set. With `m_UVInfo` 0 (older Meshes), the UVs are two
/// 2D sets in a row.
fn compressed_uv0(c: &Value, n: usize) -> Result<Vec<[f32; 2]>, String> {
    let Some(packed) = Packed::from_value(c, "m_UV")? else {
        return Ok(Vec::new());
    };
    let info = c.get("m_UVInfo").and_then(Value::as_u64).unwrap_or(0);
    let dim = if info == 0 {
        2
    } else {
        let set0 = info & ((1 << UV_INFO_BITS) - 1);
        if set0 & UV_EXISTS == 0 {
            return Ok(Vec::new());
        }
        // 1..=4, so the cast is lossless.
        1 + (set0 & UV_DIMENSION_MASK) as usize
    };
    if packed.items < n * dim {
        return Ok(Vec::new());
    }
    Ok(packed
        .floats(0, n * dim)?
        .chunks_exact(dim)
        .map(|uv| [uv[0], uv.get(1).copied().unwrap_or(0.0)])
        .collect())
}

/// Normals are stored as X and Y, with Z's sign in `m_NormalSigns`.
fn compressed_normals(c: &Value, n: usize) -> Result<Vec<[f32; 3]>, String> {
    let Some(packed) = Packed::from_value(c, "m_Normals")? else {
        return Ok(Vec::new());
    };
    if packed.items / 2 != n {
        return Ok(Vec::new());
    }
    let xy = packed.floats(0, n * 2)?;
    let signs = match Packed::from_value(c, "m_NormalSigns")? {
        Some(s) => s.ints(0, s.items.min(n))?,
        None => Vec::new(),
    };
    Ok(xy
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
        .map(|(i, &[mut nx, mut ny])| {
            let zsqr = 1.0 - nx * nx - ny * ny;
            let mut nz = if zsqr >= 0.0 {
                zsqr.sqrt()
            } else {
                let len = nx.hypot(ny);
                if len > 0.0 {
                    nx /= len;
                    ny /= len;
                }
                0.0
            };
            if signs.get(i) == Some(&0) {
                nz = -nz;
            }
            [nx, ny, nz]
        })
        .collect())
}

// --- m_VertexData ---

/// Unity's `VertexFormat` (2019 and later).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Float32,
    Float16,
    UNorm8,
    SNorm8,
    UNorm16,
    SNorm16,
    UInt8,
    SInt8,
    UInt16,
    SInt16,
    UInt32,
    SInt32,
}

impl Format {
    fn from_id(id: u64) -> Option<Self> {
        Some(match id {
            0 => Self::Float32,
            1 => Self::Float16,
            2 => Self::UNorm8,
            3 => Self::SNorm8,
            4 => Self::UNorm16,
            5 => Self::SNorm16,
            6 => Self::UInt8,
            7 => Self::SInt8,
            8 => Self::UInt16,
            9 => Self::SInt16,
            10 => Self::UInt32,
            11 => Self::SInt32,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Self::UNorm8 | Self::SNorm8 | Self::UInt8 | Self::SInt8 => 1,
            Self::Float16 | Self::UNorm16 | Self::SNorm16 | Self::UInt16 | Self::SInt16 => 2,
            Self::Float32 | Self::UInt32 | Self::SInt32 => 4,
        }
    }

    /// One little-endian component; `bytes` is exactly [`Format::size`] long.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "integer attributes become f32 as the GPU would read them"
    )]
    fn read(self, bytes: &[u8]) -> f32 {
        let b2 = || [bytes[0], bytes[1]];
        let b4 = || [bytes[0], bytes[1], bytes[2], bytes[3]];
        match self {
            Self::Float32 => f32::from_le_bytes(b4()),
            Self::Float16 => f16_to_f32(u16::from_le_bytes(b2())),
            Self::UNorm8 => f32::from(bytes[0]) / 255.0,
            Self::SNorm8 => (f32::from(i8::from_le_bytes([bytes[0]])) / 127.0).max(-1.0),
            Self::UNorm16 => f32::from(u16::from_le_bytes(b2())) / 65535.0,
            Self::SNorm16 => (f32::from(i16::from_le_bytes(b2())) / 32767.0).max(-1.0),
            Self::UInt8 => f32::from(bytes[0]),
            Self::SInt8 => f32::from(i8::from_le_bytes([bytes[0]])),
            Self::UInt16 => f32::from(u16::from_le_bytes(b2())),
            Self::SInt16 => f32::from(i16::from_le_bytes(b2())),
            Self::UInt32 => u32::from_le_bytes(b4()) as f32,
            Self::SInt32 => f64::from(i32::from_le_bytes(b4())) as f32,
        }
    }
}

/// An IEEE half-precision float.
fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = u32::from((h >> 10) & 0x1f);
    let mantissa = u32::from(h & 0x3ff);
    match exponent {
        // Zero and subnormals: mantissa * 2^-24.
        0 => sign * f32::from(h & 0x3ff) * f32::from_bits(0x3380_0000),
        0x1f => f32::from_bits(u32::from(h & 0x8000) << 16 | 0x7f80_0000 | mantissa << 13),
        _ => f32::from_bits(u32::from(h & 0x8000) << 16 | (exponent + 112) << 23 | mantissa << 13),
    }
}

/// One `ChannelInfo`, with the offset of its stream resolved.
#[derive(Debug, Clone, Copy)]
struct Channel {
    /// Byte offset of this channel in the first vertex.
    start: usize,
    stride: usize,
    format: Format,
    dimension: usize,
}

fn channels(vertex_data: &Value, vertex_count: usize) -> Result<Vec<Option<Channel>>, String> {
    struct Info {
        stream: usize,
        offset: usize,
        format: u64,
        dimension: usize,
    }
    let infos: Vec<Info> = vertex_data
        .get("m_Channels")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|c| Info {
            stream: usize_of(c.get("stream")).unwrap_or(0),
            offset: usize_of(c.get("offset")).unwrap_or(0),
            format: c.get("format").and_then(Value::as_u64).unwrap_or(0),
            // The high bits may carry flags.
            dimension: usize_of(c.get("dimension")).unwrap_or(0) & 0xf,
        })
        .collect();
    if infos.len() != CHANNEL_COUNT {
        return Err(format!(
            "unsupported vertex layout with {} channels; AE3 reads the 14-channel layout of Unity 2019 and later",
            infos.len()
        ));
    }
    let format = |info: &Info| {
        Format::from_id(info.format).ok_or_else(|| format!("unknown vertex format {}", info.format))
    };
    // Streams follow one another, each padded to 16 bytes.
    let stream_count = infos.iter().map(|c| c.stream + 1).max().unwrap_or(0);
    let mut stream_start = Vec::with_capacity(stream_count);
    let mut stream_stride = Vec::with_capacity(stream_count);
    let mut offset = 0usize;
    for s in 0..stream_count {
        let stride = infos
            .iter()
            .filter(|c| c.stream == s && c.dimension > 0)
            .map(|c| Ok(c.dimension * format(c)?.size()))
            .sum::<Result<usize, String>>()?;
        stream_start.push(offset);
        stream_stride.push(stride);
        offset = offset
            .checked_add(
                vertex_count
                    .checked_mul(stride)
                    .ok_or("vertex data too large")?,
            )
            .ok_or("vertex data too large")?
            .next_multiple_of(STREAM_ALIGN);
    }
    infos
        .iter()
        .map(|c| {
            if c.dimension == 0 {
                return Ok(None);
            }
            Ok(Some(Channel {
                start: stream_start[c.stream] + c.offset,
                stride: stream_stride[c.stream],
                format: format(c)?,
                dimension: c.dimension,
            }))
        })
        .collect()
}

/// The first `N` components of a channel for each vertex, zero-padded.
fn read_channel<const N: usize>(
    data: &[u8],
    channel: Channel,
    vertex_count: usize,
) -> Result<Vec<[f32; N]>, String> {
    if vertex_count == 0 {
        return Ok(Vec::new());
    }
    let size = channel.format.size();
    let end = (vertex_count - 1)
        .checked_mul(channel.stride)
        .and_then(|o| o.checked_add(channel.start + channel.dimension * size))
        .ok_or("vertex data too large")?;
    if end > data.len() {
        return Err(format!(
            "the vertex data is {} bytes, shorter than the {end} its channels describe",
            data.len()
        ));
    }
    let used = channel.dimension.min(N);
    Ok((0..vertex_count)
        .map(|v| {
            let base = channel.start + v * channel.stride;
            let mut out = [0.0; N];
            for (i, slot) in out.iter_mut().enumerate().take(used) {
                let at = base + i * size;
                *slot = channel.format.read(&data[at..at + size]);
            }
            out
        })
        .collect())
}

fn decode_vertex_data(
    value: &Value,
    stream: Option<&[u8]>,
    index_size: u64,
) -> Result<(Vertices, Vec<u32>), String> {
    let vertex_data = value
        .get("m_VertexData")
        .ok_or("the Mesh has no m_VertexData")?;
    let n = usize_of(vertex_data.get("m_VertexCount")).unwrap_or(0);
    let mut vertices = Vertices::default();
    if n > 0 {
        let data = stream.unwrap_or_else(|| {
            vertex_data
                .get("m_DataSize")
                .and_then(Value::as_bytes)
                .unwrap_or_default()
        });
        let channels = channels(vertex_data, n)?;
        let position = channels[CHANNEL_POSITION].ok_or("the Mesh has no positions")?;
        vertices.positions = read_channel(data, position, n)?;
        if let Some(normal) = channels[CHANNEL_NORMAL].filter(|c| c.dimension >= 3) {
            vertices.normals = read_channel(data, normal, n)?;
        }
        if let Some(uv) = channels[CHANNEL_UV0] {
            vertices.uvs = read_channel(data, uv, n)?;
        }
    }
    let bytes = value
        .get("m_IndexBuffer")
        .and_then(Value::as_bytes)
        .unwrap_or_default();
    let indices = if index_size == 4 {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| u32::from_le_bytes(b))
            .collect()
    } else {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u32::from(u16::from_le_bytes(b)))
            .collect()
    };
    Ok((vertices, indices))
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "test data is small and in range"
)]
mod tests {
    use ae3_unity::Map;

    use super::*;

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    /// Packs values LSB-first, `bits` each, as Unity's `PackedBitVector`.
    fn pack(values: &[u32], bits: u32) -> Vec<u8> {
        let mut out = vec![0u8; (values.len() * bits as usize).div_ceil(8)];
        for (i, &v) in values.iter().enumerate() {
            for b in 0..bits as usize {
                if v >> b & 1 == 1 {
                    let pos = i * bits as usize + b;
                    out[pos / 8] |= 1 << (pos % 8);
                }
            }
        }
        out
    }

    fn packed_ints(values: &[u32], bits: u32) -> Value {
        map(vec![
            ("m_NumItems", Value::UInt(values.len() as u64)),
            ("m_Data", Value::Bytes(pack(values, bits))),
            ("m_BitSize", Value::UInt(u64::from(bits))),
        ])
    }

    /// Quantizes floats in `start..=start+range` to `bits`.
    fn packed_floats(values: &[f32], bits: u32, start: f32, range: f32) -> Value {
        let max = ((1u64 << bits) - 1) as f32;
        let ints: Vec<u32> = values
            .iter()
            .map(|v| ((v - start) / range * max).round() as u32)
            .collect();
        map(vec![
            ("m_NumItems", Value::UInt(values.len() as u64)),
            ("m_Range", Value::Float(range)),
            ("m_Start", Value::Float(start)),
            ("m_Data", Value::Bytes(pack(&ints, bits))),
            ("m_BitSize", Value::UInt(u64::from(bits))),
        ])
    }

    fn submesh(first_byte: u64, index_count: u64, topology: i64) -> Value {
        map(vec![
            ("firstByte", Value::UInt(first_byte)),
            ("indexCount", Value::UInt(index_count)),
            ("topology", Value::Int(topology)),
            ("baseVertex", Value::UInt(0)),
        ])
    }

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn packed_ints_read_lsb_first_across_bytes() {
        let values = [1, 2, 3, 4, 5, 6, 7, 0, 5];
        let v = map(vec![("m", packed_ints(&values, 3))]);
        let packed = Packed::from_value(&v, "m").unwrap().unwrap();
        assert_eq!(packed.ints(0, 9).unwrap(), values);
        assert_eq!(packed.ints(4, 3).unwrap(), [5, 6, 7]);
        assert!(packed.ints(8, 2).is_err());

        let wide = [u32::MAX, 0, 0x8000_0001];
        let v = map(vec![("m", packed_ints(&wide, 32))]);
        let packed = Packed::from_value(&v, "m").unwrap().unwrap();
        assert_eq!(packed.ints(0, 3).unwrap(), wide);
    }

    #[test]
    fn packed_floats_span_start_to_start_plus_range() {
        let v = map(vec![("m", packed_floats(&[-1.0, 0.0, 1.0], 8, -1.0, 2.0))]);
        let floats = Packed::from_value(&v, "m")
            .unwrap()
            .unwrap()
            .floats(0, 3)
            .unwrap();
        assert!(close(floats[0], -1.0, 1e-6));
        assert!(close(floats[1], 0.0, 2.0 / 255.0));
        assert!(close(floats[2], 1.0, 1e-6));
    }

    #[test]
    fn short_packed_data_is_an_error() {
        let mut v = packed_ints(&[1, 2, 3], 8);
        if let Value::Map(m) = &mut v {
            *m = m
                .iter()
                .map(|(k, v)| {
                    let v = if k == "m_Data" {
                        Value::Bytes(vec![1])
                    } else {
                        v.clone()
                    };
                    (k.to_owned(), v)
                })
                .collect::<Map>();
        }
        let v = map(vec![("m", v)]);
        assert!(
            Packed::from_value(&v, "m")
                .unwrap()
                .unwrap()
                .ints(0, 3)
                .is_err()
        );
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x8000), -0.0);
        assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
        assert!(f16_to_f32(0x7e00).is_nan());
    }

    #[test]
    fn strips_and_quads_become_triangles() {
        assert_eq!(destrip(&[0, 1, 2, 3]), [[0, 1, 2], [2, 1, 3]]);
        // Degenerate triangles that join strips are dropped but keep their
        // place in the alternation.
        assert_eq!(destrip(&[0, 1, 2, 2, 3, 4]), [[0, 1, 2], [3, 2, 4]]);
        let value = map(vec![(
            "m_SubMeshes",
            Value::Array(vec![submesh(0, 4, 2), submesh(8, 2, 3)]),
        )]);
        let subs = submeshes(&value, &[0, 1, 2, 3, 0, 1], 2, 4).unwrap();
        assert_eq!(subs[0].triangles, [[0, 1, 2], [0, 2, 3]]);
        assert!(subs[1].triangles.is_empty(), "lines have no faces");
    }

    #[test]
    fn bad_submeshes_are_errors() {
        let value = map(vec![("m_SubMeshes", Value::Array(vec![submesh(2, 3, 0)]))]);
        let e = submeshes(&value, &[0, 1, 2], 2, 3).unwrap_err();
        assert!(e.contains("past the index buffer"), "{e}");
        let value = map(vec![("m_SubMeshes", Value::Array(vec![submesh(0, 3, 0)]))]);
        let e = submeshes(&value, &[0, 1, 7], 2, 3).unwrap_err();
        assert!(e.contains("vertex 7 of 3"), "{e}");
    }

    fn channel(stream: u64, offset: u64, format: u64, dimension: u64) -> Value {
        map(vec![
            ("stream", Value::UInt(stream)),
            ("offset", Value::UInt(offset)),
            ("format", Value::UInt(format)),
            ("dimension", Value::UInt(dimension)),
        ])
    }

    /// One triangle: Float32 positions and Float16 normals in stream 0,
    /// `UNorm16` UVs in stream 1, with u32 indices.
    fn uncompressed() -> Value {
        let positions = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.5]];
        let mut data = Vec::new();
        for p in positions {
            for c in p {
                data.extend(c.to_le_bytes());
            }
            // Normal (0, 0, -1) as halves, padded to 4 components.
            data.extend([0, 0, 0, 0, 0x00, 0xbc, 0, 0]);
        }
        // Stream 0 is 3 * 20 = 60 bytes, so stream 1 starts at 64.
        data.resize(64, 0);
        for uv in [[0u16, 0], [65535, 0], [0, 65535]] {
            for c in uv {
                data.extend(c.to_le_bytes());
            }
        }
        let mut channels = vec![channel(0, 0, 0, 3), channel(0, 12, 1, 4)];
        channels.extend((2..CHANNEL_COUNT).map(|i| {
            if i == CHANNEL_UV0 {
                channel(1, 0, 4, 2)
            } else {
                channel(0, 0, 0, 0)
            }
        }));
        let indices: Vec<u8> = [0u32, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        map(vec![
            ("m_Name", Value::String("tri".into())),
            ("m_SubMeshes", Value::Array(vec![submesh(0, 3, 0)])),
            ("m_IndexFormat", Value::Int(1)),
            ("m_IndexBuffer", Value::Bytes(indices)),
            (
                "m_VertexData",
                map(vec![
                    ("m_VertexCount", Value::UInt(3)),
                    ("m_Channels", Value::Array(channels)),
                    ("m_DataSize", Value::Bytes(data)),
                ]),
            ),
        ])
    }

    #[test]
    fn decodes_vertex_data_streams() {
        let mesh = decode(&uncompressed(), None).unwrap();
        assert_eq!(mesh.name, "tri");
        assert_eq!(
            mesh.positions,
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.5]]
        );
        assert_eq!(mesh.normals, [[0.0, 0.0, -1.0]; 3]);
        assert_eq!(mesh.uvs, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        assert_eq!(mesh.submeshes[0].triangles, [[0, 1, 2]]);
        assert_eq!(mesh.bounds(), Some(([0.0, 0.0, 0.0], [1.0, 2.0, 0.5])));
    }

    #[test]
    fn streamed_vertex_data_replaces_the_inline_bytes() {
        let value = uncompressed();
        let data = value
            .pointer("m_VertexData/m_DataSize")
            .and_then(Value::as_bytes)
            .unwrap()
            .to_vec();
        assert!(
            decode(&value, Some(&data[..40]))
                .unwrap_err()
                .contains("shorter")
        );
        assert_eq!(
            decode(&value, Some(&data)).unwrap(),
            decode(&value, None).unwrap()
        );
    }

    #[test]
    fn decodes_compressed_meshes() {
        let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let compressed = map(vec![
            ("m_Vertices", packed_floats(&positions, 16, -1.0, 2.0)),
            (
                "m_UV",
                packed_floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0], 10, 0.0, 1.0),
            ),
            // X and Y of (0.6, 0, ±0.8) and (0, 0, ±1).
            (
                "m_Normals",
                packed_floats(&[0.6, 0.0, 0.0, 0.0, 0.0, 0.0], 8, -1.0, 2.0),
            ),
            ("m_NormalSigns", packed_ints(&[1, 0, 1], 1)),
            ("m_Triangles", packed_ints(&[0, 1, 2], 2)),
            ("m_UVInfo", Value::UInt(5)),
        ]);
        let value = map(vec![
            ("m_Name", Value::String("c".into())),
            ("m_SubMeshes", Value::Array(vec![submesh(0, 3, 0)])),
            ("m_CompressedMesh", compressed),
        ]);
        let mesh = decode(&value, None).unwrap();
        assert_eq!(mesh.positions.len(), 3);
        assert!(close(mesh.positions[1][0], 1.0, 1e-4));
        assert!(close(mesh.positions[2][1], 1.0, 1e-4));
        assert!(close(mesh.uvs[1][0], 1.0, 1e-3) && close(mesh.uvs[2][1], 1.0, 1e-3));
        let n = mesh.normals[0];
        assert!(close(n[0], 0.6, 0.01) && close(n[2], 0.8, 0.01), "{n:?}");
        assert!(mesh.normals[1][2] < -0.99, "sign 0 flips Z");
        assert_eq!(mesh.submeshes[0].triangles, [[0, 1, 2]]);
    }

    #[test]
    fn unsupported_layouts_are_errors() {
        let value = map(vec![(
            "m_VertexData",
            map(vec![
                ("m_VertexCount", Value::UInt(1)),
                ("m_Channels", Value::Array(vec![channel(0, 0, 0, 3)])),
                ("m_DataSize", Value::Bytes(vec![0; 12])),
            ]),
        )]);
        assert!(decode(&value, None).unwrap_err().contains("1 channels"));
        let value = map(vec![("m_IndexFormat", Value::Int(7))]);
        assert!(decode(&value, None).unwrap_err().contains("index format 7"));
    }

    #[test]
    fn obj_negates_x_and_reverses_winding() {
        let mesh = decode(&uncompressed(), None).unwrap();
        let obj = mesh.to_obj();
        let lines: Vec<&str> = obj.lines().collect();
        assert_eq!(lines[0], "g tri");
        assert_eq!(lines[2], "v -1 0 0");
        assert_eq!(lines[3], "v -0 2 0.5");
        assert_eq!(lines[4], "vt 0 0");
        assert_eq!(lines[7], "vn -0 0 -1");
        assert_eq!(lines[10], "g tri_0");
        assert_eq!(lines[11], "f 3/3/3 2/2/2 1/1/1");

        let bare = Mesh {
            name: "b".into(),
            positions: vec![[f32::NAN, 0.0, 0.0]; 3],
            normals: Vec::new(),
            uvs: Vec::new(),
            submeshes: vec![SubMesh {
                triangles: vec![[0, 1, 2]],
            }],
        };
        assert_eq!(
            bare.to_obj(),
            "g b\nv 0 0 0\nv 0 0 0\nv 0 0 0\ng b_0\nf 3 2 1\n"
        );
        let with_normals = Mesh {
            normals: vec![[0.0; 3]; 3],
            ..bare
        };
        assert!(with_normals.to_obj().ends_with("f 3//3 2//2 1//1\n"));
    }
}
