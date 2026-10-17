//! glb output against the real Asset Version 100054 fixtures. Every glb
//! must parse and validate with the `gltf` crate and hold the same
//! geometry as the OBJ of the same Mesh: the same vertices, UVs (V
//! flipped), normals and faces, in the same order. Skips when the fixtures
//! are absent.

#![allow(
    clippy::cast_possible_truncation,
    clippy::float_cmp,
    reason = "fixture counts are small, and positions must round-trip exactly"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ae3_core::config::{MeshFormat, StructuredMode};
use ae3_core::{AssetKind, DirectoryDependencies, ExportOptions, LoadedBundle, export_bundle};

fn fixtures_dir() -> PathBuf {
    std::env::var_os("AE3_FIXTURES")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        )
        .join("100054/Windows")
}

/// The fixture Bundle, resolving references into the other fixtures, or
/// `None` (skip) when it is absent.
fn fixture(name: &str) -> Option<LoadedBundle> {
    let dir = fixtures_dir();
    let path = dir.join(format!("{name}.bundle"));
    if !path.exists() {
        eprintln!("skipping: fixture {} not found", path.display());
        return None;
    }
    let bundle = LoadedBundle::open_file(&path).unwrap();
    let dependencies = bundle.dependencies().to_vec();
    Some(bundle.with_dependencies(dependencies, Arc::new(DirectoryDependencies::new(dir))))
}

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ae3-gltf-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An OBJ file's numbers: `v`, `vt`, `vn` and 0-based face indices.
#[derive(Default)]
struct Obj {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    faces: Vec<u32>,
}

fn parse_obj(text: &str) -> Obj {
    let mut obj = Obj::default();
    let floats = |rest: &str| -> Vec<f32> { rest.split(' ').map(|f| f.parse().unwrap()).collect() };
    for line in text.lines() {
        let (tag, rest) = line.split_once(' ').unwrap();
        match tag {
            "v" => obj.positions.push(floats(rest).try_into().unwrap()),
            "vt" => obj.uvs.push(floats(rest).try_into().unwrap()),
            "vn" => obj.normals.push(floats(rest).try_into().unwrap()),
            "f" => obj.faces.extend(
                rest.split(' ')
                    .map(|corner| corner.split('/').next().unwrap().parse::<u32>().unwrap() - 1),
            ),
            _ => {}
        }
    }
    obj
}

/// What a glb holds, read through the `gltf` crate (which validates it).
struct Glb {
    document: gltf::Document,
    blob: Option<Vec<u8>>,
}

fn parse_glb(bytes: &[u8]) -> Glb {
    let gltf = gltf::Gltf::from_slice(bytes).unwrap_or_else(|e| panic!("invalid glb: {e}"));
    Glb {
        document: gltf.document,
        blob: gltf.blob,
    }
}

/// Checks that `glb` holds the geometry of `obj`, and returns the number
/// of triangles.
fn check_against_obj(name: &str, obj: &Obj, glb: &Glb) -> usize {
    let mut meshes = glb.document.meshes();
    let Some(mesh) = meshes.next() else {
        assert!(obj.positions.is_empty(), "{name}: glb has no mesh");
        return 0;
    };
    assert!(meshes.next().is_none());
    let blob = glb.blob.as_deref();
    let mut faces = Vec::new();
    for primitive in mesh.primitives() {
        let reader = primitive.reader(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => blob,
            gltf::buffer::Source::Uri(_) => None,
        });
        let positions: Vec<[f32; 3]> = reader.read_positions().unwrap().collect();
        assert_eq!(positions, obj.positions, "{name}: positions");
        let uvs: Vec<[f32; 2]> = reader
            .read_tex_coords(0)
            .map(|t| t.into_f32().collect())
            .unwrap_or_default();
        assert_eq!(uvs.len(), obj.uvs.len(), "{name}: UVs");
        for (g, o) in uvs.iter().zip(&obj.uvs) {
            assert!(
                g[0] == o[0] && (g[1] - (1.0 - o[1])).abs() < 1e-6,
                "{name}: UV {g:?} vs {o:?}"
            );
        }
        let normals: Vec<[f32; 3]> = reader
            .read_normals()
            .map(Iterator::collect)
            .unwrap_or_default();
        assert_eq!(normals.len(), obj.normals.len(), "{name}: normals");
        for (g, o) in normals.iter().zip(&obj.normals) {
            let len = o.iter().map(|c| c * c).sum::<f32>().sqrt();
            assert!(
                (0..3).all(|i| (g[i] - o[i] / len).abs() < 1e-5),
                "{name}: normal {g:?} vs {o:?}"
            );
        }
        if let Some(indices) = reader.read_indices() {
            assert_eq!(primitive.mode(), gltf::mesh::Mode::Triangles);
            faces.extend(indices.into_u32());
        }
    }
    assert_eq!(faces, obj.faces, "{name}: faces");
    faces.len() / 3
}

/// The PNG an image of the glb embeds, as (width, height).
fn embedded_png_size(glb: &Glb, image: &gltf::Image<'_>) -> (u32, u32) {
    let gltf::image::Source::View { view, mime_type } = image.source() else {
        panic!("image not embedded");
    };
    assert_eq!(mime_type, "image/png");
    let blob = glb.blob.as_deref().unwrap();
    let png = &blob[view.offset()..view.offset() + view.length()];
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    (be(16), be(20))
}

#[test]
fn bossk_glb_matches_obj_and_embeds_his_texture() {
    let Some(bundle) = fixture("char_bossk_pre") else {
        return;
    };
    let out = TempDir::new("bossk");
    let options = ExportOptions {
        mesh_format: MeshFormat::Both,
        structured: StructuredMode::None,
        parallel: 1,
        ..ExportOptions::new(out.path())
    };
    let report = export_bundle(&bundle, None, &options, &|_| {}, &AtomicBool::new(false));
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    let dir = out.path().join("char/char_bossk_pre");

    let mut checked = 0;
    for name in ["bossk_mesh", "Bossk_weapon", "vfx_net_gun_01"] {
        let obj = parse_obj(&fs::read_to_string(dir.join(format!("{name}.obj"))).unwrap());
        let glb = parse_glb(&fs::read(dir.join(format!("{name}.glb"))).unwrap());
        let triangles = check_against_obj(name, &obj, &glb);
        assert_eq!(triangles, obj.faces.len() / 3);
        assert_eq!(glb.document.nodes().next().unwrap().name(), Some(name));
        checked += 1;
    }
    assert_eq!(checked, 3);

    // Bossk: 1693 vertices with normals and UVs, 2502 triangles, drawn with
    // bossk_mat and its 256x256 bossk_tex.
    let glb = parse_glb(&fs::read(dir.join("bossk_mesh.glb")).unwrap());
    let obj = parse_obj(&fs::read_to_string(dir.join("bossk_mesh.obj")).unwrap());
    assert_eq!(
        (obj.positions.len(), obj.uvs.len(), obj.normals.len()),
        (1693, 1693, 1693)
    );
    assert_eq!(obj.faces.len(), 2502 * 3);
    let material = glb.document.materials().next().unwrap();
    assert_eq!(material.name(), Some("bossk_mat"));
    let texture = material
        .pbr_metallic_roughness()
        .base_color_texture()
        .unwrap()
        .texture();
    assert_eq!(embedded_png_size(&glb, &texture.source()), (256, 256));
    let primitive = glb
        .document
        .meshes()
        .next()
        .unwrap()
        .primitives()
        .next()
        .unwrap();
    assert_eq!(primitive.material().name(), Some("bossk_mat"));

    // The weapon has two renderers; the one with a Material in this Bundle
    // wins: rfl_bossk_mat and its 128x128 rfl_bossk_tex.
    let glb = parse_glb(&fs::read(dir.join("Bossk_weapon.glb")).unwrap());
    let material = glb.document.materials().next().unwrap();
    assert_eq!(material.name(), Some("rfl_bossk_mat"));
    assert_eq!(glb.document.images().count(), 1);
    let image = glb.document.images().next().unwrap();
    assert_eq!(embedded_png_size(&glb, &image), (128, 128));

    // Both formats are reported, and only untextured glbs carry a note.
    for name in ["bossk_mesh", "Bossk_weapon", "vfx_net_gun_01"] {
        for ext in ["obj", "glb"] {
            let path = dir.join(format!("{name}.{ext}"));
            let written = report.written.iter().find(|w| w.path == path);
            let written = written.unwrap_or_else(|| panic!("{name}.{ext} not reported"));
            assert_eq!(written.bytes, fs::metadata(&path).unwrap().len());
        }
    }
    let note = |name: &str| {
        let path = dir.join(format!("{name}.glb"));
        report
            .written
            .iter()
            .find(|w| w.path == path)
            .unwrap()
            .note
            .clone()
    };
    assert_eq!(note("bossk_mesh"), None);
    assert_eq!(note("Bossk_weapon"), None);
    let vfx = glb_note_or_texture(&dir.join("vfx_net_gun_01.glb"), note("vfx_net_gun_01"));
    assert!(vfx, "an untextured glb says why");
}

/// Whether a glb either embeds a texture or its report note says why not.
fn glb_note_or_texture(path: &Path, note: Option<String>) -> bool {
    let glb = parse_glb(&fs::read(path).unwrap());
    match note {
        None => glb.document.images().count() > 0,
        Some(note) => note.starts_with("geometry only: ") && glb.document.images().count() == 0,
    }
}

#[test]
fn default_mesh_format_writes_obj_only() {
    let Some(bundle) = fixture("char_bossk_pre") else {
        return;
    };
    let mesh = bundle
        .assets()
        .unwrap()
        .into_iter()
        .find(|a| a.kind == AssetKind::Mesh)
        .unwrap();
    for (format, obj, glb) in [
        (MeshFormat::Obj, true, false),
        (MeshFormat::Glb, false, true),
    ] {
        let out = TempDir::new("formats");
        let options = ExportOptions {
            mesh_format: format,
            ..ExportOptions::new(out.path())
        };
        let report = export_bundle(
            &bundle,
            Some(&[mesh.path_id]),
            &options,
            &|_| {},
            &AtomicBool::new(false),
        );
        assert!(report.errors.is_empty(), "{:#?}", report.errors);
        assert_eq!(report.written.len(), 1);
        let file = |ext: &str| {
            out.path()
                .join(format!("char/char_bossk_pre/bossk_mesh.{ext}"))
        };
        assert_eq!((file("obj").exists(), file("glb").exists()), (obj, glb));
    }
    assert_eq!(ExportOptions::default().mesh_format, MeshFormat::Obj);
}

#[test]
fn every_fixture_mesh_round_trips_through_glb() {
    let mut meshes = 0;
    for name in [
        "char_bossk_pre",
        "shared_resourcecontainer",
        "env_tat_dunes_jawa_pre",
        "env_space_crait_01_pre",
    ] {
        let Some(bundle) = fixture(name) else {
            continue;
        };
        let view = bundle.view().unwrap();
        let ids: Vec<i64> = view
            .assets()
            .iter()
            .filter(|a| a.kind == AssetKind::Mesh)
            .map(|a| a.path_id)
            .collect();
        for &id in &ids {
            let mesh = view.mesh(id).unwrap();
            let obj = parse_obj(&mesh.to_obj());
            check_against_obj(&mesh.name, &obj, &parse_glb(&mesh.to_glb(&[])));
            meshes += 1;
        }

        // Exported with textures where the fixtures resolve them; every glb
        // validates and every untextured one says why.
        let out = TempDir::new("all");
        let options = ExportOptions {
            mesh_format: MeshFormat::Glb,
            parallel: 1,
            ..ExportOptions::new(out.path())
        };
        let report = export_bundle(
            &bundle,
            Some(&ids),
            &options,
            &|_| {},
            &AtomicBool::new(false),
        );
        assert!(report.errors.is_empty(), "{name}: {:#?}", report.errors);
        assert_eq!(report.written.len(), ids.len(), "{name}");
        for w in &report.written {
            assert!(
                glb_note_or_texture(&w.path, w.note.clone()),
                "{name}: {w:?}"
            );
        }
        let textured = report.written.iter().filter(|w| w.note.is_none()).count();
        eprintln!("{name}: {} glbs, {textured} textured", ids.len());
    }
    if meshes > 0 {
        assert!(meshes >= 3, "{meshes} meshes");
    }
}
