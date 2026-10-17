//! Binary glTF 2.0 (`.glb`) output for Meshes.
//!
//! A `.glb` holds one scene with one node and, unless the Mesh has no
//! vertices, one mesh: a triangle primitive per non-empty submesh, sharing
//! the vertex attributes (`POSITION`, `NORMAL`, `TEXCOORD_0`). Each
//! submesh's Material can carry its main texture, embedded as PNG.
//!
//! Space is converted as for OBJ: X is negated to turn Unity's left-handed
//! space into glTF's right-handed one, and each triangle's winding is
//! reversed so front faces stay counter-clockwise. V is flipped
//! (`1 - v`) because glTF puts the texture origin at the top left, where
//! Unity puts it at the bottom left. Non-finite numbers are written as 0,
//! and normals are normalized as glTF requires.

use serde::Serialize;

use crate::mesh::Mesh;

/// `glTF`, little-endian.
const MAGIC: u32 = 0x4654_6C67;
const VERSION: u32 = 2;
/// `JSON`, little-endian.
const CHUNK_JSON: u32 = 0x4E4F_534A;
/// `BIN\0`, little-endian.
const CHUNK_BIN: u32 = 0x004E_4942;
const HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;

const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const MODE_POINTS: u32 = 0;
const MODE_TRIANGLES: u32 = 4;
/// Vertex counts up to this fit 16-bit indices; glTF reserves the largest
/// value of an index type (primitive restart).
const MAX_U16_VERTICES: usize = u16::MAX as usize;

/// What one submesh is drawn with in a `.glb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlbMaterial<'a> {
    pub name: &'a str,
    /// The base colour texture as PNG bytes, embedded in the file.
    pub png: Option<&'a [u8]>,
}

impl Mesh {
    /// The Mesh as binary glTF. `materials[i]` is what submesh `i` is drawn
    /// with; submeshes past its end, or with `None`, get no material. Equal
    /// Materials and PNGs are stored once.
    #[must_use]
    pub fn to_glb(&self, materials: &[Option<GlbMaterial<'_>>]) -> Vec<u8> {
        let mut doc = Builder::default();
        let mut root = Root {
            asset: AssetJson {
                version: "2.0",
                generator: concat!("AE3 ", env!("CARGO_PKG_VERSION")),
            },
            scene: 0,
            scenes: [SceneJson { nodes: [0] }],
            nodes: [NodeJson {
                name: &self.name,
                mesh: None,
            }],
            ..Root::default()
        };
        if !self.positions.is_empty() {
            let primitives = self.primitives(&mut doc, &mut root, materials);
            root.nodes[0].mesh = Some(0);
            root.meshes.push(MeshJson {
                name: &self.name,
                primitives,
            });
        }
        let bin = doc.finish(&mut root);
        // Serializing plain structs of strings and numbers cannot fail.
        let json = serde_json::to_vec(&root).unwrap_or_default();
        glb(json, &bin)
    }

    /// Writes the vertex attributes and indices, and returns the primitives.
    fn primitives<'a>(
        &self,
        doc: &mut Builder,
        root: &mut Root<'a>,
        materials: &[Option<GlbMaterial<'a>>],
    ) -> Vec<PrimitiveJson> {
        let vertex_count = self.positions.len();
        let positions: Vec<[f32; 3]> = self
            .positions
            .iter()
            .map(|&[x, y, z]| [finite(-x), finite(y), finite(z)])
            .collect();
        let (min, max) = positions.iter().fold(
            ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
            |(mut min, mut max), p| {
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
                (min, max)
            },
        );
        let position = doc.floats(positions.into_iter(), "VEC3", Some((min, max)));
        let normal = (self.normals.len() == vertex_count).then(|| {
            let normals = self
                .normals
                .iter()
                .map(|&[x, y, z]| unit([finite(-x), finite(y), finite(z)]));
            doc.floats(normals, "VEC3", None)
        });
        let uv = (self.uvs.len() == vertex_count).then(|| {
            let uvs = self.uvs.iter().map(|&[u, v]| [finite(u), 1.0 - finite(v)]);
            doc.floats(uvs, "VEC2", None)
        });
        let attributes = AttributesJson {
            position,
            normal,
            uv,
        };

        let wide = vertex_count > MAX_U16_VERTICES;
        let drawn: Vec<(usize, &[[u32; 3]])> = self
            .submeshes
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.triangles.is_empty())
            .map(|(i, s)| (i, s.triangles.as_slice()))
            .collect();
        if drawn.is_empty() {
            return vec![PrimitiveJson {
                attributes,
                indices: None,
                material: None,
                mode: MODE_POINTS,
            }];
        }
        let index_counts: Vec<usize> = drawn.iter().map(|(_, t)| t.len() * 3).collect();
        let (view, offsets) = doc.indices(drawn.iter().map(|(_, t)| *t), wide);
        let mut materials_done = Materials::default();
        drawn
            .iter()
            .zip(index_counts.iter().zip(offsets))
            .map(|(&(i, _), (&count, offset))| {
                let indices = doc.accessor(AccessorJson {
                    buffer_view: view,
                    byte_offset: offset,
                    component_type: if wide { UNSIGNED_INT } else { UNSIGNED_SHORT },
                    count,
                    kind: "SCALAR",
                    min: None,
                    max: None,
                });
                let material = materials
                    .get(i)
                    .copied()
                    .flatten()
                    .map(|m| materials_done.index(doc, root, m));
                PrimitiveJson {
                    attributes,
                    indices: Some(indices),
                    material,
                    mode: MODE_TRIANGLES,
                }
            })
            .collect()
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// `v` scaled to length 1, or +Z when it has no direction.
fn unit(v: [f32; 3]) -> [f32; 3] {
    let len = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > f32::EPSILON && len.is_finite() {
        v.map(|c| c / len)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// The Materials and images written so far, so equal ones are stored once.
#[derive(Default)]
struct Materials<'a> {
    materials: Vec<GlbMaterial<'a>>,
    images: Vec<&'a [u8]>,
}

impl<'a> Materials<'a> {
    fn index(&mut self, doc: &mut Builder, root: &mut Root<'a>, m: GlbMaterial<'a>) -> usize {
        if let Some(i) = self
            .materials
            .iter()
            .position(|&done| same_material(done, m))
        {
            return i;
        }
        let texture = m.png.map(|png| {
            self.images
                .iter()
                .position(|&done| same_bytes(done, png))
                .unwrap_or_else(|| {
                    let view = doc.view(png, None);
                    root.images.push(ImageJson {
                        buffer_view: view,
                        mime_type: "image/png",
                    });
                    root.textures.push(TextureJson {
                        source: self.images.len(),
                    });
                    self.images.push(png);
                    self.images.len() - 1
                })
        });
        root.materials.push(MaterialJson {
            name: m.name,
            pbr_metallic_roughness: PbrJson {
                base_color_texture: texture.map(|index| TextureRefJson { index }),
                // Unity's toon shaders are not metallic; glTF's default is.
                metallic_factor: 0.0,
            },
        });
        self.materials.push(m);
        self.materials.len() - 1
    }
}

fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    std::ptr::eq(a, b) || a == b
}

fn same_material(a: GlbMaterial<'_>, b: GlbMaterial<'_>) -> bool {
    a.name == b.name
        && match (a.png, b.png) {
            (Some(a), Some(b)) => same_bytes(a, b),
            (None, None) => true,
            _ => false,
        }
}

/// The binary buffer and the JSON objects that describe it.
#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<BufferViewJson>,
    accessors: Vec<AccessorJson>,
}

impl Builder {
    /// Appends `bytes` as a buffer view, starting on a 4-byte boundary.
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.view_with(target, |bin| bin.extend_from_slice(bytes))
    }

    /// Appends what `write` writes as a buffer view, starting on a 4-byte
    /// boundary.
    fn view_with(&mut self, target: Option<u32>, write: impl FnOnce(&mut Vec<u8>)) -> usize {
        self.align();
        let byte_offset = self.bin.len();
        write(&mut self.bin);
        self.views.push(BufferViewJson {
            buffer: 0,
            byte_offset,
            byte_length: self.bin.len() - byte_offset,
            target,
        });
        self.views.len() - 1
    }

    fn align(&mut self) {
        self.bin.resize(self.bin.len().next_multiple_of(4), 0);
    }

    fn accessor(&mut self, accessor: AccessorJson) -> usize {
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    /// A vertex attribute of `N` floats per vertex, as its own view.
    fn floats<const N: usize>(
        &mut self,
        values: impl ExactSizeIterator<Item = [f32; N]>,
        kind: &'static str,
        bounds: Option<([f32; N], [f32; N])>,
    ) -> usize {
        let count = values.len();
        let view = self.view_with(Some(ARRAY_BUFFER), |bin| {
            bin.reserve(count * N * 4);
            for value in values {
                for f in value {
                    bin.extend_from_slice(&f.to_le_bytes());
                }
            }
        });
        let (min, max) = bounds.map_or((None, None), |(min, max)| {
            (Some(min.to_vec()), Some(max.to_vec()))
        });
        self.accessor(AccessorJson {
            buffer_view: view,
            byte_offset: 0,
            component_type: FLOAT,
            count,
            kind,
            min,
            max,
        })
    }

    /// Every submesh's indices in one view, windings reversed; returns the
    /// view and each submesh's byte offset within it.
    fn indices<'t>(
        &mut self,
        submeshes: impl Iterator<Item = &'t [[u32; 3]]>,
        wide: bool,
    ) -> (usize, Vec<usize>) {
        let mut offsets = Vec::new();
        let view = self.view_with(Some(ELEMENT_ARRAY_BUFFER), |bin| {
            let start = bin.len();
            for triangles in submeshes {
                offsets.push(bin.len() - start);
                for &[a, b, c] in triangles {
                    for index in [c, b, a] {
                        if wide {
                            bin.extend_from_slice(&index.to_le_bytes());
                        } else {
                            // Every index is below the vertex count, which fits.
                            let narrow = u16::try_from(index).unwrap_or(u16::MAX - 1);
                            bin.extend_from_slice(&narrow.to_le_bytes());
                        }
                    }
                }
            }
        });
        (view, offsets)
    }

    /// Moves the views and accessors into `root` and returns the padded
    /// binary chunk, empty when nothing was written.
    fn finish(mut self, root: &mut Root<'_>) -> Vec<u8> {
        self.align();
        if !self.bin.is_empty() {
            root.buffers.push(BufferJson {
                byte_length: self.bin.len(),
            });
        }
        root.buffer_views = self.views;
        root.accessors = self.accessors;
        self.bin
    }
}

/// The GLB container: header, JSON chunk (space-padded), BIN chunk.
fn glb(mut json: Vec<u8>, bin: &[u8]) -> Vec<u8> {
    json.resize(json.len().next_multiple_of(4), b' ');
    let bin_chunk = if bin.is_empty() {
        0
    } else {
        CHUNK_HEADER_LEN + bin.len()
    };
    let total = HEADER_LEN + CHUNK_HEADER_LEN + json.len() + bin_chunk;
    let mut out = Vec::with_capacity(total);
    let word = |out: &mut Vec<u8>, n: usize| {
        // A glb is limited to 4 GiB; Meshes are far smaller.
        out.extend_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());
    };
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    word(&mut out, total);
    word(&mut out, json.len());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json);
    if !bin.is_empty() {
        word(&mut out, bin.len());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(bin);
    }
    out
}

// --- The glTF JSON, only the parts AE3 writes ---

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Root<'a> {
    asset: AssetJson,
    scene: usize,
    scenes: [SceneJson; 1],
    nodes: [NodeJson<'a>; 1],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    meshes: Vec<MeshJson<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    materials: Vec<MaterialJson<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    textures: Vec<TextureJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<ImageJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    accessors: Vec<AccessorJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    buffer_views: Vec<BufferViewJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    buffers: Vec<BufferJson>,
}

#[derive(Serialize, Default)]
struct AssetJson {
    version: &'static str,
    generator: &'static str,
}

#[derive(Serialize, Default)]
struct SceneJson {
    nodes: [usize; 1],
}

#[derive(Serialize, Default)]
struct NodeJson<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mesh: Option<usize>,
}

#[derive(Serialize)]
struct MeshJson<'a> {
    name: &'a str,
    primitives: Vec<PrimitiveJson>,
}

#[derive(Serialize)]
struct PrimitiveJson {
    attributes: AttributesJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    indices: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    material: Option<usize>,
    mode: u32,
}

#[derive(Serialize, Clone, Copy)]
struct AttributesJson {
    #[serde(rename = "POSITION")]
    position: usize,
    #[serde(rename = "NORMAL", skip_serializing_if = "Option::is_none")]
    normal: Option<usize>,
    #[serde(rename = "TEXCOORD_0", skip_serializing_if = "Option::is_none")]
    uv: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterialJson<'a> {
    name: &'a str,
    pbr_metallic_roughness: PbrJson,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PbrJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    base_color_texture: Option<TextureRefJson>,
    metallic_factor: f32,
}

#[derive(Serialize)]
struct TextureRefJson {
    index: usize,
}

#[derive(Serialize)]
struct TextureJson {
    source: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageJson {
    buffer_view: usize,
    mime_type: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccessorJson {
    buffer_view: usize,
    #[serde(skip_serializing_if = "is_zero")]
    byte_offset: usize,
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<Vec<f32>>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BufferViewJson {
    buffer: usize,
    byte_offset: usize,
    byte_length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BufferJson {
    byte_length: usize,
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "test glbs are tiny, so their offsets fit any usize"
)]
mod tests {
    use serde_json::Value as Json;

    use super::*;
    use crate::mesh::SubMesh;

    /// Splits a glb into its JSON and binary chunk, checking the framing.
    fn parse(glb: &[u8]) -> (Json, Vec<u8>) {
        let word = |at: usize| u32::from_le_bytes(glb[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(word(0), MAGIC as usize);
        assert_eq!(word(4), 2);
        assert_eq!(word(8), glb.len());
        let json_len = word(12);
        assert_eq!(word(16), CHUNK_JSON as usize);
        assert_eq!(json_len % 4, 0);
        let json = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        let rest = &glb[20 + json_len..];
        if rest.is_empty() {
            return (json, Vec::new());
        }
        let bin_len = word(20 + json_len);
        assert_eq!(word(24 + json_len), CHUNK_BIN as usize);
        assert_eq!(rest.len(), 8 + bin_len);
        (json, rest[8..].to_vec())
    }

    fn quad() -> Mesh {
        Mesh {
            name: "quad".into(),
            positions: vec![
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [f32::NAN, 2.0, 0.0],
            ],
            normals: vec![
                [2.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, -1.0],
            ],
            uvs: vec![[0.0, 0.0], [1.0, 0.25], [0.5, 1.0], [0.0, 1.0]],
            submeshes: vec![
                SubMesh {
                    triangles: vec![[0, 1, 2]],
                },
                SubMesh::default(),
                SubMesh {
                    triangles: vec![[1, 2, 3]],
                },
            ],
        }
    }

    fn floats(bin: &[u8], json: &Json, accessor: usize) -> Vec<f32> {
        let a = &json["accessors"][accessor];
        let view = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let start = view["byteOffset"].as_u64().unwrap() as usize;
        let len = view["byteLength"].as_u64().unwrap() as usize;
        bin[start..start + len]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| f32::from_le_bytes(c))
            .collect()
    }

    #[test]
    fn converts_space_winding_and_uvs() {
        let (json, bin) = parse(&quad().to_glb(&[]));
        assert_eq!(json["asset"]["version"], "2.0");
        assert_eq!(json["nodes"][0]["name"], "quad");
        let prims = json["meshes"][0]["primitives"].as_array().unwrap();
        // The empty submesh has no primitive.
        assert_eq!(prims.len(), 2);
        assert!(prims.iter().all(|p| p.get("material").is_none()));
        let attrs = &prims[0]["attributes"];
        let pos = floats(&bin, &json, attrs["POSITION"].as_u64().unwrap() as usize);
        assert_eq!(&pos[..3], &[-1.0, 0.0, 0.0]);
        assert_eq!(&pos[9..12], &[0.0, 2.0, 0.0], "NaN becomes 0");
        let accessor = &json["accessors"][attrs["POSITION"].as_u64().unwrap() as usize];
        assert_eq!(accessor["min"], serde_json::json!([-1.0, 0.0, 0.0]));
        assert_eq!(accessor["max"], serde_json::json!([0.0, 2.0, 1.0]));
        let normals = floats(&bin, &json, attrs["NORMAL"].as_u64().unwrap() as usize);
        assert_eq!(&normals[..6], &[-1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        let uvs = floats(&bin, &json, attrs["TEXCOORD_0"].as_u64().unwrap() as usize);
        assert_eq!(&uvs[..4], &[0.0, 1.0, 1.0, 0.75]);
        // 16-bit indices, windings reversed, the second submesh after the first.
        let second = &json["accessors"][prims[1]["indices"].as_u64().unwrap() as usize];
        assert_eq!(second["componentType"], UNSIGNED_SHORT);
        assert_eq!(second["byteOffset"], 6);
        let view = &json["bufferViews"][second["bufferView"].as_u64().unwrap() as usize];
        let start = view["byteOffset"].as_u64().unwrap() as usize;
        let indices: Vec<u16> = bin[start..start + 12]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        assert_eq!(indices, [2, 1, 0, 3, 2, 1]);
        assert_eq!(
            json["buffers"][0]["byteLength"].as_u64().unwrap() as usize,
            bin.len()
        );
    }

    #[test]
    fn embeds_each_png_and_material_once() {
        let png: &[u8] = b"\x89PNG fake";
        let skin = GlbMaterial {
            name: "skin",
            png: Some(png),
        };
        let copy = png.to_vec();
        let same = GlbMaterial {
            name: "skin",
            png: Some(&copy),
        };
        let (json, bin) = parse(&quad().to_glb(&[Some(skin), None, Some(same)]));
        let prims = json["meshes"][0]["primitives"].as_array().unwrap();
        assert_eq!(prims[0]["material"], 0);
        assert_eq!(prims[1]["material"], 0);
        assert_eq!(json["materials"].as_array().unwrap().len(), 1);
        assert_eq!(json["materials"][0]["name"], "skin");
        assert_eq!(
            json["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["index"],
            0
        );
        assert_eq!(json["textures"][0]["source"], 0);
        let image = &json["images"][0];
        assert_eq!(image["mimeType"], "image/png");
        let view = &json["bufferViews"][image["bufferView"].as_u64().unwrap() as usize];
        let start = view["byteOffset"].as_u64().unwrap() as usize;
        assert_eq!(start % 4, 0);
        assert_eq!(&bin[start..start + png.len()], png);
        assert!(view.get("target").is_none());

        // A Material without a texture is a separate one.
        let plain = GlbMaterial {
            name: "skin",
            png: None,
        };
        let (json, _) = parse(&quad().to_glb(&[Some(skin), None, Some(plain)]));
        assert_eq!(json["materials"].as_array().unwrap().len(), 2);
        assert!(
            json["materials"][1]["pbrMetallicRoughness"]
                .get("baseColorTexture")
                .is_none()
        );
        assert_eq!(json["images"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn large_meshes_use_32_bit_indices() {
        let n = MAX_U16_VERTICES + 1;
        let mesh = Mesh {
            name: "big".into(),
            positions: vec![[0.0; 3]; n],
            submeshes: vec![SubMesh {
                triangles: vec![[0, 1, u32::try_from(n - 1).unwrap()]],
            }],
            ..Mesh::default()
        };
        let (json, _) = parse(&mesh.to_glb(&[]));
        let prim = &json["meshes"][0]["primitives"][0];
        assert!(prim["attributes"].get("NORMAL").is_none());
        assert!(prim["attributes"].get("TEXCOORD_0").is_none());
        let indices = &json["accessors"][prim["indices"].as_u64().unwrap() as usize];
        assert_eq!(indices["componentType"], UNSIGNED_INT);
    }

    #[test]
    fn meshes_without_faces_or_vertices_stay_valid() {
        let points = Mesh {
            name: "dots".into(),
            positions: vec![[0.0; 3]; 2],
            ..Mesh::default()
        };
        let (json, _) = parse(&points.to_glb(&[]));
        let prim = &json["meshes"][0]["primitives"][0];
        assert_eq!(prim["mode"], MODE_POINTS);
        assert!(prim.get("indices").is_none());

        let empty = Mesh {
            name: "none".into(),
            ..Mesh::default()
        };
        let (json, bin) = parse(&empty.to_glb(&[]));
        assert_eq!(bin, Vec::<u8>::new());
        assert!(json.get("meshes").is_none() && json.get("buffers").is_none());
        assert_eq!(json["nodes"][0]["name"], "none");
        assert!(json["nodes"][0].get("mesh").is_none());
    }
}
