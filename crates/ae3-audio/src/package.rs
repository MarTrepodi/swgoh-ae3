//! An opened Audio Package and the sounds in it.

use std::collections::HashSet;
use std::io::{Cursor, Read};

use crate::akpk::{Akpk, Language, LutEntry};
use crate::bank;
use crate::bytes::to_usize;
use crate::convert::{Converted, convert_wem};
use crate::error::{Error, Result};
use crate::wem::WemInfo;

/// The largest `.pck` this crate inflates. The biggest SWGOH Audio Package
/// is a few tens of megabytes; this only guards against zip bombs.
pub const MAX_PCK_SIZE: u64 = 1 << 30;

/// Where in the package a sound is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundSource {
    /// A streamed WEM listed in the package's streamed file table.
    Stream,
    /// An external source listed in the package's externals table.
    External,
    /// A WEM embedded in the sound bank with this id.
    Bank {
        /// The sound bank's id from the package's sound bank table.
        bank_id: u64,
    },
}

/// One WEM in an Audio Package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sound {
    /// The Wwise media (or external source) id. Ids are 32-bit except for
    /// externals.
    pub id: u64,
    /// The language name, such as `sfx` for sounds that are not localised.
    pub language: String,
    /// Where the sound is stored.
    pub source: SoundSource,
    /// Size of the WEM in bytes.
    pub size: u64,
    /// The parsed WEM header, or `None` when it is unreadable (such sounds
    /// still export as raw `.wem`).
    pub info: Option<WemInfo>,
    /// Byte offset of the WEM within the `.pck`.
    offset: u64,
}

impl Sound {
    /// A file name stem that is unique within its package, such as
    /// `123456` for an unlocalised stream, `english(us)/123456` for a
    /// localised one, or `bank_987/123456` for a sound embedded in a bank.
    /// Characters that are unsafe in file names become `_`.
    #[must_use]
    pub fn file_stem(&self) -> String {
        let language = if self.language == "sfx" {
            String::new()
        } else {
            let safe: String = self
                .language
                .chars()
                .map(|c| {
                    if "/\\:*?\"<>|".contains(c) || c.is_control() {
                        '_'
                    } else {
                        c
                    }
                })
                .collect();
            format!("{safe}/")
        };
        let place = match self.source {
            SoundSource::Stream => String::new(),
            SoundSource::External => "external/".to_owned(),
            SoundSource::Bank { bank_id } => format!("bank_{bank_id}/"),
        };
        format!("{language}{place}{}", self.id)
    }

    fn is_bank_media(&self) -> bool {
        matches!(self.source, SoundSource::Bank { .. })
    }
}

/// An opened Audio Package: the `.pck` it holds, its languages and sounds.
#[derive(Debug, Clone)]
pub struct AudioPackage {
    pck_name: String,
    pck: Vec<u8>,
    languages: Vec<Language>,
    sounds: Vec<Sound>,
}

impl AudioPackage {
    /// Opens an Audio Package from the bytes of a `.wwpkg` (a zip holding a
    /// `.pck`) or of a bare `.pck`.
    pub fn open(bytes: &[u8]) -> Result<Self> {
        match bytes.get(..4) {
            Some(b"AKPK") => Self::from_pck(String::new(), bytes.to_vec()),
            Some(b"PK\x03\x04" | b"PK\x05\x06") => {
                let (name, pck) = read_pck_from_zip(bytes)?;
                Self::from_pck(name, pck)
            }
            _ => Err(Error::NotAudioPackage),
        }
    }

    /// Opens a bare `.pck`, taking ownership of its bytes. `name` is
    /// reported by [`pck_name`](Self::pck_name).
    pub fn from_pck(name: String, pck: Vec<u8>) -> Result<Self> {
        let akpk = Akpk::parse(&pck)?;
        let language = |e: &LutEntry| akpk.language_name(e.language_id);
        let direct = |entries: &[LutEntry], source: SoundSource| -> Vec<Sound> {
            entries
                .iter()
                .map(|e| Sound {
                    id: e.id,
                    language: language(e),
                    source,
                    size: e.size,
                    info: None,
                    offset: e.offset,
                })
                .collect()
        };
        let mut sounds = direct(&akpk.streamed_files, SoundSource::Stream);
        sounds.extend(direct(&akpk.externals, SoundSource::External));
        for entry in &akpk.sound_banks {
            let start = to_usize(entry.offset, "AKPK lookup table")?;
            let bank = &pck[start..start + to_usize(entry.size, "AKPK lookup table")?];
            sounds.extend(
                bank::embedded_media(bank, akpk.endian)?
                    .into_iter()
                    .map(|m| Sound {
                        id: u64::from(m.id),
                        language: language(entry),
                        source: SoundSource::Bank { bank_id: entry.id },
                        size: m.size,
                        info: None,
                        offset: entry.offset + m.offset,
                    }),
            );
        }
        let mut package = Self {
            pck_name: name,
            pck,
            languages: akpk.languages,
            sounds,
        };
        for i in 0..package.sounds.len() {
            let info = WemInfo::parse(package.slice(&package.sounds[i])?).ok();
            package.sounds[i].info = info;
        }
        // Banks embed the head ("prefetch") of streamed sounds so playback
        // can start at once. Where the package also holds the whole sound,
        // the head is redundant.
        let complete: HashSet<u64> = package
            .sounds
            .iter()
            .filter(|s| !s.is_bank_media() && s.info.is_some_and(|i| !i.truncated))
            .map(|s| s.id)
            .collect();
        package.sounds.retain(|s| {
            !(s.is_bank_media() && s.info.is_some_and(|i| i.truncated) && complete.contains(&s.id))
        });
        Ok(package)
    }

    /// The path of the `.pck` inside the zip, or empty for a bare `.pck`.
    #[must_use]
    pub fn pck_name(&self) -> &str {
        &self.pck_name
    }

    /// The languages the package declares.
    #[must_use]
    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    /// Every sound: streamed files, then externals, then the WEMs embedded
    /// in each sound bank. A bank's truncated prefetch copy of a sound is
    /// left out when the package also streams the whole sound; one whose
    /// stream lives elsewhere stays, with [`WemInfo::truncated`] set.
    #[must_use]
    pub fn sounds(&self) -> &[Sound] {
        &self.sounds
    }

    /// The raw WEM bytes of `sound`, borrowed from the package.
    pub fn sound_slice(&self, sound: &Sound) -> Result<&[u8]> {
        self.slice(sound)
    }

    /// The raw WEM bytes of `sound`.
    pub fn sound_bytes(&self, sound: &Sound) -> Result<Vec<u8>> {
        self.slice(sound).map(<[u8]>::to_vec)
    }

    /// Converts `sound` as [`convert_wem`] does: Ogg Vorbis, WAV, or the raw
    /// WEM with a note for codecs that are not converted.
    pub fn convert(&self, sound: &Sound) -> Result<Converted> {
        convert_wem(self.slice(sound)?)
    }

    fn slice(&self, sound: &Sound) -> Result<&[u8]> {
        let out_of_range = || Error::SoundOutOfRange { id: sound.id };
        let start = usize::try_from(sound.offset).map_err(|_| out_of_range())?;
        let len = usize::try_from(sound.size).map_err(|_| out_of_range())?;
        start
            .checked_add(len)
            .and_then(|end| self.pck.get(start..end))
            .ok_or_else(out_of_range)
    }
}

/// Inflates the first `.pck` in the zip `bytes`, returning its path and data.
fn read_pck_from_zip(bytes: &[u8]) -> Result<(String, Vec<u8>)> {
    let zip_err = |e: zip::result::ZipError| Error::Zip(e.to_string());
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_err)?;
    let index = (0..archive.len())
        .find(|&i| {
            archive
                .name_for_index(i)
                .and_then(std::result::Result::ok)
                .is_some_and(|n| {
                    let n = n.as_bytes();
                    n.len() > 4 && n[n.len() - 4..].eq_ignore_ascii_case(b".pck")
                })
        })
        .ok_or(Error::NoPck)?;
    let mut file = archive.by_index(index).map_err(zip_err)?;
    let size = file.size();
    if size > MAX_PCK_SIZE {
        return Err(Error::PckTooLarge {
            size,
            limit: MAX_PCK_SIZE,
        });
    }
    let name = file.name().map_err(zip_err)?.into_owned();
    // The declared size is untrusted: it only sizes the first allocation
    // (up to 64 MiB), and the read itself is capped.
    let mut pck = Vec::with_capacity(to_usize(size.min(64 << 20), "zip entry")?);
    (&mut file).take(MAX_PCK_SIZE + 1).read_to_end(&mut pck)?;
    if pck.len() as u64 > MAX_PCK_SIZE {
        return Err(Error::PckTooLarge {
            size: pck.len() as u64,
            limit: MAX_PCK_SIZE,
        });
    }
    Ok((name, pck))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::akpk::tests::build_pck;
    use crate::bank::tests::build_bank;
    use crate::wem::Codec;
    use crate::wem::tests::{build_wem, fmt_chunk};

    fn pcm_wem(fill: u8) -> Vec<u8> {
        build_wem(&fmt_chunk(1, 1, 8000, 2, 16, &[]), &[], &[fill; 16])
    }

    fn zip_of(name: &str, data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(data).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn enumerates_streams_and_bank_media() {
        let (a, b) = (pcm_wem(1), pcm_wem(2));
        let bank = build_bank(77, &[(500, &b)]);
        let pck = build_pck(&[(77, &bank)], &[(400, &a)], Some(&[(1 << 40, &a)]));
        let package = AudioPackage::open(&pck).unwrap();
        let sounds = package.sounds();
        assert_eq!(sounds.len(), 3);
        assert_eq!((sounds[0].id, sounds[0].source), (400, SoundSource::Stream));
        assert_eq!(sounds[1].source, SoundSource::External);
        assert_eq!(sounds[1].id, 1 << 40);
        assert_eq!(sounds[2].source, SoundSource::Bank { bank_id: 77 });
        assert_eq!(sounds[2].file_stem(), "bank_77/500");
        assert_eq!(sounds[0].file_stem(), "400");
        assert!(sounds.iter().all(|s| s.language == "sfx"));
        assert_eq!(package.sound_bytes(&sounds[0]).unwrap(), a);
        assert_eq!(package.sound_bytes(&sounds[2]).unwrap(), b);
        assert_eq!(sounds[2].info.unwrap().codec, Codec::Pcm);
        assert_eq!(package.convert(&sounds[2]).unwrap().ext, "wav");
    }

    #[test]
    fn drops_prefetch_heads_of_streams_it_holds() {
        let (a, b) = (pcm_wem(1), pcm_wem(2));
        let bank = build_bank(77, &[(400, &a[..50]), (401, &b[..50])]);
        let pck = build_pck(&[(77, &bank)], &[(400, &a)], None);
        let package = AudioPackage::open(&pck).unwrap();
        let ids: Vec<_> = package.sounds().iter().map(|s| (s.id, s.source)).collect();
        assert_eq!(
            ids,
            [
                (400, SoundSource::Stream),
                (401, SoundSource::Bank { bank_id: 77 })
            ]
        );
        let orphan = &package.sounds()[1];
        assert!(orphan.info.unwrap().truncated);
        let converted = package.convert(orphan).unwrap();
        assert_eq!(converted.ext, "wem");
        assert!(converted.note.unwrap().contains("truncated"));
    }

    #[test]
    fn corrupt_packages_are_errors_not_panics() {
        let a = pcm_wem(1);
        let bank = build_bank(77, &[(500, &a), (501, &a[..50])]);
        let pck = build_pck(&[(77, &bank)], &[(400, &a)], Some(&[(9, &a)]));
        for i in 0..pck.len() {
            for value in [0x00, 0x01, 0x7F, 0xFF] {
                let mut bad = pck.clone();
                bad[i] = value;
                if let Ok(package) = AudioPackage::open(&bad) {
                    for sound in package.sounds() {
                        package.convert(sound).unwrap();
                    }
                }
            }
        }
        for len in 0..pck.len() {
            let _ = AudioPackage::open(&pck[..len]);
        }
    }

    #[test]
    fn opens_zip_and_names_the_pck() {
        let a = pcm_wem(3);
        let pck = build_pck(&[], &[(1, &a)], None);
        let package = AudioPackage::open(&zip_of("Wwise/x.PCK", &pck)).unwrap();
        assert_eq!(package.pck_name(), "Wwise/x.PCK");
        assert_eq!(package.sounds().len(), 1);
    }

    #[test]
    fn reports_missing_pck_and_foreign_bytes() {
        assert!(matches!(
            AudioPackage::open(&zip_of("readme.txt", b"hi")),
            Err(Error::NoPck)
        ));
        assert!(matches!(
            AudioPackage::open(b"nope"),
            Err(Error::NotAudioPackage)
        ));
        assert!(matches!(
            AudioPackage::open(b"PK\x03\x04junk"),
            Err(Error::Zip(_))
        ));
    }

    #[test]
    fn sound_from_another_package_is_out_of_range() {
        let big = build_wem(&fmt_chunk(1, 1, 8000, 2, 16, &[]), &[], &[0; 4096]);
        let large = AudioPackage::open(&build_pck(&[], &[(1, &big)], None)).unwrap();
        let small = AudioPackage::open(&build_pck(&[], &[(1, &pcm_wem(0))], None)).unwrap();
        let foreign = &large.sounds()[0];
        assert!(matches!(
            small.sound_bytes(foreign),
            Err(Error::SoundOutOfRange { id: 1 })
        ));
    }
}
