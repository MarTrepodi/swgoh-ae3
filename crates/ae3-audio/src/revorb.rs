//! Our equivalent of `revorb`: re-pages an Ogg Vorbis stream and rewrites
//! every page's granule position from the packets' block sizes.
//!
//! ww2ogg writes one packet per page, and its granule positions are only as
//! good as the WEM's. A Vorbis decoder outputs no samples for the first audio
//! packet and `(previous_blocksize + current_blocksize) / 4` samples for each
//! later one, so the granule position after a packet is the running sum of
//! that. A packet's block size follows from its mode's block flag, which the
//! setup header defines. lewton validates the identification, comment and
//! setup headers; it does not expose the mode table, so [`ModeTable`] reads
//! the block flags from the end of the setup header itself.

use std::fmt::Display;
use std::io::Cursor;

use lewton::header::{read_header_comment, read_header_ident, read_header_setup};
use ogg::{PacketReader, PacketWriteEndInfo, PacketWriter};

use crate::error::{Error, Result};

/// Audio packets are gathered onto a page until it holds about this many
/// bytes, as revorb does, rather than paying a page header per packet.
const PAGE_TARGET: usize = 4096;

fn err(e: impl Display) -> Error {
    Error::Vorbis(format!("granule fix-up: {e}"))
}

/// The block flag of every mode in a Vorbis setup header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModeTable {
    long: Vec<bool>,
}

impl ModeTable {
    /// Reads the mode table from the tail of a setup header packet.
    ///
    /// The setup header ends with a 6-bit `mode_count - 1`, then per mode a
    /// block flag (1 bit), window type (16 bits, always 0), transform type
    /// (16 bits, always 0) and mapping (8 bits), then a framing bit of 1.
    /// Vorbis packs bits least-significant first, so walking back from the
    /// framing bit finds the modes without decoding the codebooks, floors and
    /// residues before them (the approach of the `FFmpeg` Vorbis parser). The
    /// largest mode count consistent with both the zero fields and the count
    /// field wins, since a smaller count can match by accident.
    pub(crate) fn from_setup(packet: &[u8]) -> Result<Self> {
        const MODE_BITS: usize = 1 + 16 + 16 + 8;
        let bit = |i: usize| (packet[i / 8] >> (i % 8)) & 1 == 1;
        let bits =
            |start: usize, n: usize| (0..n).fold(0u32, |v, k| v | u32::from(bit(start + k)) << k);
        let last_byte = packet
            .iter()
            .rposition(|&b| b != 0)
            .ok_or_else(|| err("setup header is empty"))?;
        let framing = last_byte * 8 + (7 - packet[last_byte].leading_zeros() as usize);
        // A mode is plausible while its window and transform types are zero.
        let plausible = (1..=64)
            .take_while(|&n| framing >= n * MODE_BITS && bits(framing - n * MODE_BITS + 1, 32) == 0)
            .count();
        let count = (1..=plausible)
            .rev()
            .find(|&n| {
                let at = framing - n * MODE_BITS;
                at >= 6 && bits(at - 6, 6) as usize == n - 1
            })
            .ok_or_else(|| err("setup header has no readable mode table"))?;
        let first = framing - count * MODE_BITS;
        Ok(Self {
            long: (0..count).map(|m| bit(first + m * MODE_BITS)).collect(),
        })
    }

    /// Number of bits an audio packet uses for its mode number.
    pub(crate) fn mode_bits(&self) -> u32 {
        (self.long.len() - 1).bit_width()
    }

    pub(crate) fn len(&self) -> usize {
        self.long.len()
    }

    /// Whether audio `packet` is a long block, or `None` when it is not an
    /// audio packet or names a mode the table lacks.
    fn is_long(&self, packet: &[u8]) -> Option<bool> {
        let first = u32::from(*packet.first()?) | u32::from(*packet.get(1).unwrap_or(&0)) << 8;
        if first & 1 != 0 {
            return None;
        }
        let mode = (first >> 1) & ((1 << self.mode_bits()) - 1);
        self.long.get(mode as usize).copied()
    }
}

/// Rewrites `ogg`, a single-stream Ogg Vorbis file, with granule positions
/// computed from block sizes and audio packets packed several to a page.
///
/// `sample_count`, when given, trims the final page to the true length if it
/// is shorter than the computed one, which Vorbis treats as end trimming.
pub(crate) fn fix_granules(ogg: &[u8], sample_count: Option<u64>) -> Result<Vec<u8>> {
    let mut reader = PacketReader::new(Cursor::new(ogg));
    let mut header = |what: &str| {
        reader
            .read_packet()
            .map_err(err)?
            .ok_or_else(|| err(format!("stream ends before the {what} header")))
    };
    let ident_packet = header("identification")?;
    let comment_packet = header("comment")?;
    let setup_packet = header("setup")?;
    let ident = read_header_ident(&ident_packet.data).map_err(err)?;
    read_header_comment(&comment_packet.data).map_err(err)?;
    read_header_setup(
        &setup_packet.data,
        ident.audio_channels,
        (ident.blocksize_0, ident.blocksize_1),
    )
    .map_err(err)?;
    let modes = ModeTable::from_setup(&setup_packet.data)?;
    let serial = ident_packet.stream_serial();

    let mut writer = PacketWriter::new(Vec::with_capacity(ogg.len()));
    let mut write = |data: Vec<u8>, end: PacketWriteEndInfo, granule: u64| {
        writer
            .write_packet(data.into_boxed_slice(), serial, end, granule)
            .map_err(err)
    };
    // The identification header sits alone on the first page, and audio
    // starts on a fresh page after the setup header.
    write(ident_packet.data, PacketWriteEndInfo::EndPage, 0)?;
    write(comment_packet.data, PacketWriteEndInfo::NormalPacket, 0)?;
    write(setup_packet.data, PacketWriteEndInfo::EndPage, 0)?;

    let short = 1u64 << ident.blocksize_0;
    let long = 1u64 << ident.blocksize_1;
    let mut previous_block = None;
    let mut granule = 0u64;
    let mut page_bytes = 0usize;
    // The newest packet is held back until the next one is read, so the last
    // packet can be written with the end-of-stream flag.
    let mut pending: Option<(Vec<u8>, u64)> = None;
    let mut granule_before_last = 0u64;
    while let Some(packet) = reader.read_packet().map_err(err)? {
        // An empty packet is legal and decodes to nothing.
        if !packet.data.is_empty() {
            let is_long = modes
                .is_long(&packet.data)
                .ok_or_else(|| err("audio packet with an invalid mode"))?;
            let block = if is_long { long } else { short };
            if let Some(previous) = previous_block {
                granule += (previous + block) / 4;
            }
            previous_block = Some(block);
        }
        if let Some((data, at)) = pending.take() {
            page_bytes += data.len();
            let end = if page_bytes >= PAGE_TARGET {
                page_bytes = 0;
                PacketWriteEndInfo::EndPage
            } else {
                PacketWriteEndInfo::NormalPacket
            };
            write(data, end, at)?;
            granule_before_last = at;
        }
        pending = Some((packet.data, granule));
    }
    let (data, computed) = pending.ok_or_else(|| err("stream has no audio packets"))?;
    let last = match sample_count {
        Some(count) if (granule_before_last..computed).contains(&count) => count,
        _ => computed,
    };
    write(data, PacketWriteEndInfo::EndStream, last)?;
    drop(write);
    Ok(writer.into_inner())
}

#[cfg(test)]
#[expect(clippy::cast_possible_truncation, reason = "test data is small")]
mod tests {
    use super::*;

    /// Packs `(value, bits)` fields least-significant bit first.
    fn pack(fields: &[(u32, usize)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut at = 0;
        for &(value, n) in fields {
            for k in 0..n {
                if at / 8 == out.len() {
                    out.push(0);
                }
                out[at / 8] |= u8::from((value >> k) & 1 == 1) << (at % 8);
                at += 1;
            }
        }
        out
    }

    fn setup_tail(prefix: &[(u32, usize)], modes: &[(bool, u32)]) -> Vec<u8> {
        let mut fields = prefix.to_vec();
        fields.push((modes.len() as u32 - 1, 6));
        for &(long, mapping) in modes {
            fields.extend([(u32::from(long), 1), (0, 16), (0, 16), (mapping, 8)]);
        }
        fields.push((1, 1));
        pack(&fields)
    }

    #[test]
    fn reads_mode_block_flags_from_the_setup_tail() {
        let packet = setup_tail(&[(0x5A5A, 16), (0x3, 3)], &[(false, 0), (true, 1)]);
        let modes = ModeTable::from_setup(&packet).unwrap();
        assert_eq!(modes.long, [false, true]);
        assert_eq!(modes.mode_bits(), 1);
        // Packet type 0, mode 1 (long), then anything.
        assert_eq!(modes.is_long(&[0b0000_0010]), Some(true));
        assert_eq!(modes.is_long(&[0b0000_0000]), Some(false));
        assert_eq!(modes.is_long(&[0b0000_0001]), None, "header packet");
        assert_eq!(modes.is_long(&[]), None);
    }

    #[test]
    fn prefers_the_largest_consistent_mode_count() {
        // With mappings below 4, a single-mode reading of the last entry also
        // has a zero count field; the full table must still win.
        let packet = setup_tail(&[(0xFFFF, 16)], &[(true, 0), (false, 1), (true, 2)]);
        let modes = ModeTable::from_setup(&packet).unwrap();
        assert_eq!(modes.long, [true, false, true]);
        assert_eq!(modes.mode_bits(), 2);
        // Bit 0 is the packet type; the mode number follows.
        assert_eq!(modes.is_long(&[0b0000_0100]), Some(true));
        assert_eq!(modes.is_long(&[0b1111_0010]), Some(false));
        // Mode 3 does not exist.
        assert_eq!(modes.is_long(&[0b0000_0110]), None);
    }

    #[test]
    fn rejects_setups_without_a_mode_table() {
        assert!(ModeTable::from_setup(&[]).is_err());
        assert!(ModeTable::from_setup(&[0, 0]).is_err());
        assert!(ModeTable::from_setup(&[0xFF; 3]).is_err());
    }

    #[test]
    fn rejects_streams_that_are_not_vorbis() {
        assert!(fix_granules(b"", None).is_err());
        assert!(fix_granules(b"OggS garbage", None).is_err());
    }
}
