//! Mesh decoding against the real Asset Version 100054 fixtures. Unity
//! stores each Mesh's bounding box beside its geometry, computed before
//! quantization (and padded slightly), so decoded positions must fill that
//! box to within two quantization steps. Skips when the fixtures are absent.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "fixture counts and coordinates are small"
)]

use std::path::PathBuf;

use ae3_core::{AssetKind, LoadedBundle, Mesh, Value};

fn fixture(file: &str) -> Option<LoadedBundle> {
    let path = std::env::var_os("AE3_FIXTURES")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        )
        .join("100054/Windows")
        .join(file);
    if !path.exists() {
        eprintln!("skipping: fixture {} not found", path.display());
        return None;
    }
    Some(LoadedBundle::open_file(&path).unwrap())
}

fn f32_at(value: &Value, path: &str) -> f32 {
    value.pointer(path).and_then(Value::as_f64).unwrap() as f32
}

/// Checks a decoded Mesh against the counts and `m_LocalAABB` its value
/// declares.
fn check(mesh: &Mesh, value: &Value) {
    let name = &mesh.name;
    let n = mesh.positions.len();
    assert!(n > 0, "{name}: no vertices");
    for (what, len) in [("normals", mesh.normals.len()), ("uvs", mesh.uvs.len())] {
        assert!(
            len == 0 || len == n,
            "{name}: {len} {what} for {n} vertices"
        );
    }
    let indices: usize = value
        .get("m_SubMeshes")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|s| s.get("indexCount").and_then(Value::as_u64).unwrap() as usize)
        .sum();
    assert_eq!(mesh.triangle_count() * 3, indices, "{name}: triangles");

    // One quantization step of the positions, when compressed.
    let packed = |key: &str| value.pointer(&format!("m_CompressedMesh/m_Vertices/{key}"));
    let items = packed("m_NumItems").and_then(Value::as_u64).unwrap_or(0);
    let bits = packed("m_BitSize").and_then(Value::as_u64).unwrap_or(0);
    let step = if items > 0 && bits > 0 {
        f32_at(value, "m_CompressedMesh/m_Vertices/m_Range") / ((1u64 << bits) - 1) as f32
    } else {
        0.0
    };
    let (min, max) = mesh.bounds().unwrap();
    for (i, axis) in ["x", "y", "z"].iter().enumerate() {
        let center = f32_at(value, &format!("m_LocalAABB/m_Center/{axis}"));
        let extent = f32_at(value, &format!("m_LocalAABB/m_Extent/{axis}"));
        let eps = 2.0 * step + 1e-5 * (center.abs() + extent).max(1.0);
        assert!(
            (min[i] - (center - extent)).abs() <= eps && (max[i] - (center + extent)).abs() <= eps,
            "{name}: {axis} spans {}..{}, AABB {}..{} (eps {eps})",
            min[i],
            max[i],
            center - extent,
            center + extent
        );
    }
    for normal in &mesh.normals {
        let len = normal.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((len - 1.0).abs() < 0.02, "{name}: normal {normal:?}");
    }
}

#[test]
fn decodes_bossk_compressed_meshes() {
    let Some(bundle) = fixture("char_bossk_pre.bundle") else {
        return;
    };
    let view = bundle.view().unwrap();
    let meshes: Vec<_> = view
        .assets()
        .into_iter()
        .filter(|a| a.kind == AssetKind::Mesh)
        .collect();
    let names: Vec<&str> = meshes.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["bossk_mesh", "Bossk_weapon", "vfx_net_gun_01"]);
    for asset in &meshes {
        let mesh = view.mesh(asset.path_id).unwrap();
        check(&mesh, &view.structured(asset.path_id).unwrap());
    }
    let bossk = view.mesh(meshes[0].path_id).unwrap();
    assert_eq!(bossk.name, "bossk_mesh");
    assert_eq!(bossk.positions.len(), 1693);
    assert_eq!((bossk.normals.len(), bossk.uvs.len()), (1693, 1693));
    assert_eq!(bossk.submeshes.len(), 1);
    assert_eq!(bossk.triangle_count(), 2502);
    assert!(
        bossk.uvs.iter().flatten().all(|c| (0.0..=1.0).contains(c)),
        "Bossk's UVs stay within the texture"
    );
    // Only Meshes decode as Meshes.
    let texture = view
        .assets()
        .into_iter()
        .find(|a| a.kind == AssetKind::Texture)
        .unwrap();
    assert!(view.mesh(texture.path_id).is_err());
}

#[test]
fn decodes_every_shared_mesh_including_uncompressed_vertex_data() {
    let Some(bundle) = fixture("shared_resourcecontainer.bundle") else {
        return;
    };
    let view = bundle.view().unwrap();
    let mut decoded = 0;
    for asset in view
        .assets()
        .into_iter()
        .filter(|a| a.kind == AssetKind::Mesh)
    {
        let mesh = view
            .mesh(asset.path_id)
            .unwrap_or_else(|e| panic!("{}: {e}", asset.name));
        check(&mesh, &view.structured(asset.path_id).unwrap());
        decoded += 1;
    }
    assert!(decoded > 70, "{decoded} meshes");

    // The one readable Mesh: plain Float32 vertex data in one stream.
    let hose = view
        .assets()
        .into_iter()
        .find(|a| a.name == "combattrench_01_hose_01_mesh")
        .unwrap();
    let value = view.structured(hose.path_id).unwrap();
    assert_eq!(
        value
            .pointer("m_VertexData/m_VertexCount")
            .and_then(Value::as_u64),
        Some(272)
    );
    let mesh = view.mesh(hose.path_id).unwrap();
    assert_eq!(mesh.positions.len(), 272);
    assert_eq!((mesh.normals.len(), mesh.uvs.len()), (272, 272));
    assert_eq!(mesh.triangle_count(), 204);
}
