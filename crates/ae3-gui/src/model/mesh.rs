//! The mesh viewer's pure parts: a Mesh's geometry prepared for the GPU,
//! and the orbit camera that looks at it.

use ae3_core::Mesh;

/// Vertical field of view, in radians.
const FOV_Y: f32 = 0.8;
/// Radians per point dragged.
const ORBIT_SPEED: f32 = 0.01;
const MAX_PITCH: f32 = 1.55;
/// Camera distance limits, in bounding radii.
const MIN_DISTANCE: f32 = 0.05;
const MAX_DISTANCE: f32 = 50.0;
const DEFAULT_PITCH: f32 = 0.2;

/// A Mesh ready to upload: positions and normals in the right-handed space
/// OBJ export uses (X negated), triangles wound counter-clockwise.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshPreview {
    /// `x, y, z, nx, ny, nz` for each vertex.
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub center: [f32; 3],
    /// The bounding sphere's radius around `center`; never 0.
    pub radius: f32,
    pub vertex_count: usize,
    pub triangle_count: usize,
    pub submesh_count: usize,
    /// Whether the normals came with the Mesh (otherwise they are computed).
    pub has_normals: bool,
    pub has_uvs: bool,
}

impl MeshPreview {
    #[must_use]
    pub fn new(mesh: &Mesh) -> Self {
        let positions: Vec<[f32; 3]> = mesh
            .positions
            .iter()
            .map(|&[x, y, z]| [-x, y, z].map(|c| if c.is_finite() { c } else { 0.0 }))
            .collect();
        let indices: Vec<u32> = mesh
            .submeshes
            .iter()
            .flat_map(|s| &s.triangles)
            .flat_map(|&[a, b, c]| [c, b, a])
            .collect();
        let normals = if mesh.normals.len() == positions.len() {
            mesh.normals.iter().map(|&[x, y, z]| [-x, y, z]).collect()
        } else {
            smooth_normals(&positions, &indices)
        };
        let vertices = positions
            .iter()
            .zip(&normals)
            .flat_map(|(p, n)| {
                let n = normalized(*n).unwrap_or([0.0, 1.0, 0.0]);
                [p[0], p[1], p[2], n[0], n[1], n[2]]
            })
            .collect();
        let (center, radius) = bounding_sphere(&positions);
        Self {
            vertices,
            indices,
            center,
            radius,
            vertex_count: positions.len(),
            triangle_count: mesh.triangle_count(),
            submesh_count: mesh.submeshes.len(),
            has_normals: mesh.normals.len() == mesh.positions.len() && !mesh.normals.is_empty(),
            has_uvs: !mesh.uvs.is_empty(),
        }
    }
}

fn normalized([x, y, z]: [f32; 3]) -> Option<[f32; 3]> {
    let len = (x * x + y * y + z * z).sqrt();
    (len > 1e-12 && len.is_finite()).then(|| [x / len, y / len, z / len])
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Area-weighted vertex normals for counter-clockwise triangles.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| i as usize);
        let (Some(&pa), Some(&pb), Some(&pc)) =
            (positions.get(a), positions.get(b), positions.get(c))
        else {
            continue;
        };
        let face = cross(sub(pb, pa), sub(pc, pa));
        for v in [a, b, c] {
            for k in 0..3 {
                normals[v][k] += face[k];
            }
        }
    }
    normals
}

/// The bounding box's center and the farthest vertex's distance from it.
fn bounding_sphere(positions: &[[f32; 3]]) -> ([f32; 3], f32) {
    let Some(first) = positions.first() else {
        return ([0.0; 3], 1.0);
    };
    let (min, max) = positions
        .iter()
        .fold((*first, *first), |(mut lo, mut hi), p| {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            (lo, hi)
        });
    let center = [0, 1, 2].map(|k| f32::midpoint(min[k], max[k]));
    let radius = positions
        .iter()
        .map(|p| {
            let d = sub(*p, center);
            dot(d, d)
        })
        .fold(0.0f32, f32::max)
        .sqrt();
    (center, if radius > 1e-6 { radius } else { 1.0 })
}

/// A column-major 4×4 matrix, as OpenGL takes it.
pub type Mat4 = [f32; 16];

fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y / 2.0).tan();
    let mut m = [0.0; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = (far + near) / (near - far);
    m[11] = -1.0;
    m[14] = 2.0 * far * near / (near - far);
    m
}

fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalized(sub(target, eye)).unwrap_or([0.0, 0.0, -1.0]);
    let s = normalized(cross(f, up)).unwrap_or([1.0, 0.0, 0.0]);
    let u = cross(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.0,
        s[1],
        u[1],
        -f[1],
        0.0,
        s[2],
        u[2],
        -f[2],
        0.0,
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.0,
    ]
}

/// A camera circling the Mesh's center. Distance and panning are in
/// bounding radii, so the same view suits any Mesh size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    /// Around the vertical axis; 0 looks at the Mesh's front (from +Z).
    pub yaw: f32,
    /// Up and down; positive looks from above.
    pub pitch: f32,
    pub distance: f32,
    /// The point looked at, relative to the Mesh's center.
    pub pan: [f32; 3],
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            distance: Self::fit_distance(),
            pan: [0.0; 3],
        }
    }
}

impl OrbitCamera {
    /// The distance at which the bounding sphere just fits vertically.
    fn fit_distance() -> f32 {
        1.05 / (FOV_Y / 2.0).sin()
    }

    /// Orbits by a drag of `dx`, `dy` points.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * ORBIT_SPEED;
        self.pitch = (self.pitch + dy * ORBIT_SPEED).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Moves closer by `factor` (> 1 zooms in).
    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance / factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        }
    }

    /// Slides the view by a drag of `dx`, `dy` points in a view `height`
    /// points tall, so the Mesh follows the pointer.
    pub fn pan_by(&mut self, dx: f32, dy: f32, height: f32) {
        let (right, up, _) = self.basis();
        // Radii per point at the target's depth.
        let scale = 2.0 * self.distance * (FOV_Y / 2.0).tan() / height.max(1.0);
        for k in 0..3 {
            self.pan[k] += (-dx * right[k] + dy * up[k]) * scale;
        }
    }

    /// Right, up and backward (towards the camera) unit vectors.
    fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let back = [sy * cp, sp, cy * cp];
        let right = [cy, 0.0, -sy];
        (right, cross(back, right), back)
    }

    /// The direction towards a light above and left of the camera, which
    /// moves with it so the side in view is always lit.
    #[must_use]
    pub fn light(&self) -> [f32; 3] {
        let (right, up, back) = self.basis();
        normalized([0, 1, 2].map(|k| back[k] + 0.5 * up[k] - 0.35 * right[k])).unwrap_or(back)
    }

    /// The view-projection matrix for `preview` in a view of `aspect`
    /// (width / height).
    #[must_use]
    pub fn view_projection(&self, preview: &MeshPreview, aspect: f32) -> Mat4 {
        let r = preview.radius;
        let target = [0, 1, 2].map(|k| preview.center[k] + self.pan[k] * r);
        let (_, up, back) = self.basis();
        let eye = [0, 1, 2].map(|k| target[k] + back[k] * self.distance * r);
        let pan_len = dot(self.pan, self.pan).sqrt();
        let near = ((self.distance - 1.1 - pan_len) * r).max(r * 0.01);
        let far = (self.distance + 1.1 + pan_len) * r;
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        mul(
            &perspective(FOV_Y, aspect, near, far),
            &look_at(eye, target, up),
        )
    }
}

#[cfg(test)]
mod tests {
    use ae3_core::SubMesh;

    use super::*;

    /// Transforms a point by a matrix into normalized device coordinates.
    fn project(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
        let v = [p[0], p[1], p[2], 1.0];
        let row = |r: usize| (0..4).map(|k| m[k * 4 + r] * v[k]).sum::<f32>();
        let w = row(3);
        [row(0) / w, row(1) / w, row(2) / w]
    }

    fn quad() -> Mesh {
        Mesh {
            name: "quad".into(),
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 2.0, 0.0],
                [0.0, 2.0, 0.0],
            ],
            normals: Vec::new(),
            uvs: Vec::new(),
            submeshes: vec![SubMesh {
                // Clockwise seen from -Z, Unity's front.
                triangles: vec![[0, 2, 1], [0, 3, 2]],
            }],
        }
    }

    #[test]
    fn preview_mirrors_x_and_reverses_winding() {
        let preview = MeshPreview::new(&quad());
        assert_eq!(preview.vertex_count, 4);
        assert_eq!(preview.triangle_count, 2);
        assert_eq!(preview.indices, [1, 2, 0, 2, 3, 0]);
        assert_eq!(&preview.vertices[6..9], &[-2.0, 0.0, 0.0]);
        assert_eq!(preview.center, [-1.0, 1.0, 0.0]);
        assert!((preview.radius - 2f32.sqrt()).abs() < 1e-6);
        assert!(!preview.has_normals);
        // Computed normals face Unity's front (-Z) for its clockwise faces.
        for v in preview.vertices.chunks(6) {
            assert_eq!(&v[3..], &[0.0, 0.0, -1.0]);
        }
    }

    #[test]
    fn given_normals_are_mirrored_and_normalized() {
        let mesh = Mesh {
            normals: vec![[2.0, 0.0, 0.0], [0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            ..quad()
        };
        let preview = MeshPreview::new(&mesh);
        assert!(preview.has_normals);
        assert_eq!(&preview.vertices[3..6], &[-1.0, 0.0, 0.0]);
        // A zero normal falls back to up rather than NaN.
        assert_eq!(&preview.vertices[9..12], &[0.0, 1.0, 0.0]);
    }

    #[test]
    fn empty_meshes_get_a_unit_sphere() {
        let preview = MeshPreview::new(&Mesh::default());
        assert_eq!((preview.center, preview.radius), ([0.0; 3], 1.0));
        assert!(preview.vertices.is_empty() && preview.indices.is_empty());
    }

    #[test]
    fn default_view_frames_the_bounding_sphere() {
        let preview = MeshPreview::new(&quad());
        let camera = OrbitCamera {
            pitch: 0.0,
            ..OrbitCamera::default()
        };
        let m = camera.view_projection(&preview, 1.0);
        let center = project(&m, preview.center);
        assert!(center[0].abs() < 1e-5 && center[1].abs() < 1e-5);
        assert!((-1.0..1.0).contains(&center[2]));
        // Sphere extremes stay on screen and in front of the near plane.
        let r = preview.radius;
        for offset in [[r, 0.0, 0.0], [0.0, r, 0.0], [0.0, 0.0, r], [0.0, -r, 0.0]] {
            let p = project(&m, [0, 1, 2].map(|k| preview.center[k] + offset[k]));
            assert!(
                p.iter().all(|c| (-1.0..=1.0).contains(c)),
                "{offset:?} -> {p:?}"
            );
        }
        // Yaw 0 looks from +Z: mirrored +X (Unity's -X) is on the right.
        let right = project(&m, [preview.center[0] + 1.0, preview.center[1], 0.0]);
        assert!(right[0] > 0.0);
        // The light comes from the camera's side, above it.
        let light = camera.light();
        assert!(
            light[2] > 0.5 && light[1] > 0.0 && light[0] < 0.0,
            "{light:?}"
        );
    }

    #[test]
    fn orbit_zoom_and_pan_stay_in_range() {
        let mut camera = OrbitCamera::default();
        camera.orbit(0.0, 10_000.0);
        assert_eq!(camera.pitch, MAX_PITCH);
        camera.orbit(100.0, 0.0);
        assert!((camera.yaw + 1.0).abs() < 1e-6);
        camera.zoom(1e9);
        assert_eq!(camera.distance, MIN_DISTANCE);
        camera.zoom(0.0);
        assert_eq!(camera.distance, MIN_DISTANCE);
        camera.zoom(1e-9);
        assert_eq!(camera.distance, MAX_DISTANCE);

        // Panning right by half the view moves the target left by half the
        // visible width at that depth.
        let mut camera = OrbitCamera {
            pitch: 0.0,
            ..OrbitCamera::default()
        };
        camera.pan_by(50.0, 0.0, 100.0);
        let half_height = camera.distance * (FOV_Y / 2.0).tan();
        assert!(
            (camera.pan[0] + half_height).abs() < 1e-5,
            "{:?}",
            camera.pan
        );
        let preview = MeshPreview::new(&quad());
        let m = camera.view_projection(&preview, 1.0);
        let center = project(&m, preview.center);
        assert!((center[0] - 1.0).abs() < 1e-4, "{center:?}");
    }
}
