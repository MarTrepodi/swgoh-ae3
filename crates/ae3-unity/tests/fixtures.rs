//! Tests against real SWGOH Bundles for Asset Version 100054 (Windows).
//! Each test skips when its fixture is absent (see `common`).

mod common;

use ae3_unity::{
    Bundle, CLASS_MONO_BEHAVIOUR, Compression, PPtr, Probe, ScriptClass, StreamingInfo, Value,
    probe_prefix,
};

const BUNDLES: [&str; 8] = [
    "charui_bossk.bundle",
    "abilityui_passive_summon.bundle",
    "vanity_bossk.bundle",
    "relic_rfl_bossk.bundle",
    "territoryui_view_standard_atlas.bundle",
    "unit_details_atlas.bundle",
    "shared_uicontainer.bundle",
    "char_bossk_pre.bundle",
];

#[test]
fn parses_every_fixture_bundle() {
    for name in BUNDLES {
        let Some(bytes) = common::fixture(name) else {
            continue;
        };
        let bundle = Bundle::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let header = bundle.header();
        assert_eq!(header.signature, "UnityFS", "{name}");
        assert_eq!(header.format_version, 8, "{name}");
        assert_eq!(header.flags, 0x243, "{name}");
        assert_eq!(
            header.blocks_info_compression(),
            Compression::Lz4Hc,
            "{name}"
        );
        assert_eq!(bundle.unity_version(), "6000.3.17f1", "{name}");
        assert_eq!(header.size, bytes.len() as u64, "{name}");
        assert_eq!(bundle.blocks().len(), 1, "{name}");
        assert_eq!(
            bundle.blocks()[0].compression(),
            Compression::Lzma,
            "{name}"
        );

        let serialized: Vec<_> = bundle.serialized_nodes().collect();
        assert_eq!(serialized.len(), 1, "{name}");
        let cab = &serialized[0].path;
        assert!(cab.starts_with("CAB-"), "{name}: {cab}");
        let resources: Vec<_> = bundle.resource_nodes().collect();
        assert_eq!(resources.len(), 1, "{name}");
        assert_eq!(resources[0].path, format!("{cab}.resS"), "{name}");
        assert_eq!(bundle.node(cab), Some(serialized[0]));

        let sf = bundle.serialized_file().unwrap();
        assert_eq!(sf.format_version(), 22, "{name}");
        assert_eq!(sf.unity_version().as_deref(), Some("6000.3.17f1"), "{name}");
        assert_eq!(
            sf.target_platform(),
            Some(19),
            "{name}: StandaloneWindows64"
        );
        assert!(sf.has_type_trees(), "{name}");
        assert!(!sf.is_big_endian(), "{name}");
        assert!(sf.types().iter().all(|t| t.type_tree.is_some()), "{name}");
        assert!(!sf.objects().is_empty(), "{name}");
        for object in sf.objects() {
            assert_eq!(sf.object(object.path_id), Some(object));
            let value = sf
                .read_object(object.path_id)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(value.as_map().is_some(), "{name}");
            if let Some(Value::String(full)) = value.get("m_Name") {
                let fast = sf.object_name(object.path_id);
                assert_eq!(
                    fast.as_deref(),
                    Some(full.as_str()).filter(|s| !s.is_empty()),
                    "{name}"
                );
            }
        }

        let ab = sf
            .asset_bundle()
            .unwrap()
            .expect("every Bundle has an AssetBundle object");
        assert_eq!(ab.name, name, "{name}");
        assert!(!ab.container.is_empty(), "{name}");
        for entry in &ab.container {
            assert!(entry.asset.is_local(), "{name}: {entry:?}");
            assert!(
                sf.object(entry.asset.path_id).is_some(),
                "{name}: {entry:?}"
            );
        }
    }
}

#[test]
fn lists_portrait_bundle_objects() {
    let Some(bytes) = common::fixture("charui_bossk.bundle") else {
        return;
    };
    let bundle = Bundle::parse(&bytes).unwrap();
    let sf = bundle.serialized_file().unwrap();
    let listed: Vec<_> = sf
        .objects()
        .iter()
        .map(|o| (o.type_name.as_str(), sf.object_name(o.path_id)))
        .collect();
    assert_eq!(
        listed,
        [
            ("Texture2D", Some("tex.charui_bossk".to_owned())),
            ("AssetBundle", Some("charui_bossk.bundle".to_owned())),
        ]
    );
    let ab = sf.asset_bundle().unwrap().unwrap();
    assert_eq!(ab.dependencies, Vec::<String>::new());
    assert_eq!(ab.container.len(), 1);
    assert_eq!(ab.container[0].asset.path_id, sf.objects()[0].path_id);
}

/// `(fixture, m_TextureFormat, width, height)`
const TEXTURES: [(&str, i64, i64, i64); 4] = [
    ("charui_bossk.bundle", 10, 128, 128),               // DXT1
    ("relic_rfl_bossk.bundle", 12, 256, 256),            // DXT5
    ("unit_details_atlas.bundle", 4, 1024, 1024),        // RGBA32
    ("territoryui_view_standard_atlas.bundle", 4, 0, 0), // RGBA32, size checked loosely
];

#[test]
fn reads_texture_fields_and_resolves_stream_data() {
    for (name, format, width, height) in TEXTURES {
        let Some(bytes) = common::fixture(name) else {
            continue;
        };
        let bundle = Bundle::parse(&bytes).unwrap();
        let sf = bundle.serialized_file().unwrap();
        let texture = sf
            .objects()
            .iter()
            .find(|o| o.type_name == "Texture2D")
            .unwrap();
        let value = sf.read_object(texture.path_id).unwrap();
        let int = |key: &str| value.get(key).and_then(Value::as_i64).unwrap();
        assert_eq!(int("m_TextureFormat"), format, "{name}");
        let (w, h) = (int("m_Width"), int("m_Height"));
        if width > 0 {
            assert_eq!((w, h), (width, height), "{name}");
        }
        let expected_len = match format {
            10 => w * h / 2,
            12 => w * h,
            4 => w * h * 4,
            _ => unreachable!(),
        };
        assert_eq!(int("m_CompleteImageSize"), expected_len, "{name}");
        assert_eq!(
            value.get("image data").and_then(Value::as_bytes),
            Some(&[][..]),
            "{name}"
        );

        let stream = StreamingInfo::from_value(value.get("m_StreamData").unwrap()).unwrap();
        assert!(stream.path.starts_with("archive:/CAB-"), "{name}");
        assert!(stream.path.ends_with(".resS"), "{name}");
        assert_eq!(stream.size, u64::try_from(expected_len).unwrap(), "{name}");
        let pixels = bundle.resolve_stream(&stream).unwrap();
        assert_eq!(pixels.len() as u64, stream.size, "{name}");
        assert!(
            pixels.iter().any(|&b| b != 0),
            "{name}: pixel data is all zero"
        );
        // Out-of-range and unknown paths are errors, not panics.
        assert!(
            bundle
                .stream_data(&stream.path, stream.offset + 1, stream.size + u64::MAX / 2)
                .is_err()
        );
        assert!(
            bundle
                .stream_data("archive:/CAB-nope/CAB-nope.resS", 0, 1)
                .is_err()
        );
    }
}

#[test]
fn resolves_textures_in_a_prefab_resource_node() {
    let Some(bytes) = common::fixture("char_bossk_pre.bundle") else {
        return;
    };
    let bundle = Bundle::parse(&bytes).unwrap();
    let sf = bundle.serialized_file().unwrap();
    let mut seen = 0;
    for texture in sf.objects().iter().filter(|o| o.type_name == "Texture2D") {
        let value = sf.read_object(texture.path_id).unwrap();
        assert_eq!(
            value.get("m_TextureFormat").and_then(Value::as_i64),
            Some(25),
            "BC7"
        );
        let stream = StreamingInfo::from_value(value.get("m_StreamData").unwrap()).unwrap();
        let data = bundle.resolve_stream(&stream).unwrap();
        let w = value.get("m_Width").and_then(Value::as_u64).unwrap();
        let h = value.get("m_Height").and_then(Value::as_u64).unwrap();
        assert_eq!(data.len() as u64, w * h, "BC7 is one byte per pixel");
        seen += 1;
    }
    assert!(seen >= 4);
    let ab = sf.asset_bundle().unwrap().unwrap();
    assert_eq!(
        ab.dependencies,
        [
            "shared_animationcontainer.bundle",
            "shared_resourcecontainer.bundle",
            "shared_shadercontainer.bundle"
        ]
    );
    assert_eq!(sf.externals().len(), 4);
}

#[test]
fn reads_ngui_atlas_sprites() {
    let Some(bytes) = common::fixture("unit_details_atlas.bundle") else {
        return;
    };
    let bundle = Bundle::parse(&bytes).unwrap();
    let sf = bundle.serialized_file().unwrap();

    let atlas = sf
        .objects_of_class(CLASS_MONO_BEHAVIOUR)
        .find(|o| sf.script_class(o.path_id) == Some(ScriptClass::Local("NGUIAtlas".into())))
        .expect("an NGUIAtlas MonoBehaviour");
    let value = sf.read_object(atlas.path_id).unwrap();
    assert_eq!(
        value.get("m_Name").and_then(Value::as_str),
        Some("unit_details_atlas")
    );

    // material -> Material._MainTex -> Texture2D, all in this file.
    let material = value.get("material").and_then(Value::as_pptr).unwrap();
    assert!(material.is_local());
    let material = sf.read_object(material.path_id).unwrap();
    let tex_envs = material
        .pointer("m_SavedProperties/m_TexEnvs")
        .and_then(Value::as_array)
        .unwrap();
    let main_tex = tex_envs
        .iter()
        .find(|e| e.get("first").and_then(Value::as_str) == Some("_MainTex"))
        .and_then(|e| e.pointer("second/m_Texture"))
        .and_then(Value::as_pptr)
        .unwrap();
    let texture = sf.read_object(main_tex.path_id).unwrap();
    let tw = texture.get("m_Width").and_then(Value::as_i64).unwrap();
    let th = texture.get("m_Height").and_then(Value::as_i64).unwrap();
    assert_eq!((tw, th), (1024, 1024));

    let sprites = value.get("mSprites").and_then(Value::as_array).unwrap();
    assert_eq!(sprites.len(), 39);
    let first = sprites[0].as_map().unwrap();
    assert_eq!(
        first.keys().take(5).collect::<Vec<_>>(),
        ["name", "x", "y", "width", "height"],
        "fields keep TypeTree order"
    );
    assert_eq!(
        sprites[0].get("name").and_then(Value::as_str),
        Some("active_prereq_default")
    );
    for sprite in sprites {
        let int = |key: &str| sprite.get(key).and_then(Value::as_i64).unwrap();
        let name = sprite.get("name").and_then(Value::as_str).unwrap();
        assert_ne!(name, "");
        let (x, y, w, h) = (int("x"), int("y"), int("width"), int("height"));
        assert!(x >= 0 && y >= 0 && w > 0 && h > 0, "{name}");
        assert!(x + w <= tw && y + h <= th, "{name} outside the atlas");
        for key in [
            "borderLeft",
            "borderRight",
            "borderTop",
            "borderBottom",
            "paddingLeft",
            "paddingRight",
        ] {
            assert!(int(key) >= 0, "{name}.{key}");
        }
    }
    assert!(value.get("mPixelSize").and_then(Value::as_f64).is_some());
}

#[test]
fn atlas_material_shader_lives_in_a_dependency() {
    let Some(bytes) = common::fixture("territoryui_view_standard_atlas.bundle") else {
        return;
    };
    let bundle = Bundle::parse(&bytes).unwrap();
    let sf = bundle.serialized_file().unwrap();
    let material = sf
        .objects()
        .iter()
        .find(|o| o.type_name == "Material")
        .unwrap();
    let shader = sf
        .read_field(material.path_id, "m_Shader")
        .unwrap()
        .and_then(|v| v.as_pptr())
        .unwrap();
    assert_eq!(shader.file_id, 1);
    let external = sf.external(shader.file_id).unwrap();
    assert!(external.starts_with("archive:/CAB-"));
    assert_eq!(sf.external(0), None);
    assert_eq!(sf.external(2), None);

    // The dependency named by m_Dependencies holds that file as its CAB node.
    let ab = sf.asset_bundle().unwrap().unwrap();
    assert_eq!(ab.dependencies, ["shared_uicontainer.bundle"]);
    let Some(shared) = common::fixture("shared_uicontainer.bundle") else {
        return;
    };
    let shared = Bundle::parse(&shared).unwrap();
    let cab = ae3_unity::external_file_name(external);
    assert!(shared.node(cab).is_some(), "{cab}");
    let shared_sf = shared.serialized_file().unwrap();
    let shader_object = shared_sf.object(shader.path_id).unwrap();
    assert_eq!(shader_object.type_name, "Shader");
}

#[test]
fn shared_container_has_fourteen_ngui_atlases() {
    let Some(bytes) = common::fixture("shared_uicontainer.bundle") else {
        return;
    };
    let bundle = Bundle::parse(&bytes).unwrap();
    let sf = bundle.serialized_file().unwrap();
    let atlases = sf
        .objects_of_class(CLASS_MONO_BEHAVIOUR)
        .filter(|o| {
            sf.script_class(o.path_id)
                .as_ref()
                .and_then(ScriptClass::local_name)
                == Some("NGUIAtlas")
        })
        .count();
    assert_eq!(atlases, 14);
}

/// Probing from the start of the file must list exactly what a full parse
/// lists, stop well short of the file, and converge from the suggested
/// first fetch in a few rounds.
#[test]
fn probe_prefix_matches_full_parse() {
    // (fixture, upper bound on prefix bytes used)
    let cases = [
        ("charui_bossk.bundle", 2_000),
        ("abilityui_passive_summon.bundle", 2_000),
        ("vanity_bossk.bundle", 2_000),
        ("relic_rfl_bossk.bundle", 2_000),
        ("territoryui_view_standard_atlas.bundle", 5_000),
        ("unit_details_atlas.bundle", 5_000),
        ("shared_uicontainer.bundle", 40_000),
        ("char_bossk_pre.bundle", 170_000),
    ];
    for (name, bound) in cases {
        let Some(bytes) = common::fixture(name) else {
            continue;
        };
        let total = bytes.len() as u64;
        let bundle = Bundle::parse(&bytes).unwrap();
        let sf = bundle.serialized_file().unwrap();
        let expected: Vec<_> = sf
            .objects()
            .iter()
            .map(|o| {
                (
                    o.path_id,
                    o.class_id,
                    o.type_name.clone(),
                    sf.object_name(o.path_id),
                )
            })
            .collect();

        let Probe::Listed(full) = probe_prefix(&bytes, total).unwrap() else {
            panic!("{name}: whole file not enough");
        };
        let listed: Vec<_> = full
            .assets
            .iter()
            .map(|a| (a.path_id, a.class_id, a.type_name.clone(), a.name.clone()))
            .collect();
        assert_eq!(listed, expected, "{name}");
        assert_eq!(full.unity_version, "6000.3.17f1");
        let used = full.bytes_used;
        eprintln!("{name}: probe needs {used} of {total} bytes");
        assert!(used <= bound, "{name}: used {used} > {bound}");

        // bytes_used is sufficient on its own...
        let Probe::Listed(exact) =
            probe_prefix(&bytes[..usize::try_from(used).unwrap()], total).unwrap()
        else {
            panic!("{name}: bytes_used ({used}) is not enough");
        };
        assert_eq!(exact.assets, full.assets);
        // ...and tight to within a feed step.
        let shorter = usize::try_from(used).unwrap() - 300;
        assert!(
            matches!(
                probe_prefix(&bytes[..shorter], total).unwrap(),
                Probe::NeedMore { .. }
            ),
            "{name}: {shorter} bytes already suffice"
        );

        // Following the hints converges quickly and never asks past the file.
        let mut len = ae3_unity::SUGGESTED_FIRST_PROBE_LEN.min(total);
        let mut trace = vec![len];
        loop {
            match probe_prefix(&bytes[..usize::try_from(len).unwrap()], total).unwrap() {
                Probe::Listed(l) => {
                    assert_eq!(l.assets, full.assets);
                    break;
                }
                Probe::NeedMore { at_least } => {
                    assert!(
                        at_least > len && at_least <= total,
                        "{name}: {len} -> {at_least}"
                    );
                    len = at_least;
                    trace.push(len);
                }
            }
        }
        eprintln!("{name}: fetches {trace:?}");
        assert!(trace.len() <= 4, "{name}: {trace:?}");
    }
}

#[test]
fn probe_never_needs_the_resource_node() {
    let Some(bytes) = common::fixture("unit_details_atlas.bundle") else {
        return;
    };
    let bundle = Bundle::parse(&bytes).unwrap();
    let Probe::Listed(listing) = probe_prefix(&bytes, bytes.len() as u64).unwrap() else {
        panic!("not listed");
    };
    // The .resS node is 4 MiB of RGBA32 pixels, about 98% of the compressed file.
    assert!(listing.bytes_used * 20 < bytes.len() as u64);
    assert!(bundle.resource_nodes().all(|n| n.size > 1 << 20));
}

#[test]
fn truncated_bundles_are_errors() {
    let Some(bytes) = common::fixture("charui_bossk.bundle") else {
        return;
    };
    let header_end = 0x30;
    for len in [
        0,
        1,
        7,
        8,
        20,
        header_end,
        120,
        200,
        1_000,
        3_000,
        bytes.len() - 1,
    ] {
        assert!(
            Bundle::parse(&bytes[..len]).is_err(),
            "{len}-byte prefix parsed"
        );
    }
    // Corrupting any header or blocks-info byte is an error or a different
    // parse, never a panic.
    let mut corrupt = bytes.clone();
    for pos in 0..200 {
        let original = corrupt[pos];
        corrupt[pos] = original ^ 0x5A;
        let _ = Bundle::parse(&corrupt);
        let _ = probe_prefix(&corrupt, corrupt.len() as u64);
        corrupt[pos] = original;
    }
}

#[test]
fn truncated_lzma_data_is_an_error_not_a_short_read() {
    let Some(bytes) = common::fixture("unit_details_atlas.bundle") else {
        return;
    };
    let cut = &bytes[..bytes.len() / 2];
    // A full parse needs every block.
    assert!(Bundle::parse(cut).is_err());
    // A probe of the same prefix with the real length succeeds...
    assert!(matches!(
        probe_prefix(cut, bytes.len() as u64).unwrap(),
        Probe::Listed(_)
    ));
    // ...but claiming the prefix is the whole file is malformed.
    assert!(probe_prefix(&bytes[..2_000], 2_000).is_err());
}

#[test]
fn pptr_helpers() {
    let p = PPtr {
        file_id: 0,
        path_id: 0,
    };
    assert!(p.is_null() && p.is_local());
}
