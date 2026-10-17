//! Decodes the real game textures in the fixture Bundles.
//!
//! ae3-image may not depend on the Bundle parser, so this test carries a
//! just-enough `UnityFS` and `SerializedFile` reader for the shapes SWGOH ships
//! (`UnityFS` 8, LZ4HC directory, one LZMA data block, `SerializedFile` 22) and
//! reads each `Texture2D`'s width, height, format and `m_StreamData` slice.
//!
//! Fixtures come from `AE3_FIXTURES` (default `<workspace>/fixtures`); the test
//! skips when they are absent. Set `AE3_DUMP_DIR` to also write every decoded
//! texture there as PNG for visual inspection.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test-only parser"
)]

use std::io::Read;
use std::path::PathBuf;

use ae3_image::{ImageFormat, TextureFormat, decode, encode, thumbnail};

fn fixtures_dir() -> Option<PathBuf> {
    let root = std::env::var_os("AE3_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        PathBuf::from,
    );
    let dir = root.join("100054/Windows");
    dir.is_dir().then_some(dir)
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    big_endian: bool,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], big_endian: bool) -> Self {
        Self {
            data,
            pos: 0,
            big_endian,
        }
    }

    fn bytes(&mut self, n: usize) -> &'a [u8] {
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        b
    }

    fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut a: [u8; N] = self.bytes(N).try_into().unwrap();
        if !self.big_endian {
            a.reverse();
        }
        a
    }

    fn u8(&mut self) -> u8 {
        self.bytes(1)[0]
    }

    fn u16(&mut self) -> u16 {
        u16::from_be_bytes(self.array())
    }

    fn u32(&mut self) -> u32 {
        u32::from_be_bytes(self.array())
    }

    fn i32(&mut self) -> i32 {
        i32::from_be_bytes(self.array())
    }

    fn u64(&mut self) -> u64 {
        u64::from_be_bytes(self.array())
    }

    fn cstr(&mut self) -> &'a [u8] {
        let len = self.data[self.pos..].iter().position(|&b| b == 0).unwrap();
        let s = self.bytes(len);
        self.pos += 1;
        s
    }

    fn align(&mut self, to: usize) {
        self.pos = self.pos.next_multiple_of(to);
    }
}

/// Returns the archive's nodes as (path, bytes).
fn unityfs_nodes(file: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut c = Cursor::new(file, true);
    assert_eq!(c.cstr(), b"UnityFS");
    assert_eq!(c.u32(), 8);
    c.cstr();
    c.cstr();
    c.u64();
    let (compressed, uncompressed, flags) = (c.u32() as usize, c.u32() as usize, c.u32());
    assert_eq!(flags, 0x243, "LZ4HC directory at the start, padded");
    c.align(16);
    let directory = lz4_flex::block::decompress(c.bytes(compressed), uncompressed).unwrap();
    c.align(16);

    let mut d = Cursor::new(&directory, true);
    d.bytes(16);
    let blocks: Vec<(usize, usize, u16)> = (0..d.i32())
        .map(|_| (d.u32() as usize, d.u32() as usize, d.u16()))
        .collect();
    let mut data = Vec::new();
    for (size, packed, block_flags) in blocks {
        let raw = c.bytes(packed);
        assert_eq!(block_flags & 0x3f, 1, "LZMA data block");
        let props = raw[0];
        let dict = u32::from_le_bytes(raw[1..5].try_into().unwrap());
        let mut reader =
            lzma_rust2::LzmaReader::new_with_props(&raw[5..], size as u64, props, dict, None)
                .unwrap();
        let start = data.len();
        data.resize(start + size, 0);
        reader.read_exact(&mut data[start..]).unwrap();
    }
    (0..d.i32())
        .map(|_| {
            let (offset, size) = (d.u64() as usize, d.u64() as usize);
            d.u32();
            let path = String::from_utf8(d.cstr().to_vec()).unwrap();
            (path, data[offset..offset + size].to_vec())
        })
        .collect()
}

struct Texture {
    name: String,
    width: u32,
    height: u32,
    format: i32,
    stream_offset: usize,
    stream_size: usize,
}

/// Every `Texture2D` (class 28) in a version-22 `SerializedFile`.
fn textures(file: &[u8]) -> Vec<Texture> {
    let mut c = Cursor::new(file, true);
    c.bytes(8);
    assert_eq!(c.u32(), 22);
    c.u32();
    let little = c.u8() == 0;
    c.bytes(3);
    c.u32();
    c.u64();
    let data_offset = c.u64() as usize;
    c.u64();
    c.big_endian = !little;

    c.cstr();
    c.i32();
    assert_eq!(c.u8(), 1, "TypeTrees enabled");
    let class_ids: Vec<i32> = (0..c.i32())
        .map(|_| {
            let class_id = c.i32();
            c.u8();
            c.u16();
            if class_id == 114 {
                c.bytes(16);
            }
            c.bytes(16);
            let (nodes, strings) = (c.i32() as usize, c.i32() as usize);
            c.bytes(nodes * 32 + strings);
            let dependencies = c.i32() as usize;
            c.bytes(dependencies * 4);
            class_id
        })
        .collect();

    let objects: Vec<(usize, usize, i32)> = (0..c.i32())
        .map(|_| {
            c.align(4);
            c.u64();
            let start = c.u64() as usize;
            let size = c.u32() as usize;
            (start, size, class_ids[c.i32() as usize])
        })
        .collect();

    objects
        .into_iter()
        .filter(|&(_, _, class)| class == 28)
        .map(|(start, size, _)| {
            let object = &file[data_offset + start..data_offset + start + size];
            let mut o = Cursor::new(object, false);
            let name_len = o.i32() as usize;
            let name = String::from_utf8(o.bytes(name_len).to_vec()).unwrap();
            o.align(4);
            // Unity 6 SWGOH Bundles have one 4-byte field before m_Width; older
            // layouts have m_ForcedFallbackFormat plus two aligned bools. Pick the
            // offset where m_Width, m_Height, m_CompleteImageSize and m_TextureFormat
            // agree with each other.
            let after_name = o.pos;
            let (width, height, format) = [4, 8, 0]
                .into_iter()
                .find_map(|skip| {
                    o.pos = after_name + skip;
                    let (w, h, complete) = (o.i32() as u32, o.i32() as u32, o.i32() as usize);
                    o.i32(); // m_MipsStripped
                    let format = o.i32();
                    let size = TextureFormat::from_id(format)?.image_data_size(w, h)?;
                    (size > 0 && size <= complete).then_some((w, h, format))
                })
                .expect("Texture2D header fields");
            // m_StreamData {offset: u64, size: u32, path: string} ends the object;
            // find it through its "archive:/" path.
            let path_at = object
                .windows(9)
                .rposition(|w| w == b"archive:/")
                .expect("texture data is streamed");
            let mut s = Cursor::new(&object[path_at - 16..], false);
            let stream_offset = s.u64() as usize;
            let stream_size = s.u32() as usize;
            Texture {
                name,
                width,
                height,
                format,
                stream_offset,
                stream_size,
            }
        })
        .collect()
}

/// Decodes every texture in `bundle`, checks it, and returns (name, format).
fn decode_bundle(dir: &std::path::Path, bundle: &str) -> Vec<(String, TextureFormat)> {
    let file = std::fs::read(dir.join(bundle)).unwrap();
    let nodes = unityfs_nodes(&file);
    let serialized = &nodes.iter().find(|(p, _)| !p.ends_with(".resS")).unwrap().1;
    let res_s = nodes
        .iter()
        .find(|(p, _)| p.ends_with(".resS"))
        .map(|(_, d)| d);
    let dump = std::env::var_os("AE3_DUMP_DIR").map(PathBuf::from);

    let mut decoded = Vec::new();
    for texture in textures(serialized) {
        let format = TextureFormat::try_from(texture.format).unwrap();
        let res_s = res_s.expect("streamed texture needs a .resS node");
        let data = &res_s[texture.stream_offset..texture.stream_offset + texture.stream_size];
        let level0 = format
            .image_data_size(texture.width, texture.height)
            .unwrap();
        assert!(level0 <= data.len(), "{} {format}", texture.name);

        let image = decode(format, texture.width, texture.height, data).unwrap();
        assert_eq!(
            (image.width(), image.height()),
            (texture.width, texture.height)
        );
        // A real texture is not a single flat colour.
        let pixels = image.pixels().as_chunks::<4>().0;
        assert!(
            pixels.iter().any(|p| *p != pixels[0]),
            "{} is flat",
            texture.name
        );
        // A truncated buffer is an error, not a panic.
        assert!(decode(format, texture.width, texture.height, &data[..level0 - 1]).is_err());

        let png = encode(&image, ImageFormat::default()).unwrap();
        let webp = encode(&thumbnail(&image, 64), ImageFormat::WebP).unwrap();
        assert!(png.starts_with(b"\x89PNG") && webp.starts_with(b"RIFF"));
        if let Some(dir) = &dump {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("{}_{format}.png", texture.name)), &png).unwrap();
        }
        decoded.push((texture.name, format));
    }
    decoded
}

#[test]
fn decodes_fixture_textures() {
    let Some(dir) = fixtures_dir() else {
        eprintln!("fixtures not found; skipping");
        return;
    };
    let mut formats = std::collections::BTreeSet::new();
    for bundle in [
        "charui_bossk.bundle",
        "abilityui_passive_summon.bundle",
        "vanity_bossk.bundle",
        "relic_rfl_bossk.bundle",
        "territoryui_view_standard_atlas.bundle",
        "unit_details_atlas.bundle",
        "char_bossk_pre.bundle",
    ] {
        if !dir.join(bundle).is_file() {
            eprintln!("{bundle} not found; skipping it");
            continue;
        }
        let decoded = decode_bundle(&dir, bundle);
        assert!(!decoded.is_empty(), "{bundle} has no textures");
        eprintln!("{bundle}: {decoded:?}");
        formats.extend(decoded.into_iter().map(|(_, f)| f));
    }
    eprintln!("formats decoded: {formats:?}");
}
