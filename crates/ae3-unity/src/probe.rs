//! Prefix probing for the Asset Index: list a Bundle's Assets (path id, type
//! and name) from the first few kilobytes of the file, without downloading or
//! decompressing the rest.
//!
//! A SWGOH Bundle is a header, an LZ4HC blocks-info and one LZMA block holding
//! the SerializedFile followed by the `.resS` node. The SerializedFile's
//! metadata and the start of each object (where `m_Name` sits) come first in
//! the LZMA stream, so the decoder can stop long before the end of the file.

use lzma_rust2::{Action, LzmaStream, Status};

use crate::bundle::{BlockInfo, Node, parse_layout};
use crate::decompress::{Compression, LZMA_PROPS_LEN, decompress_block, lzma_error, lzma_stream};
use crate::error::{Error, ReadError, Result};
use crate::reader::Reader;
use crate::serialized::{HEADER_LEN_V22, ObjectInfo, metadata_header, parse_metadata};
use crate::typetree::{has_field, read_field};
use crate::value::Value;

/// A reasonable first prefix length to fetch: enough for most SWGOH Bundles
/// whose SerializedFile is small (portraits, single atlases).
pub const SUGGESTED_FIRST_PROBE_LEN: u64 = 8 * 1024;

/// Compressed bytes handed to the LZMA decoder at a time. Smaller steps make
/// [`ProbeListing::bytes_used`] tighter at a small cost in calls.
const FEED_STEP: usize = 256;
/// The least margin added to an estimate of the compressed bytes missing.
/// Estimates of a few hundred bytes are often short, and a request costs
/// far more than a few kilobytes.
const MIN_MARGIN: u128 = 4 * 1024;
/// Output buffer growth per decode call.
const OUT_CHUNK: usize = 64 * 1024;

/// One Asset found by probing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedAsset {
    /// The object's path id.
    pub path_id: i64,
    /// Unity class id.
    pub class_id: i32,
    /// The type's name, such as `Texture2D`.
    pub type_name: String,
    /// `m_Name`; `None` when the type has none or it is empty.
    pub name: Option<String>,
}

/// The result of a successful probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeListing {
    /// The Unity Version from the Bundle header.
    pub unity_version: String,
    /// Every object of every SerializedFile node, in file order.
    pub assets: Vec<ProbedAsset>,
    /// How many prefix bytes the listing needed (within a few hundred bytes).
    pub bytes_used: u64,
}

/// The outcome of [`probe_prefix`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The prefix was long enough.
    Listed(ProbeListing),
    /// Probe again with a longer prefix. `at_least` is an estimate of the
    /// prefix length that will suffice; it is always longer than the prefix
    /// passed in and never longer than the file.
    NeedMore {
        /// Suggested prefix length for the next attempt.
        at_least: u64,
    },
}

/// Lists the Assets of a Bundle from a prefix of its bytes.
///
/// `prefix` is the first bytes of a Bundle file that is `total_len` bytes long.
/// Only the header, blocks-info and the beginning of the decompressed stream
/// are decoded; the `.resS` node is never needed.
///
/// Every object's `m_Name` is read. [`probe_prefix_filtered`] reads only the
/// names the caller wants, which can stop earlier.
pub fn probe_prefix(prefix: &[u8], total_len: u64) -> Result<Probe> {
    probe_prefix_filtered(prefix, total_len, &|_| true)
}

/// [`probe_prefix`], reading `m_Name` only of the objects for which
/// `wants_name` returns true; the others are listed with `name: None`.
///
/// Every object is still listed (path id and type come from the metadata),
/// but the decompressed stream only has to reach the end of the last wanted
/// name, so a probe that skips the names of scene plumbing (`GameObject`,
/// `Transform`) can stop before the objects that follow the last wanted
/// one. Objects lie in path-id order rather than grouped by type, so how
/// much this saves depends on where the last wanted object falls.
pub fn probe_prefix_filtered(
    prefix: &[u8],
    total_len: u64,
    wants_name: &dyn Fn(&ObjectInfo) -> bool,
) -> Result<Probe> {
    let prefix_len = prefix.len() as u64;
    if prefix_len > total_len {
        return Err(Error::malformed("probe", "prefix is longer than the file"));
    }
    let layout = match parse_layout(prefix, total_len) {
        Ok(layout) => layout,
        Err(ReadError::Short { needed }) => {
            // The header and blocks-info are tiny; ask for them plus a start
            // on the data so the next round usually finishes.
            return need_more(prefix_len, total_len, needed.saturating_add(2048));
        }
        Err(ReadError::Bad(e)) => return Err(e),
    };

    let nodes: Vec<&Node> = layout
        .nodes
        .iter()
        .filter(|n| n.is_serialized_file())
        .collect();
    let data = prefix
        .get(to_usize(layout.data_start)..)
        .unwrap_or_default();
    let mut inflater = Inflater::new(&layout.blocks, data);
    let mut parsed: Vec<Option<Parsed>> = vec![None; nodes.len()];

    loop {
        match analyze(&nodes, &inflater.out, &mut parsed, wants_name)? {
            Analysis::Done(assets) => {
                return Ok(Probe::Listed(ProbeListing {
                    unity_version: layout.header.unity_version.clone(),
                    assets,
                    bytes_used: (layout.data_start + inflater.consumed()).min(prefix_len),
                }));
            }
            Analysis::Need(want) => {
                inflater.fill(to_usize(want))?;
                if (inflater.out.len() as u64) < want {
                    if prefix_len >= total_len {
                        return Err(Error::Truncated {
                            context: "Bundle data",
                        });
                    }
                    let available = prefix_len.saturating_sub(layout.data_start);
                    let totals = Totals {
                        compressed: layout.compressed_len(),
                        uncompressed: layout.uncompressed_len(),
                        resources: layout
                            .nodes
                            .iter()
                            .filter(|n| !n.is_serialized_file() && n.offset >= want)
                            .map(|n| n.size)
                            .sum(),
                    };
                    let decoded = Decoded {
                        available,
                        consumed: inflater.consumed(),
                        produced: inflater.out.len() as u64,
                        recent: inflater.recent(),
                    };
                    let estimate = estimate_compressed(decoded, want, totals);
                    return need_more(prefix_len, total_len, layout.data_start + estimate);
                }
            }
        }
    }
}

fn to_usize(x: u64) -> usize {
    usize::try_from(x).unwrap_or(usize::MAX)
}

fn need_more(prefix_len: u64, total_len: u64, at_least: u64) -> Result<Probe> {
    if prefix_len >= total_len {
        return Err(Error::Truncated { context: "Bundle" });
    }
    Ok(Probe::NeedMore {
        at_least: at_least.max(prefix_len + 1).min(total_len),
    })
}

/// Compressed and uncompressed totals of the Bundle's block data.
#[derive(Debug, Clone, Copy)]
struct Totals {
    compressed: u64,
    uncompressed: u64,
    /// Uncompressed bytes of resource nodes (`.resS`, `.resource`) that lie
    /// wholly past the wanted bytes.
    resources: u64,
}

/// How far decoding got, in compressed bytes from the start of the block
/// data and decompressed bytes.
#[derive(Debug, Clone, Copy)]
struct Decoded {
    /// Compressed bytes in the prefix.
    available: u64,
    /// Of those, the bytes the decoder consumed.
    consumed: u64,
    produced: u64,
    /// `(consumed, produced)` when about half of `produced` was decoded.
    recent: (u64, u64),
}

/// Estimates how many compressed bytes yield `want` decompressed bytes.
///
/// The bytes still missing are estimated at two ratios, of which the larger
/// wins, plus a margin of 25% of it (at least [`MIN_MARGIN`]):
/// - the ratio of the most recent half of the decoded output, which is
///   object data once the metadata is behind (the metadata, mostly type
///   trees, compresses far better and would underestimate);
/// - the average ratio of the rest of the SerializedFile data, counting the
///   resource nodes past `want` as incompressible (their texture data nearly
///   is), so that they do not inflate it.
fn estimate_compressed(decoded: Decoded, want: u64, totals: Totals) -> u64 {
    let c = u128::from(decoded.consumed);
    let p = u128::from(decoded.produced);
    let missing = u128::from(want).saturating_sub(p);
    let (c1, p1) = (u128::from(decoded.recent.0), u128::from(decoded.recent.1));
    let recent = match (c.saturating_sub(c1), p.saturating_sub(p1)) {
        (dc, dp) if dc > 0 && dp > 0 => missing * dc / dp,
        _ if c > 0 && p > 0 => missing * c / p,
        _ => missing / 2,
    };
    let resources = u128::from(totals.resources);
    let rest_c = u128::from(totals.compressed)
        .saturating_sub(c)
        .saturating_sub(resources);
    let rest_u = u128::from(totals.uncompressed)
        .saturating_sub(p)
        .saturating_sub(resources);
    let rest = (missing * rest_c).checked_div(rest_u).unwrap_or(0);
    let extra = recent.max(rest);
    let start = u128::from(decoded.available.max(decoded.consumed));
    u64::try_from(start + extra + (extra / 4).max(MIN_MARGIN)).unwrap_or(u64::MAX)
}

/// Metadata of one SerializedFile node, parsed once and reused across rounds.
#[derive(Clone)]
struct Parsed {
    inner: rabex::files::SerializedFile,
    objects: Vec<ObjectInfo>,
    big_endian: bool,
}

enum Analysis {
    Done(Vec<ProbedAsset>),
    /// The decompressed stream must reach this many bytes.
    Need(u64),
}

fn analyze(
    nodes: &[&Node],
    out: &[u8],
    parsed: &mut [Option<Parsed>],
    wants_name: &dyn Fn(&ObjectInfo) -> bool,
) -> Result<Analysis> {
    let have = out.len() as u64;
    let mut need = 0u64;
    let mut assets = Vec::new();
    for (node, slot) in nodes.iter().zip(parsed.iter_mut()) {
        let start = node.offset;
        let end = start + node.size;
        let avail = out
            .get(to_usize(start)..to_usize(end.min(have)))
            .unwrap_or_default();

        if slot.is_none() {
            let header = match metadata_header(avail) {
                Ok(h) => h,
                Err(Error::Truncated { .. }) if end > have => {
                    need = need.max(start + HEADER_LEN_V22);
                    continue;
                }
                Err(e) => return Err(e),
            };
            if header.metadata_end > node.size {
                return Err(Error::malformed(
                    "SerializedFile header",
                    "metadata larger than the node",
                ));
            }
            if (avail.len() as u64) < header.metadata_end {
                need = need.max(start + header.metadata_end);
                continue;
            }
            let (inner, objects) = parse_metadata(&avail[..to_usize(header.metadata_end)])?;
            let big_endian = matches!(
                inner.m_Header.m_Endianess,
                rabex::files::serializedfile::Endianness::Big
            );
            *slot = Some(Parsed {
                inner,
                objects,
                big_endian,
            });
        }
        let Some(p) = slot.as_ref() else { continue };

        for object in &p.objects {
            let tree = p
                .inner
                .m_Types
                .get(object.type_index)
                .and_then(|t| t.m_Type.as_ref());
            let mut name = None;
            if let Some(tree) = tree.filter(|t| wants_name(object) && has_field(t, "m_Name")) {
                let obj_start = object.offset;
                let obj_end = obj_start
                    .saturating_add(u64::from(object.size))
                    .min(node.size);
                let slice = avail
                    .get(to_usize(obj_start)..to_usize(obj_end).min(avail.len()))
                    .unwrap_or_default();
                let mut r = Reader::new(slice, p.big_endian);
                match read_field(tree, &mut r, "m_Name") {
                    Ok(Some(Value::String(s))) if !s.is_empty() => name = Some(s),
                    Ok(_) | Err(ReadError::Bad(_)) => {}
                    Err(ReadError::Short { needed }) => {
                        let wanted = start + (obj_start + needed).min(obj_end);
                        // A complete object that still reads short is corrupt;
                        // list it without a name rather than fail the Bundle.
                        if wanted > have {
                            need = need.max(wanted);
                        }
                    }
                }
            }
            assets.push(ProbedAsset {
                path_id: object.path_id,
                class_id: object.class_id,
                type_name: object.type_name.clone(),
                name,
            });
        }
    }
    Ok(if need > 0 {
        Analysis::Need(need)
    } else {
        Analysis::Done(assets)
    })
}

/// Incrementally decompresses the Bundle's blocks from the available prefix.
struct Inflater<'a> {
    blocks: &'a [BlockInfo],
    /// The prefix from the start of the block data.
    input: &'a [u8],
    /// Index of the block being decoded.
    block: usize,
    /// Offset of that block's compressed bytes in `input`.
    block_in: usize,
    /// Compressed bytes of that block consumed so far.
    fed: usize,
    /// Offset of that block's output in `out`.
    block_out: usize,
    lzma: Option<LzmaStream>,
    out: Vec<u8>,
    /// `(consumed, produced)` after each step of at least [`MARK_STEP`]
    /// output bytes, for [`Inflater::recent`].
    marks: Vec<(u64, u64)>,
}

/// Output bytes between the progress marks the estimator looks back at.
const MARK_STEP: u64 = 4 * 1024;

/// Notes `(consumed, produced)` once output has grown by [`MARK_STEP`].
fn mark(marks: &mut Vec<(u64, u64)>, consumed: u64, produced: u64) {
    if marks.last().is_none_or(|&(_, p)| produced >= p + MARK_STEP) {
        marks.push((consumed, produced));
    }
}

impl<'a> Inflater<'a> {
    fn new(blocks: &'a [BlockInfo], input: &'a [u8]) -> Self {
        Self {
            blocks,
            input,
            block: 0,
            block_in: 0,
            fed: 0,
            block_out: 0,
            lzma: None,
            out: Vec::new(),
            marks: vec![(0, 0)],
        }
    }

    /// Compressed bytes consumed, from the start of the block data.
    fn consumed(&self) -> u64 {
        (self.block_in + self.fed) as u64
    }

    /// `(consumed, produced)` at the last mark at or before half the output.
    fn recent(&self) -> (u64, u64) {
        let half = self.out.len() as u64 / 2;
        let at = self.marks.partition_point(|&(_, p)| p <= half);
        self.marks[at.saturating_sub(1)]
    }

    fn next_block(&mut self, block: BlockInfo) {
        self.block += 1;
        self.block_in += block.compressed_size as usize;
        self.block_out += block.uncompressed_size as usize;
        self.fed = 0;
        self.lzma = None;
    }

    /// Decodes until `out` holds `want` bytes or the input runs out.
    fn fill(&mut self, want: usize) -> Result<()> {
        while self.out.len() < want {
            let Some(&block) = self.blocks.get(self.block) else {
                return Ok(());
            };
            let clen = block.compressed_size as usize;
            let ulen = block.uncompressed_size as usize;
            let Some(avail) = self
                .input
                .get(self.block_in..(self.block_in + clen).min(self.input.len()))
            else {
                return Ok(());
            };
            let finished = match block.compression() {
                Compression::None => {
                    let done = self.out.len() - self.block_out;
                    let take = avail.len().min(ulen).saturating_sub(done);
                    self.out.extend_from_slice(&avail[done..done + take]);
                    self.fed = done + take;
                    self.out.len() - self.block_out == ulen
                }
                Compression::Lz4 | Compression::Lz4Hc => {
                    if avail.len() < clen {
                        return Ok(());
                    }
                    self.out.resize(self.block_out + ulen, 0);
                    decompress_block(block.compression(), avail, &mut self.out[self.block_out..])?;
                    true
                }
                Compression::Lzma => self.fill_lzma(avail, ulen, want)?,
                other => {
                    return Err(Error::Unsupported(format!(
                        "{} block compression",
                        other.name()
                    )));
                }
            };
            let (consumed, produced) = (self.consumed(), self.out.len() as u64);
            mark(&mut self.marks, consumed, produced);
            if finished {
                self.next_block(block);
            } else {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Feeds one LZMA block; returns whether the block is complete.
    fn fill_lzma(&mut self, avail: &[u8], ulen: usize, want: usize) -> Result<bool> {
        if self.lzma.is_none() {
            let Some(props) = avail.get(..LZMA_PROPS_LEN) else {
                return Ok(false);
            };
            self.lzma = Some(lzma_stream(props, ulen as u64)?);
            self.fed = LZMA_PROPS_LEN;
        }
        let Some(stream) = self.lzma.as_mut() else {
            return Ok(false);
        };
        loop {
            let done = self.out.len() - self.block_out;
            if done == ulen {
                return Ok(true);
            }
            if self.out.len() >= want {
                return Ok(false);
            }
            let input = &avail[self.fed..(self.fed + FEED_STEP).min(avail.len())];
            let start = self.out.len();
            self.out.resize(start + (ulen - done).min(OUT_CHUNK), 0);
            let result = stream.process(input, &mut self.out[start..], Action::Run);
            let r = match result {
                Ok(r) => r,
                Err(e) => {
                    self.out.truncate(start);
                    return Err(lzma_error(&e));
                }
            };
            self.out.truncate(start + r.bytes_produced);
            self.fed += r.bytes_consumed;
            mark(
                &mut self.marks,
                (self.block_in + self.fed) as u64,
                self.out.len() as u64,
            );
            if r.status == Status::StreamEnd {
                return if self.out.len() - self.block_out == ulen {
                    Ok(true)
                } else {
                    Err(Error::Truncated {
                        context: "LZMA block",
                    })
                };
            }
            if r.bytes_consumed == 0 && r.bytes_produced == 0 {
                // Out of input (or the decoder needs more to make progress).
                return Ok(false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(consumed: u64, produced: u64, recent: (u64, u64)) -> Decoded {
        Decoded {
            available: consumed,
            consumed,
            produced,
            recent,
        }
    }

    #[test]
    fn estimate_scales_with_ratio() {
        // 1:4 everywhere: the missing 400 KB need 100 KB more, plus 25%.
        let even = Totals {
            compressed: 1_000_000,
            uncompressed: 4_000_000,
            resources: 0,
        };
        let uniform = decoded(100_000, 400_000, (50_000, 200_000));
        assert_eq!(
            estimate_compressed(uniform, 800_000, even),
            100_000 + 125_000
        );
        // The rest of the data compresses 1:2: 200 KB more.
        let worse = Totals {
            compressed: 1_900_000,
            ..even
        };
        assert_eq!(
            estimate_compressed(uniform, 800_000, worse),
            100_000 + 250_000
        );
        // The latest output compressed 1:2 although the whole did 1:4 (the
        // metadata first compresses far better): 200 KB more.
        let metadata_first = decoded(100_000, 400_000, (50_000, 300_000));
        assert_eq!(
            estimate_compressed(metadata_first, 800_000, even),
            100_000 + 250_000
        );
        // Bytes in the prefix the decoder has not consumed yet count.
        let unconsumed = Decoded {
            available: 120_000,
            ..uniform
        };
        assert_eq!(
            estimate_compressed(unconsumed, 800_000, even),
            120_000 + 125_000
        );
        // Small estimates get the least margin.
        assert_eq!(
            estimate_compressed(decoded(0, 0, (0, 0)), 1000, even),
            500 + 4096
        );
    }

    #[test]
    fn estimate_counts_trailing_resources_as_incompressible() {
        // 2 MB of `.resS` past the wanted bytes, stored nearly as is: the
        // SerializedFile's rest still compresses 1:4.
        let with_resources = Totals {
            compressed: 3_000_000,
            uncompressed: 6_000_000,
            resources: 2_000_000,
        };
        let uniform = decoded(100_000, 400_000, (50_000, 200_000));
        assert_eq!(
            estimate_compressed(uniform, 800_000, with_resources),
            100_000 + 125_000
        );
        let counted = Totals {
            resources: 0,
            ..with_resources
        };
        assert!(estimate_compressed(uniform, 800_000, counted) > 100_000 + 250_000);
    }

    #[test]
    fn filtered_probes_skip_unwanted_names() {
        let root = std::env::var_os("AE3_FIXTURES").map_or_else(
            || std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            std::path::PathBuf::from,
        );
        let Ok(bytes) = std::fs::read(root.join("100054/Windows/char_bossk_pre.bundle")) else {
            eprintln!("skipping: fixture char_bossk_pre.bundle not found");
            return;
        };
        // A probe that wants no names lists the same objects, all unnamed.
        let total = bytes.len() as u64;
        let Ok(Probe::Listed(all)) = probe_prefix(&bytes, total) else {
            panic!("lists");
        };
        let Ok(Probe::Listed(none)) = probe_prefix_filtered(&bytes, total, &|_| false) else {
            panic!("lists");
        };
        assert_eq!(all.assets.len(), none.assets.len());
        assert!(all.assets.iter().any(|a| a.name.is_some()));
        assert!(none.assets.iter().all(|a| a.name.is_none()));
        assert!(
            none.bytes_used < all.bytes_used / 2,
            "stops after the metadata"
        );
        // Wanting Meshes only stops at the last Mesh name.
        let meshes = |o: &ObjectInfo| o.class_id == 43;
        let Ok(Probe::Listed(some)) = probe_prefix_filtered(&bytes, total, &meshes) else {
            panic!("lists");
        };
        for (a, s) in all.assets.iter().zip(&some.assets) {
            assert_eq!(
                s.name,
                if a.class_id == 43 {
                    a.name.clone()
                } else {
                    None
                }
            );
        }
        assert!(some.bytes_used <= all.bytes_used);
        // A prefix that ends after the last wanted name is enough.
        let short = &bytes[..usize::try_from(some.bytes_used + 256).unwrap()];
        assert!(matches!(
            probe_prefix_filtered(short, total, &meshes),
            Ok(Probe::Listed(_))
        ));
    }

    #[test]
    fn rejects_non_bundles() {
        assert!(probe_prefix(b"PK\x03\x04hello", 100).is_err());
        assert!(probe_prefix(b"UnityWeb\0", 100).is_err());
    }

    #[test]
    fn short_prefixes_ask_for_more() {
        for prefix in [&b""[..], b"Unity", b"UnityFS\0\0\0\0\x08"] {
            match probe_prefix(prefix, 10_000).unwrap() {
                Probe::NeedMore { at_least } => assert!(at_least > prefix.len() as u64),
                Probe::Listed(_) => panic!("listed from {} bytes", prefix.len()),
            }
        }
    }

    #[test]
    fn prefix_longer_than_file_is_an_error() {
        assert!(probe_prefix(b"UnityFS\0", 4).is_err());
    }
}
