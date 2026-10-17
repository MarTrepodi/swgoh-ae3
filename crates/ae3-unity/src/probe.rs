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
pub fn probe_prefix(prefix: &[u8], total_len: u64) -> Result<Probe> {
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
        match analyze(&nodes, &inflater.out, &mut parsed)? {
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
                    };
                    let estimate =
                        estimate_compressed(available, inflater.out.len() as u64, want, totals);
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
}

/// Estimates how many compressed bytes yield `want` decompressed bytes, given
/// that `consumed` bytes yielded `produced`.
///
/// Two estimates, of which the larger wins (plus a 25% margin): the ratio
/// observed so far, and the average ratio of the rest of the data. The
/// metadata decoded first compresses far better than the object data after
/// it, so the observed ratio alone underestimates badly for large
/// SerializedFiles.
fn estimate_compressed(consumed: u64, produced: u64, want: u64, totals: Totals) -> u64 {
    let (c, p, n) = (u128::from(consumed), u128::from(produced), u128::from(want));
    let observed = if c > 0 && p > 0 { c * n / p } else { n / 2 };
    let rest_c = u128::from(totals.compressed).saturating_sub(c);
    let rest_u = u128::from(totals.uncompressed).saturating_sub(p);
    let remaining = (n.saturating_sub(p) * rest_c)
        .checked_div(rest_u)
        .map_or(observed, |rest| c + rest);
    let base = observed.max(remaining);
    u64::try_from(base + base / 4 + 512).unwrap_or(u64::MAX)
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

fn analyze(nodes: &[&Node], out: &[u8], parsed: &mut [Option<Parsed>]) -> Result<Analysis> {
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
            if let Some(tree) = tree.filter(|t| has_field(t, "m_Name")) {
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
        }
    }

    /// Compressed bytes consumed, from the start of the block data.
    fn consumed(&self) -> u64 {
        (self.block_in + self.fed) as u64
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

    #[test]
    fn estimate_scales_with_ratio() {
        // Observed 1:4 everywhere: 2000 bytes.
        let even = Totals {
            compressed: 10_000,
            uncompressed: 40_000,
        };
        assert_eq!(
            estimate_compressed(1000, 4000, 8000, even),
            2000 + 500 + 512
        );
        // The rest compresses 1:2, so the remaining 4000 bytes need 2000 more.
        let worse = Totals {
            compressed: 19_000,
            uncompressed: 40_000,
        };
        assert_eq!(
            estimate_compressed(1000, 4000, 8000, worse),
            3000 + 750 + 512
        );
        assert!(estimate_compressed(0, 0, 1000, even) >= 500);
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
