//! Keeping corrupt WEMs from panicking the Vorbis dependencies.
//!
//! ww2ogg 0.1 indexes its mode table with the mode number of every "modified"
//! audio packet (the packet form of current Wwise versions) and computes
//! `channels - 1`, so a corrupt packet or a zero channel count panics instead
//! of returning an error. [`check`] rejects these inputs before ww2ogg sees
//! them. The mode table comes from a header-only conversion: a copy of the
//! WEM cut off before its first audio packet, which ww2ogg turns into just
//! the three Vorbis headers.
//!
//! What cannot be checked up front is caught: ww2ogg and lewton do unchecked
//! arithmetic on decoded values (lewton's floor curve, for one), which
//! overflow checks in debug builds turn into panics, and future releases may
//! panic in other places. Every call into them runs through [`contain`].
//! That only helps while panics unwind; AE3's release profile keeps
//! `panic = "unwind"` for this reason.

use std::any::Any;
use std::io::Cursor;
use std::panic::{self, AssertUnwindSafe};

use ogg::PacketReader;
use ww2ogg::{CodebookLibrary, WemResult, WwiseRiffVorbis};

use crate::bytes::{Endian, Reader};
use crate::revorb::ModeTable;
use crate::wem::WemInfo;

/// Runs `work`, turning a panic inside it into `Err` with the panic message.
/// `work` must not leave shared state half-updated; callers only pass
/// closures over their own locals.
pub(crate) fn contain<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(work)).map_err(|payload| message(&*payload))
}

fn message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with a non-string payload".to_owned())
}

/// Values of the vorb "mod signal" field that mean unmodified packets.
const UNMODIFIED_PACKETS: [u32; 4] = [0x4A, 0x4B, 0x69, 0x70];

/// Returns an error describing why ww2ogg must not convert `wem`, or `Ok`
/// when it can be handed over. `books` and `inline` are the codebook choice
/// of the coming conversion, since the mode table depends on them.
pub(crate) fn check(
    wem: &[u8],
    info: &WemInfo,
    books: fn() -> WemResult<CodebookLibrary>,
    inline: bool,
) -> Result<(), String> {
    if info.channels == 0 {
        return Err("WEM has zero channels".into());
    }
    // ww2ogg writes the nominal bitrate as `avg_bytes_per_second * 8`.
    let avg_bytes_per_second = Reader::at(wem, info.fmt.0 + 8, info.endian, "WEM header")
        .u32()
        .map_err(|e| e.to_string())?;
    if avg_bytes_per_second > u32::MAX / 8 {
        return Err(format!("implausible byte rate {avg_bytes_per_second}"));
    }
    // ww2ogg shifts by the block size exponents, which Vorbis limits to
    // 6..=13; they sit after the uid in the newer vorb layouts.
    let blocksizes = match info.vorb {
        Some((at, 0x2A)) => Some(at + 0x28),
        Some((at, 0x32 | 0x34)) => Some(at + 0x30),
        _ => None,
    };
    if let Some(at) = blocksizes {
        let pair = wem.get(at..at + 2).ok_or("vorb data is truncated")?;
        if !pair.iter().all(|b| (6..=13).contains(b)) || pair[0] > pair[1] {
            return Err(format!(
                "invalid block size exponents {} and {}",
                pair[0], pair[1]
            ));
        }
    }
    // Only the 0x2A-byte vorb layout uses modified packets with 2-byte
    // packet headers; ww2ogg copies the other layouts' packets verbatim.
    let Some((vorb_at, 0x2A)) = info.vorb else {
        return Ok(());
    };
    let (data_at, data_len) = info.data;
    let u32_at = |at| Reader::at(wem, at, info.endian, "WEM header").u32();
    let (Ok(mod_signal), Ok(first_audio)) = (u32_at(vorb_at + 4), u32_at(vorb_at + 0x14)) else {
        return Ok(()); // ww2ogg reports the short vorb data itself.
    };
    if UNMODIFIED_PACKETS.contains(&mod_signal) {
        return Ok(());
    }
    let first_audio = first_audio as usize;
    let head_len = data_at.saturating_add(first_audio);
    if vorb_at > data_at || head_len > data_at + data_len {
        // The cut-off copy would lose the vorb data, or the audio offset is
        // past the data, which ww2ogg rejects itself.
        return Ok(());
    }
    let modes = header_modes(&wem[..head_len], info, first_audio, books, inline)?;
    let mask = (1u32 << modes.mode_bits()) - 1;
    let end = data_at + data_len;
    let mut offset = data_at + first_audio;
    while offset + 2 <= end {
        let size = Reader::at(wem, offset, info.endian, "WEM audio packet")
            .u16()
            .map_err(|e| e.to_string())?;
        let payload = offset + 2;
        // ww2ogg reads the mode bits even from an empty packet.
        if let Some(&first) = wem.get(payload) {
            let mode = u32::from(first) & mask;
            if mode as usize >= modes.len() {
                return Err(format!(
                    "audio packet at byte {offset} uses mode {mode} of {}",
                    modes.len()
                ));
            }
        }
        offset = payload + usize::from(size);
    }
    Ok(())
}

/// Converts the WEM head `head` (everything before the first audio packet)
/// and reads the mode table from the resulting setup header.
fn header_modes(
    head: &[u8],
    info: &WemInfo,
    first_audio: usize,
    books: fn() -> WemResult<CodebookLibrary>,
    inline: bool,
) -> Result<ModeTable, String> {
    let text = |e: ww2ogg::WemError| e.to_string();
    let mut head = head.to_vec();
    let put = |buf: &mut Vec<u8>, at: usize, value: usize| {
        let value = u32::try_from(value).unwrap_or(u32::MAX);
        let bytes = match info.endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        buf[at..at + 4].copy_from_slice(&bytes);
    };
    let riff_len = head.len() - 8;
    put(&mut head, 4, riff_len);
    put(&mut head, info.data.0 - 4, first_audio);
    let mut converter = WwiseRiffVorbis::builder(Cursor::new(head), books().map_err(text)?)
        .inline_codebooks(inline)
        .build()
        .map_err(text)?;
    let mut ogg = Vec::new();
    converter.generate_ogg(&mut ogg).map_err(text)?;
    let mut reader = PacketReader::new(Cursor::new(ogg));
    for _ in 0..2 {
        reader.read_packet().map_err(|e| e.to_string())?;
    }
    let setup = reader
        .read_packet()
        .map_err(|e| e.to_string())?
        .ok_or("header-only conversion has no setup header")?;
    ModeTable::from_setup(&setup.data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_passes_values_through_and_catches_panics() {
        assert_eq!(contain(|| 7), Ok(7));
        assert_eq!(
            contain(|| -> u8 { panic!("bad floor {}", 3) }),
            Err("bad floor 3".to_owned())
        );
        assert_eq!(
            contain(|| -> u8 { panic!("plain") }),
            Err("plain".to_owned())
        );
        assert_eq!(
            contain(|| -> u8 { panic::panic_any(5_u32) }),
            Err("panic with a non-string payload".to_owned())
        );
    }
}
