//! The 3D Mesh view: an orbit camera over a Mesh drawn with OpenGL from an
//! egui paint callback, as egui's `custom3d_glow` demo does, so the GUI
//! keeps the glow backend (ADR-0002).
//!
//! The app owns one [`MeshRenderer`] and shares it with the callback, which
//! eframe runs while it paints. The renderer compiles its shader on first
//! use and uploads a Mesh the first time it draws it; the app frees the
//! buffers once no Mesh is shown, and everything on exit.

// Every glow call is an `unsafe` call into the OpenGL driver. This module
// is the only place AE3 talks to OpenGL itself; each block below only
// passes handles it created on the same context, buffers it owns, and
// sizes taken from those buffers.
#![allow(unsafe_code, reason = "OpenGL calls through glow are unsafe FFI")]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use eframe::egui::{self, CornerRadius, PointerButton, RichText, Sense};
use eframe::egui_glow::{self, ShaderVersion};
use eframe::glow::{self, HasContext as _};

use super::error_label;
use crate::model::mesh::{Mat4, MeshPreview, OrbitCamera};

/// The surface colour, lit by [`OrbitCamera::light`].
const MESH_COLOR: [f32; 3] = [0.78, 0.8, 0.84];
/// Bytes per vertex: position and normal, three f32 each.
const VERTEX_STRIDE: i32 = 24;
const A_POS: u32 = 0;
const A_NORMAL: u32 = 1;

const VERTEX_SHADER: &str = r"
#if NEW_SHADER_INTERFACE
    #define I in
    #define O out
#else
    #define I attribute
    #define O varying
#endif
uniform mat4 u_view_proj;
I vec3 a_pos;
I vec3 a_normal;
O vec3 v_normal;
void main() {
    v_normal = a_normal;
    gl_Position = u_view_proj * vec4(a_pos, 1.0);
}
";

const FRAGMENT_SHADER: &str = r"
#ifdef GL_ES
    precision mediump float;
#endif
#if NEW_SHADER_INTERFACE
    #define I in
    out vec4 out_color;
#else
    #define I varying
    #define out_color gl_FragColor
#endif
uniform vec3 u_light;
uniform vec3 u_color;
I vec3 v_normal;
void main() {
    vec3 n = normalize(v_normal);
    if (!gl_FrontFacing) {
        n = -n;
    }
    float diffuse = max(dot(n, u_light), 0.0);
    float sky = 0.5 + 0.5 * n.y;
    out_color = vec4(u_color * (0.2 + 0.15 * sky + 0.65 * diffuse), 1.0);
}
";

/// Draws one Mesh at a time; see the module docs.
#[derive(Default)]
pub struct MeshRenderer {
    program: Option<Program>,
    mesh: Option<Uploaded>,
    /// The shader failed; nothing can be drawn.
    program_error: Option<String>,
    /// Uploading the Mesh of this generation failed.
    mesh_error: Option<(u64, String)>,
}

struct Program {
    handle: glow::Program,
    u_view_proj: Option<glow::UniformLocation>,
    u_light: Option<glow::UniformLocation>,
    u_color: Option<glow::UniformLocation>,
    /// Whether to use a vertex array object, decided as egui decides for
    /// its own (see [`supports_vao`]); otherwise the attributes are set on
    /// every draw, as egui then does too.
    use_vao: bool,
}

struct Uploaded {
    generation: u64,
    vao: Option<glow::VertexArray>,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    index_count: i32,
    index_type: u32,
}

/// What one frame draws with.
struct View {
    view_proj: Mat4,
    light: [f32; 3],
}

/// Locks the shared renderer, whatever a panicking frame left behind.
pub fn lock(renderer: &Mutex<MeshRenderer>) -> MutexGuard<'_, MeshRenderer> {
    renderer.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MeshRenderer {
    /// Why the Mesh of `generation` cannot be drawn, if it cannot.
    fn error(&self, generation: u64) -> Option<&str> {
        self.program_error.as_deref().or(self
            .mesh_error
            .as_ref()
            .filter(|(g, _)| *g == generation)
            .map(|(_, e)| e.as_str()))
    }

    fn paint(&mut self, gl: &glow::Context, preview: &MeshPreview, generation: u64, view: &View) {
        if self.error(generation).is_some() {
            return;
        }
        if self.program.is_none() {
            match unsafe { Program::new(gl) } {
                Ok(program) => self.program = Some(program),
                Err(e) => {
                    self.program_error = Some(format!("The 3D view is unavailable: {e}"));
                    return;
                }
            }
        }
        let Some(program) = &self.program else { return };
        if self.mesh.as_ref().map(|m| m.generation) != Some(generation) {
            if let Some(old) = self.mesh.take() {
                unsafe { old.delete(gl) };
            }
            match unsafe { Uploaded::new(gl, program, preview, generation) } {
                Ok(mesh) => self.mesh = Some(mesh),
                Err(e) => {
                    self.mesh_error = Some((generation, format!("Cannot upload the Mesh: {e}")));
                    return;
                }
            }
        }
        if let Some(mesh) = &self.mesh {
            unsafe { draw(gl, program, mesh, view) };
        }
    }

    /// Frees the uploaded Mesh, if any.
    pub fn release_mesh(&mut self, gl: &glow::Context) {
        if let Some(mesh) = self.mesh.take() {
            unsafe { mesh.delete(gl) };
        }
        self.mesh_error = None;
    }

    /// Frees everything, on exit.
    pub fn destroy(&mut self, gl: &glow::Context) {
        self.release_mesh(gl);
        if let Some(program) = self.program.take() {
            unsafe { gl.delete_program(program.handle) };
        }
    }
}

impl Program {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let version = ShaderVersion::get(gl);
        let header = format!(
            "{}#define NEW_SHADER_INTERFACE {}\n",
            version.version_declaration(),
            u8::from(version.is_new_shader_interface())
        );
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::with_capacity(2);
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX_SHADER),
                (glow::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let log = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    for s in shaders {
                        gl.delete_shader(s);
                    }
                    gl.delete_program(program);
                    return Err(format!("shader compilation failed: {}", log.trim()));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.bind_attrib_location(program, A_POS, "a_pos");
            gl.bind_attrib_location(program, A_NORMAL, "a_normal");
            gl.link_program(program);
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(format!("shader linking failed: {}", log.trim()));
            }
            Ok(Self {
                u_view_proj: gl.get_uniform_location(program, "u_view_proj"),
                u_light: gl.get_uniform_location(program, "u_light"),
                u_color: gl.get_uniform_location(program, "u_color"),
                handle: program,
                use_vao: supports_vao(gl),
            })
        }
    }
}

/// `egui_glow`'s own test for vertex array objects (core from OpenGL 3 and
/// OpenGL ES 3, else an extension). Matching it matters: with egui's VAO
/// bound, setting attributes without one of our own would change egui's.
fn supports_vao(gl: &glow::Context) -> bool {
    let version = unsafe { gl.get_parameter_string(glow::VERSION) };
    let extension = |name: &str| {
        let extensions = gl.supported_extensions();
        extensions.contains(name) || extensions.contains(&format!("GL_{name}"))
    };
    if let Some(pos) = version.rfind("WebGL ") {
        !version[pos..].contains("1.0") || extension("OES_vertex_array_object")
    } else if version.contains("OpenGL ES ") {
        !version.contains("2.0") || extension("OES_vertex_array_object")
    } else {
        !version.starts_with('2') || extension("ARB_vertex_array_object")
    }
}

/// Points the vertex attributes at the bound vertex buffer.
unsafe fn set_attributes(gl: &glow::Context) {
    unsafe {
        gl.vertex_attrib_pointer_f32(A_POS, 3, glow::FLOAT, false, VERTEX_STRIDE, 0);
        gl.enable_vertex_attrib_array(A_POS);
        gl.vertex_attrib_pointer_f32(A_NORMAL, 3, glow::FLOAT, false, VERTEX_STRIDE, 12);
        gl.enable_vertex_attrib_array(A_NORMAL);
    }
}

impl Uploaded {
    unsafe fn new(
        gl: &glow::Context,
        program: &Program,
        preview: &MeshPreview,
        generation: u64,
    ) -> Result<Self, String> {
        let index_count =
            i32::try_from(preview.indices.len()).map_err(|_| "too many triangles".to_owned())?;
        let vertex_bytes: Vec<u8> = preview
            .vertices
            .iter()
            .flat_map(|f| f.to_ne_bytes())
            .collect();
        // 16-bit indices whenever they suffice: half the memory, and
        // OpenGL ES 2 needs an extension for 32-bit ones.
        let (index_bytes, index_type): (Vec<u8>, u32) = if preview.vertex_count <= 1 << 16 {
            let bytes = preview
                .indices
                .iter()
                .flat_map(|&i| u16::try_from(i).unwrap_or(0).to_ne_bytes())
                .collect();
            (bytes, glow::UNSIGNED_SHORT)
        } else {
            let bytes = preview
                .indices
                .iter()
                .flat_map(|i| i.to_ne_bytes())
                .collect();
            (bytes, glow::UNSIGNED_INT)
        };
        unsafe {
            let vao = if program.use_vao {
                let vao = gl.create_vertex_array()?;
                gl.bind_vertex_array(Some(vao));
                Some(vao)
            } else {
                None
            };
            let vertices = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &vertex_bytes, glow::STATIC_DRAW);
            let indices = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(indices));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, &index_bytes, glow::STATIC_DRAW);
            if vao.is_some() {
                set_attributes(gl);
                gl.bind_vertex_array(None);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(Self {
                generation,
                vao,
                vertices,
                indices,
                index_count,
                index_type,
            })
        }
    }

    unsafe fn delete(self, gl: &glow::Context) {
        unsafe {
            if let Some(vao) = self.vao {
                gl.delete_vertex_array(vao);
            }
            gl.delete_buffer(self.vertices);
            gl.delete_buffer(self.indices);
        }
    }
}

/// Draws into the callback's viewport, depth-tested against a depth buffer
/// cleared within egui's clip rectangle. egui restores its own state after
/// the callback.
unsafe fn draw(gl: &glow::Context, program: &Program, mesh: &Uploaded, view: &View) {
    unsafe {
        gl.use_program(Some(program.handle));
        gl.uniform_matrix_4_f32_slice(program.u_view_proj.as_ref(), false, &view.view_proj);
        let [lx, ly, lz] = view.light;
        gl.uniform_3_f32(program.u_light.as_ref(), lx, ly, lz);
        let [r, g, b] = MESH_COLOR;
        gl.uniform_3_f32(program.u_color.as_ref(), r, g, b);
        if let Some(vao) = mesh.vao {
            gl.bind_vertex_array(Some(vao));
        } else {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(mesh.vertices));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(mesh.indices));
            set_attributes(gl);
        }
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LESS);
        gl.depth_mask(true);
        gl.clear(glow::DEPTH_BUFFER_BIT);
        gl.draw_elements(glow::TRIANGLES, mesh.index_count, mesh.index_type, 0);
        gl.disable(glow::DEPTH_TEST);
        if mesh.vao.is_some() {
            gl.bind_vertex_array(None);
        } else {
            gl.disable_vertex_attrib_array(A_POS);
            gl.disable_vertex_attrib_array(A_NORMAL);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
    }
}

/// The Mesh toolbar and the 3D view filling the rest of `ui`. Drag to
/// orbit, right-, middle- or shift-drag to pan, scroll or pinch to zoom,
/// double-click to reset.
pub fn view(
    ui: &mut egui::Ui,
    camera: &mut OrbitCamera,
    preview: &Arc<MeshPreview>,
    generation: u64,
    renderer: &Arc<Mutex<MeshRenderer>>,
) {
    ui.horizontal_wrapped(|ui| {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut text = format!(
            "{} · {} · {}",
            plural(preview.vertex_count, "vertex", "vertices"),
            plural(preview.triangle_count, "triangle", "triangles"),
            plural(preview.submesh_count, "submesh", "submeshes"),
        );
        if preview.has_uvs {
            text.push_str(" · UVs");
        }
        if !preview.has_normals && preview.vertex_count > 0 {
            text.push_str(" · normals computed");
        }
        ui.label(RichText::new(text).family(super::theme::medium()));
        ui.separator();
        if super::widgets::button(ui, Some(super::icons::RESET), "Reset view", true).clicked() {
            *camera = OrbitCamera::default();
        }
        ui.label(
            RichText::new("drag to orbit · right-drag to pan · scroll to zoom")
                .weak()
                .small(),
        );
    });
    if let Some(e) = lock(renderer).error(generation) {
        error_label(ui, e);
        return;
    }
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let delta = response.drag_delta();
    let shift = ui.input(|i| i.modifiers.shift);
    if response.dragged_by(PointerButton::Secondary)
        || response.dragged_by(PointerButton::Middle)
        || (response.dragged_by(PointerButton::Primary) && shift)
    {
        camera.pan_by(delta.x, delta.y, rect.height());
    } else if response.dragged_by(PointerButton::Primary) {
        camera.orbit(delta.x, delta.y);
    }
    if response.hovered() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        camera.zoom(pinch * (scroll / 200.0).exp());
    }
    if response.double_clicked() {
        *camera = OrbitCamera::default();
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, ui.visuals().extreme_bg_color);
    if preview.indices.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "This Mesh has no triangles",
            egui::FontId::proportional(super::theme::size::BODY),
            ui.visuals().weak_text_color(),
        );
        return;
    }
    let view = View {
        view_proj: camera.view_projection(preview, rect.width() / rect.height()),
        light: camera.light(),
    };
    let preview = Arc::clone(preview);
    let renderer = Arc::clone(renderer);
    let callback = egui_glow::CallbackFn::new(move |_info, painter| {
        lock(&renderer).paint(painter.gl(), &preview, generation, &view);
    });
    painter.add(egui::PaintCallback {
        rect,
        callback: Arc::new(callback),
    });
}
