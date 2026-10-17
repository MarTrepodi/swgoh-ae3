//! The Wwise file package (`.pck`, magic `AKPK`) container.
//!
//! Layout, all integers in the package's byte order:
//!
//! ```text
//! "AKPK"  header_size:u32  (bytes after this field, up to the first file)
//! version:u32  language_map_size:u32  sound_bank_lut_size:u32
//! streamed_file_lut_size:u32  [externals_lut_size:u32]
//! language map | sound bank LUT | streamed file LUT | [externals LUT]
//! ```
//!
//! The language map is `count:u32`, then `count` pairs of
//! `(string_offset:u32, language_id:u32)`, then the NUL-terminated names
//! (UTF-16 on most platforms, sometimes 8-bit), with offsets relative to the
//! start of the map. Each LUT is `count:u32` followed by `count` entries of
//! `(id, block_size:u32, file_size:u32, start_block:u32, language_id:u32)`,
//! where `id` is 32-bit, or 64-bit for externals and some newer packages; the
//! entry width tells them apart. A file starts at `start_block * block_size`
//! from the beginning of the package.

use crate::bytes::{Endian, Reader, to_usize};
use crate::error::{Error, Result};

const CONTEXT: &str = "AKPK header";

/// One language of an Audio Package, such as `sfx` for sounds that are not
/// localised, or `english(us)` for voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// Wwise's numeric language id, as referenced by the lookup tables.
    pub id: u32,
    /// The language name stored in the package.
    pub name: String,
}

/// One file listed in a lookup table: where it is and which language it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LutEntry {
    pub(crate) id: u64,
    pub(crate) offset: u64,
    pub(crate) size: u64,
    pub(crate) language_id: u32,
}

/// The parsed header of a `.pck`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Akpk {
    pub(crate) endian: Endian,
    pub(crate) languages: Vec<Language>,
    pub(crate) sound_banks: Vec<LutEntry>,
    pub(crate) streamed_files: Vec<LutEntry>,
    pub(crate) externals: Vec<LutEntry>,
}

impl Akpk {
    /// Parses the header of `data`, a whole `.pck`, and checks that every
    /// listed file lies inside it.
    pub(crate) fn parse(data: &[u8]) -> Result<Self> {
        if data.get(..4) != Some(b"AKPK") {
            return Err(Error::malformed(CONTEXT, "missing AKPK magic"));
        }
        let endian = detect_endian(data)?;
        let mut r = Reader::at(data, 4, endian, CONTEXT);
        let header_size = u64::from(r.u32()?);
        let _version = r.u32()?;
        let language_size = u64::from(r.u32()?);
        let bank_size = u64::from(r.u32()?);
        let stream_size = u64::from(r.u32()?);
        // header_size counts the version and the size fields, then the maps.
        let without_externals = 16 + language_size + bank_size + stream_size;
        let externals_candidate = u64::from(Reader::at(data, r.pos(), endian, CONTEXT).u32()?);
        let externals_size = if without_externals + 4 + externals_candidate == header_size {
            r.u32()?;
            Some(externals_candidate)
        } else if without_externals <= header_size {
            None
        } else {
            return Err(Error::malformed(
                CONTEXT,
                format!("header size {header_size} is smaller than its maps ({without_externals})"),
            ));
        };
        let header_end = to_usize(8 + header_size, CONTEXT)?;
        if header_end > data.len() {
            return Err(Error::malformed(
                CONTEXT,
                format!(
                    "header of {header_end} bytes overruns the {}-byte file",
                    data.len()
                ),
            ));
        }

        let mut section = |size: u64| -> Result<&[u8]> { r.bytes(to_usize(size, CONTEXT)?) };
        let language_map = section(language_size)?;
        let bank_lut = section(bank_size)?;
        let stream_lut = section(stream_size)?;
        let externals_lut = match externals_size {
            Some(size) => section(size)?,
            None => &[],
        };

        let pck = Self {
            endian,
            languages: parse_language_map(language_map, endian)?,
            sound_banks: parse_lut(bank_lut, endian, "sound bank LUT")?,
            streamed_files: parse_lut(stream_lut, endian, "streamed file LUT")?,
            externals: parse_lut(externals_lut, endian, "externals LUT")?,
        };
        let len = data.len() as u64;
        for entry in pck
            .sound_banks
            .iter()
            .chain(&pck.streamed_files)
            .chain(&pck.externals)
        {
            if entry
                .offset
                .checked_add(entry.size)
                .is_none_or(|end| end > len)
            {
                return Err(Error::malformed(
                    "AKPK lookup table",
                    format!(
                        "file {} at {}+{} overruns the {len}-byte package",
                        entry.id, entry.offset, entry.size
                    ),
                ));
            }
        }
        Ok(pck)
    }

    /// The name of language `id`, or a stand-in such as `language 7` when the
    /// language map does not list it.
    pub(crate) fn language_name(&self, id: u32) -> String {
        self.languages
            .iter()
            .find(|l| l.id == id)
            .map_or_else(|| format!("language {id}"), |l| l.name.clone())
    }
}

/// The version field is 1 in every known package, which fixes the byte order.
fn detect_endian(data: &[u8]) -> Result<Endian> {
    let version: [u8; 4] = Reader::at(data, 8, Endian::Little, CONTEXT).array()?;
    Ok(
        if u32::from_le_bytes(version) != 1 && u32::from_be_bytes(version) == 1 {
            Endian::Big
        } else {
            Endian::Little
        },
    )
}

fn parse_language_map(map: &[u8], endian: Endian) -> Result<Vec<Language>> {
    const CONTEXT: &str = "AKPK language map";
    if map.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(map, endian, CONTEXT);
    let count = r.u32()?;
    // Each entry needs 8 bytes; refuse counts the map cannot hold before
    // allocating for them.
    if u64::from(count) * 8 > map.len() as u64 {
        return Err(Error::malformed(
            CONTEXT,
            format!("{count} entries do not fit"),
        ));
    }
    (0..count)
        .map(|_| {
            let offset = to_usize(u64::from(r.u32()?), CONTEXT)?;
            let id = r.u32()?;
            let name =
                read_name(map.get(offset..).unwrap_or_default(), endian).ok_or_else(|| {
                    Error::malformed(CONTEXT, format!("name of language {id} is unterminated"))
                })?;
            Ok(Language { id, name })
        })
        .collect()
}

/// Reads a NUL-terminated name that is UTF-16 (the usual case: ASCII names
/// have a zero in every other byte) or 8-bit.
fn read_name(bytes: &[u8], endian: Endian) -> Option<String> {
    let utf16 = match (bytes.first(), bytes.get(1), endian) {
        (Some(&a), Some(0), Endian::Little) if a != 0 => true,
        (Some(0), Some(&b), Endian::Big) if b != 0 => true,
        _ => false,
    };
    if utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| match endian {
                Endian::Little => u16::from_le_bytes(c),
                Endian::Big => u16::from_be_bytes(c),
            })
            .take_while(|&u| u != 0)
            .collect();
        (units.len() * 2 < bytes.len()).then(|| String::from_utf16_lossy(&units))
    } else {
        let end = bytes.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

fn parse_lut(lut: &[u8], endian: Endian, context: &'static str) -> Result<Vec<LutEntry>> {
    if lut.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(lut, endian, context);
    let count = r.u32()?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let body = (lut.len() - 4) as u64;
    let wide_id = match body / u64::from(count) {
        20 => false,
        24 => true,
        width => {
            return Err(Error::malformed(
                context,
                format!("{count} entries in {body} bytes is {width} bytes each, not 20 or 24"),
            ));
        }
    };
    (0..count)
        .map(|_| {
            let id = if wide_id {
                r.u64()?
            } else {
                u64::from(r.u32()?)
            };
            let block_size = u64::from(r.u32()?);
            let size = u64::from(r.u32()?);
            let start_block = u64::from(r.u32()?);
            let language_id = r.u32()?;
            Ok(LutEntry {
                id,
                offset: start_block * block_size.max(1),
                size,
                language_id,
            })
        })
        .collect()
}

#[cfg(test)]
#[expect(clippy::cast_possible_truncation, reason = "test data is small")]
pub(crate) mod tests {
    use super::*;

    /// Builds a little-endian `.pck` with an `sfx` language map, the given
    /// banks and streamed files (each `(id, bytes)`), and an optional
    /// externals LUT. Files are laid out after the header with block size 16.
    pub(crate) fn build_pck(
        banks: &[(u32, &[u8])],
        streams: &[(u32, &[u8])],
        externals: Option<&[(u64, &[u8])]>,
    ) -> Vec<u8> {
        const BLOCK: usize = 16;
        let mut lang = Vec::new();
        lang.extend_from_slice(&1u32.to_le_bytes());
        lang.extend_from_slice(&12u32.to_le_bytes());
        lang.extend_from_slice(&0u32.to_le_bytes());
        lang.extend("sfx\0".encode_utf16().flat_map(u16::to_le_bytes));

        let lut_len = |n: usize, w: usize| if n == 0 { 4 } else { 4 + n * w };
        let ext = externals.unwrap_or_default();
        let header_size = 16
            + usize::from(externals.is_some()) * 4
            + lang.len()
            + lut_len(banks.len(), 20)
            + lut_len(streams.len(), 20)
            + externals.map_or(0, |e| lut_len(e.len(), 24));
        let mut next = (8 + header_size).div_ceil(BLOCK) * BLOCK;
        let mut place = |len: usize| {
            let at = next;
            next = (at + len).div_ceil(BLOCK) * BLOCK;
            at
        };
        let mut lut = |files: &[(u64, Vec<u8>)], wide: bool| {
            let mut out = (files.len() as u32).to_le_bytes().to_vec();
            let mut placed = Vec::new();
            for (id, bytes) in files {
                if wide {
                    out.extend_from_slice(&id.to_le_bytes());
                } else {
                    out.extend_from_slice(&(*id as u32).to_le_bytes());
                }
                let at = place(bytes.len());
                out.extend_from_slice(&(BLOCK as u32).to_le_bytes());
                out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                out.extend_from_slice(&((at / BLOCK) as u32).to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
                placed.push((at, bytes.clone()));
            }
            (out, placed)
        };
        let widen = |f: &[(u32, &[u8])]| {
            f.iter()
                .map(|&(i, b)| (u64::from(i), b.to_vec()))
                .collect::<Vec<_>>()
        };
        let ext: Vec<(u64, Vec<u8>)> = ext.iter().map(|&(i, b)| (i, b.to_vec())).collect();
        let (bank_lut, mut files) = lut(&widen(banks), false);
        let (stream_lut, f2) = lut(&widen(streams), false);
        files.extend(f2);
        let ext_lut = externals.map(|_| {
            let (l, f3) = lut(&ext, true);
            files.extend(f3);
            l
        });

        let mut out = b"AKPK".to_vec();
        out.extend_from_slice(&(header_size as u32).to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        for len in [lang.len(), bank_lut.len(), stream_lut.len()] {
            out.extend_from_slice(&(len as u32).to_le_bytes());
        }
        if let Some(l) = &ext_lut {
            out.extend_from_slice(&(l.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(&lang);
        out.extend_from_slice(&bank_lut);
        out.extend_from_slice(&stream_lut);
        if let Some(l) = &ext_lut {
            out.extend_from_slice(l);
        }
        assert_eq!(out.len(), 8 + header_size);
        for (at, bytes) in files {
            out.resize(at, 0);
            out.extend_from_slice(&bytes);
        }
        out
    }

    #[test]
    fn parses_header_tables_and_offsets() {
        let pck = build_pck(&[(7, b"bank")], &[(9, b"stream!"), (10, b"x")], None);
        let akpk = Akpk::parse(&pck).unwrap();
        assert_eq!(akpk.endian, Endian::Little);
        assert_eq!(
            akpk.languages,
            vec![Language {
                id: 0,
                name: "sfx".into()
            }]
        );
        assert_eq!(akpk.sound_banks.len(), 1);
        assert_eq!(akpk.externals.len(), 0);
        let s = akpk.streamed_files[0];
        assert_eq!((s.id, s.size), (9, 7));
        assert_eq!(&pck[s.offset as usize..][..7], b"stream!");
        assert_eq!(akpk.language_name(0), "sfx");
        assert_eq!(akpk.language_name(3), "language 3");
    }

    #[test]
    fn parses_64_bit_externals() {
        let id = 0x1122_3344_5566_7788;
        let pck = build_pck(&[], &[], Some(&[(id, b"ext")]));
        let akpk = Akpk::parse(&pck).unwrap();
        assert_eq!(akpk.externals.len(), 1);
        assert_eq!(akpk.externals[0].id, id);
        assert_eq!(&pck[akpk.externals[0].offset as usize..][..3], b"ext");
    }

    #[test]
    fn rejects_files_outside_the_package() {
        let mut pck = build_pck(&[], &[(1, b"abcd")], None);
        pck.truncate(pck.len() - 1);
        let err = Akpk::parse(&pck).unwrap_err();
        assert!(err.to_string().contains("overruns"), "{err}");
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(Akpk::parse(b"RIFF").is_err());
        assert!(Akpk::parse(b"AKPK\x48\0\0\0").is_err());
        let pck = build_pck(&[], &[(1, b"abcd")], None);
        for len in 0..40 {
            assert!(Akpk::parse(&pck[..len]).is_err(), "len {len}");
        }
    }

    #[test]
    fn reads_8_bit_and_utf16_names() {
        assert_eq!(read_name(b"sfx\0", Endian::Little).as_deref(), Some("sfx"));
        assert_eq!(
            read_name(b"s\0f\0x\0\0\0", Endian::Little).as_deref(),
            Some("sfx")
        );
        assert_eq!(read_name(b"\0s\0f\0\0", Endian::Big).as_deref(), Some("sf"));
        assert_eq!(read_name(b"s\0f\0", Endian::Little), None);
    }
}
