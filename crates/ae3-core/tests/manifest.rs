//! Manifest decoding against buffers built field by field.

use ae3_core::wire::{DecodeError, WireType};
use ae3_core::{AssetListing, AssetVersion, Manifest, Package, PackageKind};

/// A just-enough protobuf encoder for building fixtures.
#[derive(Default)]
struct Msg(Vec<u8>);

impl Msg {
    fn varint_raw(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(byte);
                return;
            }
            self.0.push(byte | 0x80);
        }
    }

    fn tag(&mut self, field: u64, wire_type: u64) {
        self.varint_raw(field << 3 | wire_type);
    }

    fn varint(mut self, field: u64, v: u64) -> Self {
        self.tag(field, 0);
        self.varint_raw(v);
        self
    }

    fn bytes(mut self, field: u64, b: &[u8]) -> Self {
        self.tag(field, 2);
        self.varint_raw(b.len() as u64);
        self.0.extend_from_slice(b);
        self
    }

    fn str(self, field: u64, s: &str) -> Self {
        self.bytes(field, s.as_bytes())
    }

    fn msg(self, field: u64, m: &Msg) -> Self {
        self.bytes(field, &m.0)
    }

    fn fixed32(mut self, field: u64) -> Self {
        self.tag(field, 5);
        self.0.extend_from_slice(&[0; 4]);
        self
    }
}

fn bundle_record() -> Msg {
    Msg::default()
        .str(1, "charui_vader")
        .varint(2, 0xdead_beef_cafe)
        .varint(3, 1_234)
        .varint(4, 5_678)
        .varint(5, 1)
        .varint(6, 3)
        .varint(7, 0)
        .msg(
            8,
            &Msg::default()
                .str(1, "tex_vader")
                .varint(2, 4_096)
                .varint(3, 512),
        )
        .msg(8, &Msg::default().str(1, "mat_vader"))
        .str(9, "shared_shaders")
        .str(9, "shared_fonts")
        .varint(10, 0xffff_ffff)
}

#[test]
fn decodes_every_field() {
    let audio = Msg::default()
        .str(1, "sfx_main")
        .varint(7, 1)
        // rank -1 as int32: sign-extended to ten bytes on the wire.
        .varint(6, u64::MAX);
    let buf = Msg::default()
        .varint(1, 100_054)
        .msg(2, &bundle_record())
        .msg(2, &audio)
        .str(3, "Windows")
        .str(4, "ETC")
        .str(5, "PROD")
        .varint(6, 1_760_000_000_000)
        .varint(7, 42)
        .0;

    let m = Manifest::decode(&buf).unwrap();
    assert_eq!(m.asset_version, AssetVersion(100_054));
    assert_eq!(m.platform, "Windows");
    assert_eq!(m.tex_format, "ETC");
    assert_eq!(m.environment, "PROD");
    assert_eq!(m.timestamp, 1_760_000_000_000);
    assert_eq!(m.revision, 42);
    assert_eq!(
        m.packages[0],
        Package {
            name: "charui_vader".into(),
            content_hash: 0xdead_beef_cafe,
            size: 1_234,
            uncompressed_size: 5_678,
            shared: true,
            rank: 3,
            kind: PackageKind::Bundle,
            assets: vec![
                AssetListing {
                    name: "tex_vader".into(),
                    runtime_size: 4_096,
                    clone_runtime_size: 512,
                },
                AssetListing {
                    name: "mat_vader".into(),
                    ..AssetListing::default()
                },
            ],
            dependencies: vec!["shared_shaders".into(), "shared_fonts".into()],
            crc: 0xffff_ffff,
        }
    );
    assert_eq!(m.packages[0].category(), "charui");
    assert_eq!(m.packages[1].kind, PackageKind::AudioPackage);
    assert_eq!(m.packages[1].rank, -1);
}

#[test]
fn empty_buffer_is_an_empty_manifest() {
    assert_eq!(Manifest::decode(&[]), Ok(Manifest::default()));
}

#[test]
fn skips_unknown_fields_at_every_level() {
    let entry = Msg::default().str(1, "a").fixed32(15).varint(99, 7);
    let record = Msg::default().str(1, "p").msg(8, &entry).str(42, "future");
    let buf = Msg::default()
        .fixed32(20)
        .msg(2, &record)
        .varint(1_000, 1)
        .varint(1, 7)
        .0;
    let m = Manifest::decode(&buf).unwrap();
    assert_eq!(m.asset_version, AssetVersion(7));
    assert_eq!(m.packages[0].name, "p");
    assert_eq!(m.packages[0].assets[0].name, "a");
}

#[test]
fn wrong_wire_type_for_known_field_is_an_error() {
    let buf = Msg::default().str(1, "100054").0;
    assert_eq!(
        Manifest::decode(&buf),
        Err(DecodeError::UnexpectedWireType {
            message: "RawAssetManifest",
            field: "version",
            expected: WireType::Varint,
            found: WireType::Len,
            offset: 0,
        })
    );
}

#[test]
fn invalid_utf8_names_the_field() {
    let record = Msg::default().bytes(1, &[0xff, 0xfe]);
    let buf = Msg::default().msg(2, &record).0;
    assert_eq!(
        Manifest::decode(&buf),
        Err(DecodeError::InvalidUtf8 {
            message: "Record",
            field: "name",
            offset: 4,
        })
    );
}

#[test]
fn every_truncation_point_is_an_error_not_a_panic() {
    let buf = Msg::default()
        .varint(1, 100_054)
        .msg(2, &bundle_record())
        .str(5, "PROD")
        .0;
    for len in 0..buf.len() {
        // Some prefixes end exactly on a field boundary and decode cleanly;
        // the rest must fail with an error.
        let _ = Manifest::decode(&buf[..len]);
    }
    assert!(Manifest::decode(&buf[..buf.len() - 1]).is_err());
}
