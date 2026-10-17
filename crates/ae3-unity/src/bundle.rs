//! UnityFS Bundles: header, blocks-info, block decompression and nodes.

use crate::decompress::{Compression, decompress_block};
use crate::error::{Error, ReadError, Result};
use crate::reader::{ReadResult, Reader};
use crate::serialized::SerializedFile;
use crate::stream::StreamingInfo;

/// Blocks-info and directory are stored together after the header.
const FLAG_COMBINED: u32 = 0x40;
/// Blocks-info is stored at the end of the file.
const FLAG_INFO_AT_END: u32 = 0x80;
/// Block data starts on a 16-byte boundary (Unity 2020.3.34+ meaning of 0x200).
const FLAG_PADDING_AT_START: u32 = 0x200;
/// UnityCN-style encryption, which AE3 does not support.
const FLAG_ENCRYPTED: u32 = 0x400;
/// Per-block flag for an encrypted block.
const BLOCK_FLAG_ENCRYPTED: u16 = 0x100;
/// Node flag marking a SerializedFile.
const NODE_FLAG_SERIALIZED: u32 = 0x4;

/// Refuse to inflate a Bundle beyond this; nothing SWGOH ships comes close.
const MAX_UNCOMPRESSED: u64 = 2 << 30;
/// The blocks-info is a small table; anything larger is corrupt.
const MAX_BLOCKS_INFO: u32 = 64 << 20;

/// The fixed fields at the start of a UnityFS Bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleHeader {
    /// Always `UnityFS`.
    pub signature: String,
    /// The archive format version (8 for SWGOH).
    pub format_version: u32,
    /// The player version string, usually `5.x.x`.
    pub player_version: String,
    /// The Unity Version that built the Bundle, such as `6000.3.17f1`.
    pub unity_version: String,
    /// The total file size the header declares.
    pub size: u64,
    /// Archive flags (`0x243` for SWGOH).
    pub flags: u32,
    /// Compressed size of the blocks-info.
    pub blocks_info_compressed_size: u32,
    /// Uncompressed size of the blocks-info.
    pub blocks_info_uncompressed_size: u32,
}

impl BundleHeader {
    /// How the blocks-info is compressed.
    #[must_use]
    pub fn blocks_info_compression(&self) -> Compression {
        Compression::from_flags(self.flags)
    }
}

/// One compressed storage block of the Bundle's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    /// Size after decompression.
    pub uncompressed_size: u32,
    /// Size in the file.
    pub compressed_size: u32,
    /// Block flags; the low 6 bits are the compression.
    pub flags: u16,
}

impl BlockInfo {
    /// How this block is compressed.
    #[must_use]
    pub fn compression(&self) -> Compression {
        Compression::from_flags(u32::from(self.flags))
    }
}

/// One file inside the Bundle, such as `CAB-<md5>` or `CAB-<md5>.resS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The node's path inside the archive.
    pub path: String,
    /// Offset into the Bundle's uncompressed data.
    pub offset: u64,
    /// Length in bytes.
    pub size: u64,
    /// Node flags; `0x4` marks a SerializedFile.
    pub flags: u32,
}

impl Node {
    /// Whether this node is a SerializedFile (as opposed to a resource such
    /// as `.resS` or `.resource`).
    #[must_use]
    pub fn is_serialized_file(&self) -> bool {
        self.flags & NODE_FLAG_SERIALIZED != 0
    }

    /// The node's file name: the part of its path after the last `/`.
    #[must_use]
    pub fn file_name(&self) -> &str {
        file_name(&self.path)
    }
}

pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The parsed directory of a Bundle, readable from a prefix of the file.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    pub header: BundleHeader,
    pub blocks: Vec<BlockInfo>,
    pub nodes: Vec<Node>,
    /// File offset where the first block's compressed bytes begin.
    pub data_start: u64,
}

impl Layout {
    pub(crate) fn uncompressed_len(&self) -> u64 {
        self.blocks
            .iter()
            .map(|b| u64::from(b.uncompressed_size))
            .sum()
    }

    pub(crate) fn compressed_len(&self) -> u64 {
        self.blocks
            .iter()
            .map(|b| u64::from(b.compressed_size))
            .sum()
    }
}

/// Parses the header and blocks-info from `bytes`, which may be a prefix of a
/// file `total_len` bytes long. A `Short` error says how long the prefix must be.
pub(crate) fn parse_layout(bytes: &[u8], total_len: u64) -> ReadResult<Layout> {
    const SIGNATURE: &[u8] = b"UnityFS\0";
    if bytes.len() < SIGNATURE.len() {
        if !SIGNATURE.starts_with(bytes) {
            return Err(not_unityfs(bytes));
        }
        return Err(ReadError::Short {
            needed: SIGNATURE.len() as u64,
        });
    }
    if !bytes.starts_with(SIGNATURE) {
        return Err(not_unityfs(bytes));
    }

    let mut r = Reader::new(bytes, true);
    let signature = r.cstr()?;
    let format_version = r.u32()?;
    if !(6..=8).contains(&format_version) {
        return Err(Error::Unsupported(format!("UnityFS format version {format_version}")).into());
    }
    let player_version = r.cstr()?;
    let unity_version = r.cstr()?;
    let size = u64::try_from(r.i64()?)
        .map_err(|_| Error::malformed("Bundle header", "negative file size"))?;
    let blocks_info_compressed_size = r.u32()?;
    let blocks_info_uncompressed_size = r.u32()?;
    let flags = r.u32()?;
    let header = BundleHeader {
        signature,
        format_version,
        player_version,
        unity_version,
        size,
        flags,
        blocks_info_compressed_size,
        blocks_info_uncompressed_size,
    };

    if flags & FLAG_ENCRYPTED != 0 {
        return Err(Error::Unsupported("encrypted Bundles".into()).into());
    }
    if blocks_info_uncompressed_size > MAX_BLOCKS_INFO {
        return Err(Error::malformed("Bundle header", "blocks-info is implausibly large").into());
    }
    if format_version >= 7 {
        r.align(16);
    }

    let info_len = blocks_info_compressed_size as usize;
    let info_start = if flags & FLAG_INFO_AT_END != 0 {
        usize::try_from(total_len)
            .ok()
            .and_then(|t| t.checked_sub(info_len))
            .ok_or_else(|| Error::malformed("Bundle header", "blocks-info larger than file"))?
    } else {
        let start = r.pos();
        r.skip(info_len)?;
        start
    };
    let mut info_reader = Reader::at(bytes, info_start, true);
    let compressed_info = info_reader.bytes(info_len)?;

    let mut info = vec![0; blocks_info_uncompressed_size as usize];
    decompress_block(header.blocks_info_compression(), compressed_info, &mut info).map_err(
        |e| match e {
            Error::Truncated { .. } => Error::Truncated {
                context: "blocks-info",
            },
            other => other,
        },
    )?;
    let (blocks, nodes) = parse_blocks_info(&info).map_err(|e| e.into_error("blocks-info"))?;

    if flags & FLAG_COMBINED == 0 && flags & FLAG_INFO_AT_END == 0 {
        // Old layouts kept the directory separately; SWGOH never does (ADR-0003).
        return Err(Error::Unsupported("UnityFS without combined blocks-info".into()).into());
    }
    if flags & FLAG_PADDING_AT_START != 0 {
        r.align(16);
    }
    let layout = Layout {
        header,
        blocks,
        nodes,
        data_start: r.pos() as u64,
    };
    validate(&layout, total_len)?;
    Ok(layout)
}

/// Checks the directory against the file and the decompressed data.
fn validate(layout: &Layout, total_len: u64) -> ReadResult<()> {
    let uncompressed = layout.uncompressed_len();
    if uncompressed > MAX_UNCOMPRESSED {
        return Err(
            Error::Unsupported(format!("Bundle of {uncompressed} uncompressed bytes")).into(),
        );
    }
    if let Some(node) = layout.nodes.iter().find(|n| {
        n.offset
            .checked_add(n.size)
            .is_none_or(|end| end > uncompressed)
    }) {
        return Err(Error::malformed(
            "Bundle directory",
            format!("node {} extends past the data", node.path),
        )
        .into());
    }
    if layout.data_start + layout.compressed_len() > total_len {
        return Err(
            Error::malformed("Bundle directory", "blocks extend past the end of the file").into(),
        );
    }
    if let Some(block) = layout
        .blocks
        .iter()
        .find(|b| b.flags & BLOCK_FLAG_ENCRYPTED != 0)
    {
        return Err(
            Error::Unsupported(format!("encrypted block (flags {:#x})", block.flags)).into(),
        );
    }
    Ok(())
}

fn not_unityfs(bytes: &[u8]) -> ReadError {
    let known = [&b"UnityWeb"[..], b"UnityRaw", b"UnityArchive"];
    if known.iter().any(|sig| bytes.starts_with(sig)) {
        Error::Unsupported("pre-UnityFS Bundle formats".into()).into()
    } else {
        Error::malformed("Bundle header", "not a UnityFS Bundle").into()
    }
}

fn parse_blocks_info(info: &[u8]) -> ReadResult<(Vec<BlockInfo>, Vec<Node>)> {
    let mut r = Reader::new(info, true);
    r.skip(16)?; // hash of the uncompressed data
    let block_count = count(r.i32()?, r.remaining(), 10)?;
    let blocks = (0..block_count)
        .map(|_| {
            Ok(BlockInfo {
                uncompressed_size: r.u32()?,
                compressed_size: r.u32()?,
                flags: r.u16()?,
            })
        })
        .collect::<ReadResult<Vec<_>>>()?;
    let node_count = count(r.i32()?, r.remaining(), 21)?;
    let nodes = (0..node_count)
        .map(|_| {
            let offset = r.i64()?;
            let size = r.i64()?;
            let flags = r.u32()?;
            let path = r.cstr()?;
            let (Ok(offset), Ok(size)) = (u64::try_from(offset), u64::try_from(size)) else {
                return Err(
                    Error::malformed("Bundle directory", "negative node offset or size").into(),
                );
            };
            Ok(Node {
                path,
                offset,
                size,
                flags,
            })
        })
        .collect::<ReadResult<Vec<_>>>()?;
    Ok((blocks, nodes))
}

/// Validates an element count against the bytes left, given each element's
/// minimum encoded size.
fn count(n: i32, remaining: usize, min_size: usize) -> ReadResult<usize> {
    let n =
        usize::try_from(n).map_err(|_| Error::malformed("Bundle directory", "negative count"))?;
    if n.saturating_mul(min_size) > remaining {
        return Err(Error::malformed("Bundle directory", "count exceeds the data").into());
    }
    Ok(n)
}

/// A parsed, fully decompressed Bundle.
#[derive(Debug, Clone)]
pub struct Bundle {
    header: BundleHeader,
    blocks: Vec<BlockInfo>,
    nodes: Vec<Node>,
    data: Vec<u8>,
}

impl Bundle {
    /// Parses a complete Bundle file and decompresses all its blocks.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let layout =
            parse_layout(bytes, bytes.len() as u64).map_err(|e| e.into_error("Bundle header"))?;
        let total = usize::try_from(layout.uncompressed_len())
            .map_err(|_| Error::Unsupported("Bundle too large for this platform".into()))?;
        let mut data = vec![0; total];
        let mut in_pos = usize::try_from(layout.data_start).unwrap_or(usize::MAX);
        let mut out_pos = 0;
        for block in &layout.blocks {
            let in_len = block.compressed_size as usize;
            let out_len = block.uncompressed_size as usize;
            let input = in_pos
                .checked_add(in_len)
                .and_then(|end| bytes.get(in_pos..end))
                .ok_or(Error::Truncated {
                    context: "Bundle block data",
                })?;
            decompress_block(
                block.compression(),
                input,
                &mut data[out_pos..out_pos + out_len],
            )?;
            in_pos += in_len;
            out_pos += out_len;
        }
        Ok(Self {
            header: layout.header,
            blocks: layout.blocks,
            nodes: layout.nodes,
            data,
        })
    }

    /// The Bundle's header.
    #[must_use]
    pub fn header(&self) -> &BundleHeader {
        &self.header
    }

    /// The Unity Version that built the Bundle.
    #[must_use]
    pub fn unity_version(&self) -> &str {
        &self.header.unity_version
    }

    /// The storage blocks.
    #[must_use]
    pub fn blocks(&self) -> &[BlockInfo] {
        &self.blocks
    }

    /// All nodes, in directory order.
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// The node with exactly this path.
    #[must_use]
    pub fn node(&self, path: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.path == path)
    }

    /// The bytes of a node of this Bundle. Bounds of this Bundle's nodes were
    /// checked at parse time; a `Node` from elsewhere that does not fit
    /// yields an empty slice.
    #[must_use]
    pub fn node_data(&self, node: &Node) -> &[u8] {
        usize::try_from(node.offset)
            .ok()
            .zip(usize::try_from(node.size).ok())
            .and_then(|(start, len)| self.data.get(start..start.checked_add(len)?))
            .unwrap_or_default()
    }

    /// The SerializedFile nodes (normally exactly one, `CAB-<md5>`).
    pub fn serialized_nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|n| n.is_serialized_file())
    }

    /// The resource nodes, such as `CAB-<md5>.resS`.
    pub fn resource_nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|n| !n.is_serialized_file())
    }

    /// Parses the first SerializedFile node.
    pub fn serialized_file(&self) -> Result<SerializedFile<'_>> {
        let node = self
            .serialized_nodes()
            .next()
            .ok_or_else(|| Error::NodeNotFound("a SerializedFile node".into()))?;
        SerializedFile::parse(self.node_data(node))
    }

    /// Parses every SerializedFile node, paired with its node.
    pub fn serialized_files(&self) -> Result<Vec<(&Node, SerializedFile<'_>)>> {
        self.serialized_nodes()
            .map(|n| Ok((n, SerializedFile::parse(self.node_data(n))?)))
            .collect()
    }

    /// Resolves streamed data: `path` as stored in a StreamingInfo (for
    /// example `archive:/CAB-x/CAB-x.resS`), plus an offset and size within
    /// that node.
    pub fn stream_data(&self, path: &str, offset: u64, size: u64) -> Result<&[u8]> {
        let wanted = path.strip_prefix("archive:/").unwrap_or(path);
        let node = self
            .nodes
            .iter()
            .find(|n| n.path == wanted)
            .or_else(|| {
                let name = file_name(wanted);
                self.nodes.iter().find(|n| n.file_name() == name)
            })
            .ok_or_else(|| Error::NodeNotFound(path.to_owned()))?;
        if offset.checked_add(size).is_none_or(|end| end > node.size) {
            return Err(Error::malformed(
                "StreamingInfo",
                format!(
                    "range {offset}+{size} exceeds node {} of {} bytes",
                    node.path, node.size
                ),
            ));
        }
        let data = self.node_data(node);
        // In range: checked against node.size above.
        #[allow(clippy::cast_possible_truncation)]
        Ok(&data[offset as usize..(offset + size) as usize])
    }

    /// Resolves a [`StreamingInfo`] to its bytes in this Bundle.
    pub fn resolve_stream(&self, info: &StreamingInfo) -> Result<&[u8]> {
        self.stream_data(&info.path, info.offset, info.size)
    }
}
