//! 3D battle view (OpenGL via eframe's glow backend). Only created while the 3D toggle is on.
//!
//! Terrain = cached height grid (cropped to the battle area) with the in-game ground texture;
//! map objects = buildings/bunkers/rocks merged into one simplified mesh; water = translucent
//! planes; tanks = cached simplified meshes (body + rotating turret + gun) tilted to the
//! ground; shots = fading lines. Markers/labels reuse the 2D minimap drawing.
//! Left-drag rotates the camera, right-drag moves it, the wheel zooms.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::glow::{self, HasContext};
use egui::{FontId, Pos2, Sense};

use super::minimap2d::{draw_marker, team_color, BattleView, Highlight, ViewInput, ViewOutput};
use crate::gamedata::cache3d::{MapScene, TankModel, Terrain};
use crate::gamedata::dds::Image;
use crate::gamedata::models::Mesh;
use crate::gamedata::ArenaDef;

type Mat4 = [f32; 16]; // column-major

fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}
fn transform(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15],
    ]
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
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-6);
    [a[0] / l, a[1] / l, a[2] / l]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = norm(sub(target, eye));
    let s = norm(cross(f, up));
    let u = cross(s, f);
    [s[0], u[0], -f[0], 0.0, s[1], u[1], -f[1], 0.0, s[2], u[2], -f[2], 0.0, -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0]
}
fn translate(t: [f32; 3]) -> Mat4 {
    [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0]
}
/// Rotation about +y in WoT's left-handed space: local forward (0,0,1) -> (sin a, 0, cos a).
fn rot_y(a: f32) -> Mat4 {
    let (s, c) = a.sin_cos();
    [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}
/// Raise the +z axis towards +y by `a` radians (gun elevation).
fn pitch_up(a: f32) -> Mat4 {
    let (s, c) = a.sin_cos();
    [1.0, 0.0, 0.0, 0.0, 0.0, c, -s, 0.0, 0.0, s, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}
/// World (WoT, left-handed: x east, y up, z north) -> render space (right-handed): flip z.
const FLIP_Z: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

struct Camera {
    /// World-space point the camera orbits.
    target: [f32; 3],
    yaw: f32,
    pitch: f32,
    dist: f32,
}

impl Camera {
    fn eye(&self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [self.target[0] - sy * cp * self.dist, self.target[1] + sp * self.dist, self.target[2] - cy * cp * self.dist]
    }
    fn view_proj(&self, aspect: f32) -> Mat4 {
        let r = |p: [f32; 3]| [p[0], p[1], -p[2]];
        let view = look_at(r(self.eye()), r(self.target), [0.0, 1.0, 0.0]);
        mul(&perspective(50f32.to_radians(), aspect, 1.0, 4000.0), &view)
    }
}

/// What the GL thread needs to draw one frame.
struct FrameDraw {
    view_proj: Mat4,
    brightness: f32,
    /// Yellow 564 m circle painted on the ground: world x, z, radius (radius 0 = off).
    circle: [f32; 3],
    /// (model key, model matrix in render space, rgba)
    meshes: Vec<(usize, Mat4, [f32; 4])>,
    /// Map-event prefab states alive now: (prefab key, model matrix in render space)
    prefabs: Vec<(usize, Mat4)>,
    /// x,y,z, r,g,b,a per vertex (render space), pairs = segments
    lines: Vec<f32>,
    /// No textures: grey ground/objects, faint blue water.
    plain: bool,
}

/// Static scene data waiting to be uploaded.
struct Pending {
    terrain: (Vec<f32>, Vec<u32>),
    texture: TexSrc,
    objects: ObjGeom,
    atlas: Option<Vec<u8>>,
    atlas_grid: u32,
    water: Vec<f32>,
}

/// Texture to upload: plain RGBA, or a DXT1 DDS with mips (uploaded compressed when possible).
enum TexSrc {
    Rgba(Image),
    Dds(Vec<u8>),
}

/// Upload a texture with mipmaps, repeat or clamp wrapping.
unsafe fn upload_texture(gl: &glow::Context, src: &TexSrc, clamp: bool) -> Option<glow::Texture> {
    let tex = gl.create_texture().ok();
    gl.bind_texture(glow::TEXTURE_2D, tex);
    let s3tc = gl.supported_extensions().iter().any(|e| e.contains("texture_compression_s3tc") || e.contains("compressed_texture_s3tc"));
    let decoded;
    let img = match src {
        TexSrc::Dds(dds) if s3tc => match crate::gamedata::dds::dxt1_levels(dds) {
            Some(levels) => {
                for (l, (w, h, data)) in levels.iter().enumerate() {
                    gl.compressed_tex_image_2d(glow::TEXTURE_2D, l as i32, glow::COMPRESSED_RGB_S3TC_DXT1_EXT as i32, *w as i32, *h as i32, 0, data.len() as i32, data);
                }
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, levels.len() as i32 - 1);
                None
            }
            None => None,
        },
        TexSrc::Dds(dds) => {
            decoded = crate::gamedata::dds::decode(dds);
            decoded.as_ref()
        }
        TexSrc::Rgba(img) => Some(img),
    };
    if let Some(img) = img {
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, img.width as i32, img.height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, Some(&img.rgba));
        gl.generate_mipmap(glow::TEXTURE_2D);
    }
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    let wrap = if clamp { glow::CLAMP_TO_EDGE } else { glow::REPEAT } as i32;
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap);
    tex
}

/// CPU-side data waiting to be uploaded, and the uploaded GL objects.
#[derive(Default)]
struct Gl {
    pending: Option<Pending>,
    mesh_data: Vec<(usize, Vec<f32>)>,
    prefab_data: Vec<(usize, ObjGeom)>,
    prefab_vaos: HashMap<usize, (glow::VertexArray, i32)>,
    ready: bool,
    terrain_prog: Option<glow::Program>,
    mesh_prog: Option<glow::Program>,
    obj_prog: Option<glow::Program>,
    line_prog: Option<glow::Program>,
    terrain_vao: Option<glow::VertexArray>,
    terrain_count: i32,
    texture: Option<glow::Texture>,
    atlas: Option<glow::Texture>,
    atlas_grid: u32,
    /// Pixels per atlas tile (smaller in the web packs).
    atlas_tile_px: f32,
    objects: Option<(glow::VertexArray, i32)>,
    water: Option<(glow::VertexArray, i32)>,
    meshes: HashMap<usize, (glow::VertexArray, i32)>,
    line_vao: Option<glow::VertexArray>,
    line_vbo: Option<glow::Buffer>,
}

#[cfg(target_arch = "wasm32")]
const HEADER: &str = "#version 300 es\nprecision highp float;\n";
#[cfg(not(target_arch = "wasm32"))]
const HEADER: &str = "#version 330 core\n";

const TERRAIN_VS: &str = r#"
layout(location=0) in vec3 a_pos; layout(location=1) in vec2 a_uv; layout(location=2) in vec3 a_n;
uniform mat4 u_vp; out vec2 v_uv; out vec3 v_n; out vec2 v_world;
void main(){ v_uv=a_uv; v_n=a_n; v_world=vec2(a_pos.x,-a_pos.z); gl_Position=u_vp*vec4(a_pos,1.0); }"#;
const TERRAIN_FS: &str = r#"
in vec2 v_uv; in vec3 v_n; in vec2 v_world; uniform sampler2D u_tex; uniform float u_bright; uniform vec3 u_circle; uniform float u_plain; out vec4 o;
void main(){
  vec3 c = u_plain > 0.5 ? vec3(0.55) : texture(u_tex, v_uv).rgb;
  float l = 0.45 + 0.65*max(dot(normalize(v_n), normalize(vec3(-0.4,0.85,-0.35))),0.0);
  vec3 col = c*l*u_bright;
  if (u_circle.z > 0.0) {
    float d = abs(length(v_world - u_circle.xy) - u_circle.z);
    float w = fwidth(length(v_world - u_circle.xy)) * 1.5 + 0.6;
    col = mix(col, vec3(0.95,0.85,0.25), clamp(1.0 - d / w, 0.0, 1.0) * 0.9);
  }
  o = vec4(col,1.0);
}"#;
const MESH_VS: &str = r#"
layout(location=0) in vec3 a_pos; layout(location=1) in vec3 a_n;
uniform mat4 u_vp; uniform mat4 u_m; out vec3 v_n;
void main(){ v_n=mat3(u_m)*a_n; gl_Position=u_vp*u_m*vec4(a_pos,1.0); }"#;
const MESH_FS: &str = r#"
in vec3 v_n; uniform vec4 u_color; out vec4 o;
void main(){
  float l = 0.4 + 0.7*abs(dot(normalize(v_n), normalize(vec3(-0.4,0.85,-0.35))));
  o = vec4(u_color.rgb*l, u_color.a);
}"#;
const OBJ_VS: &str = r#"
layout(location=0) in vec3 a_pos; layout(location=2) in vec3 a_col;
layout(location=3) in vec2 a_uv; layout(location=4) in float a_tile;
uniform mat4 u_vp; uniform mat4 u_m; out vec3 v_pos; out vec3 v_col; out vec2 v_uv; flat out float v_tile;
void main(){ vec4 w=u_m*vec4(a_pos,1.0); v_pos=w.xyz; v_col=a_col; v_uv=a_uv; v_tile=a_tile; gl_Position=u_vp*w; }"#;
// Textured triangles sample their tile of the object atlas (UVs wrap inside the tile);
// untextured ones (tile < 0) use the baked vertex colour.
const OBJ_FS: &str = r#"
in vec3 v_pos; in vec3 v_col; in vec2 v_uv; flat in float v_tile; uniform float u_bright; uniform sampler2D u_atlas; uniform float u_grid; uniform float u_tile_px; uniform float u_plain; out vec4 o;
void main(){
  vec3 c = v_col;
  if (u_plain > 0.5) {
    c = vec3(0.66);
  } else if (v_tile >= 0.0) {
    float ts = 1.0 / u_grid;
    vec2 origin = vec2(mod(v_tile, u_grid), floor(v_tile / u_grid)) * ts;
    float inset = 1.5 / (u_grid * u_tile_px);
    vec2 uv = origin + vec2(inset) + fract(v_uv) * (ts - 2.0 * inset);
    c = textureGrad(u_atlas, uv, dFdx(v_uv) * ts, dFdy(v_uv) * ts).rgb * v_col * (255.0 / 128.0);
  }
  float l = 0.45 + 0.65*abs(dot(normalize(cross(dFdx(v_pos), dFdy(v_pos))), normalize(vec3(-0.4,0.85,-0.35))));
  o = vec4(c*l*u_bright, 1.0);
}"#;
const LINE_VS: &str = r#"
layout(location=0) in vec3 a_pos; layout(location=1) in vec4 a_col;
uniform mat4 u_vp; out vec4 v_col;
void main(){ v_col=a_col; gl_Position=u_vp*vec4(a_pos,1.0); }"#;
const LINE_FS: &str = r#"
in vec4 v_col; out vec4 o; void main(){ o=v_col; }"#;

unsafe fn program(gl: &glow::Context, vs: &str, fs: &str) -> Option<glow::Program> {
    let p = gl.create_program().ok()?;
    let mut shaders = Vec::new();
    for (ty, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
        let s = gl.create_shader(ty).ok()?;
        gl.shader_source(s, &format!("{HEADER}{src}"));
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            eprintln!("shader: {}", gl.get_shader_info_log(s));
            return None;
        }
        gl.attach_shader(p, s);
        shaders.push(s);
    }
    gl.link_program(p);
    for s in shaders {
        gl.detach_shader(p, s);
        gl.delete_shader(s);
    }
    gl.get_program_link_status(p).then_some(p)
}

fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// VAO for interleaved float data with the given (components) per attribute.
unsafe fn vao_f32(gl: &glow::Context, data: &[f32], layout: &[i32]) -> Option<glow::VertexArray> {
    let vao = gl.create_vertex_array().ok();
    gl.bind_vertex_array(vao);
    let vbo = gl.create_buffer().ok();
    gl.bind_buffer(glow::ARRAY_BUFFER, vbo);
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(data), glow::STATIC_DRAW);
    let stride: i32 = layout.iter().sum::<i32>() * 4;
    let mut off = 0;
    for (i, n) in layout.iter().enumerate() {
        gl.enable_vertex_attrib_array(i as u32);
        gl.vertex_attrib_pointer_f32(i as u32, *n, glow::FLOAT, false, stride, off);
        off += n * 4;
    }
    vao
}

impl Gl {
    unsafe fn upload(&mut self, gl: &glow::Context) {
        if !self.ready {
            self.terrain_prog = program(gl, TERRAIN_VS, TERRAIN_FS);
            self.mesh_prog = program(gl, MESH_VS, MESH_FS);
            self.obj_prog = program(gl, OBJ_VS, OBJ_FS);
            self.line_prog = program(gl, LINE_VS, LINE_FS);
            let vao = gl.create_vertex_array().ok();
            let vbo = gl.create_buffer().ok();
            gl.bind_vertex_array(vao);
            gl.bind_buffer(glow::ARRAY_BUFFER, vbo);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 28, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, 28, 12);
            self.line_vao = vao;
            self.line_vbo = vbo;
            self.ready = true;
        }
        if let Some(p) = self.pending.take() {
            let (verts, idx) = p.terrain;
            self.terrain_vao = vao_f32(gl, &verts, &[3, 2, 3]);
            let ibo = gl.create_buffer().ok();
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, ibo);
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&idx), glow::STATIC_DRAW);
            self.terrain_count = idx.len() as i32;
            self.texture = upload_texture(gl, &p.texture, true);
            if let Some(a) = &p.atlas {
                self.atlas = upload_texture(gl, &TexSrc::Dds(a.clone()), true);
                self.atlas_grid = p.atlas_grid.max(1);
                let width = crate::gamedata::dds::dxt1_levels(a).and_then(|l| l.first().map(|l| l.0)).unwrap_or(self.atlas_grid as usize * crate::gamedata::cache3d::ATLAS_TILE);
                self.atlas_tile_px = width as f32 / self.atlas_grid as f32;
            }
            if !p.objects.indices.is_empty() {
                self.objects = obj_vao(gl, &p.objects);
            }
            if !p.water.is_empty() {
                self.water = vao_f32(gl, &p.water, &[3, 3]).map(|v| (v, (p.water.len() / 6) as i32));
            }
        }
        for (key, verts) in std::mem::take(&mut self.prefab_data) {
            if let Some(v) = obj_vao(gl, &verts) {
                self.prefab_vaos.insert(key, v);
            }
        }
        for (key, verts) in std::mem::take(&mut self.mesh_data) {
            if let Some(vao) = vao_f32(gl, &verts, &[3, 3]) {
                self.meshes.insert(key, (vao, (verts.len() / 6) as i32));
            }
        }
        gl.bind_vertex_array(None);
    }

    unsafe fn paint(&mut self, gl: &glow::Context, info: &egui::PaintCallbackInfo, f: &FrameDraw) {
        self.upload(gl);
        let vp = info.viewport_in_pixels();
        let clip = info.clip_rect_in_pixels();
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(clip.left_px, clip.from_bottom_px, clip.width_px, clip.height_px);
        gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
        gl.clear_color(0.07, 0.08, 0.09, 1.0);
        gl.clear_depth_f32(1.0);
        gl.depth_mask(true);
        gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LEQUAL);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::BLEND);

        if let (Some(p), Some(vao)) = (self.terrain_prog, self.terrain_vao) {
            gl.use_program(Some(p));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(p, "u_vp").as_ref(), false, &f.view_proj);
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_bright").as_ref(), f.brightness);
            gl.uniform_3_f32(gl.get_uniform_location(p, "u_circle").as_ref(), f.circle[0], f.circle[1], f.circle[2]);
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_plain").as_ref(), f.plain as i32 as f32);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, self.texture);
            gl.uniform_1_i32(gl.get_uniform_location(p, "u_tex").as_ref(), 0);
            gl.bind_vertex_array(Some(vao));
            gl.draw_elements(glow::TRIANGLES, self.terrain_count, glow::UNSIGNED_INT, 0);
        }
        // Map objects: vertex-coloured (baked from their textures), already in render space.
        if let Some(p) = self.obj_prog {
            gl.use_program(Some(p));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(p, "u_vp").as_ref(), false, &f.view_proj);
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_bright").as_ref(), f.brightness);
            let um = gl.get_uniform_location(p, "u_m");
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, self.atlas);
            gl.uniform_1_i32(gl.get_uniform_location(p, "u_atlas").as_ref(), 1);
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_grid").as_ref(), self.atlas_grid.max(1) as f32);
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_tile_px").as_ref(), if self.atlas_tile_px > 0.0 { self.atlas_tile_px } else { crate::gamedata::cache3d::ATLAS_TILE as f32 });
            gl.uniform_1_f32(gl.get_uniform_location(p, "u_plain").as_ref(), f.plain as i32 as f32);
            gl.active_texture(glow::TEXTURE0);
            if let Some((vao, n)) = self.objects {
                gl.uniform_matrix_4_f32_slice(um.as_ref(), false, &translate([0.0; 3]));
                gl.bind_vertex_array(Some(vao));
                gl.draw_elements(glow::TRIANGLES, n, glow::UNSIGNED_INT, 0);
            }
            // Map-event states (intact before the event, destroyed after).
            for (key, m) in &f.prefabs {
                if let Some((vao, n)) = self.prefab_vaos.get(key) {
                    gl.uniform_matrix_4_f32_slice(um.as_ref(), false, m);
                    gl.bind_vertex_array(Some(*vao));
                    gl.draw_elements(glow::TRIANGLES, *n, glow::UNSIGNED_INT, 0);
                }
            }
        }
        if let Some(p) = self.mesh_prog {
            gl.use_program(Some(p));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(p, "u_vp").as_ref(), false, &f.view_proj);
            let um = gl.get_uniform_location(p, "u_m");
            let uc = gl.get_uniform_location(p, "u_color");
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            for (key, m, c) in &f.meshes {
                if let Some((vao, n)) = self.meshes.get(key) {
                    gl.uniform_matrix_4_f32_slice(um.as_ref(), false, m);
                    gl.uniform_4_f32(uc.as_ref(), c[0], c[1], c[2], c[3]);
                    gl.bind_vertex_array(Some(*vao));
                    gl.draw_arrays(glow::TRIANGLES, 0, *n);
                }
            }
            // Water last: translucent, doesn't write depth.
            if let Some((vao, n)) = self.water {
                gl.depth_mask(false);
                gl.uniform_matrix_4_f32_slice(um.as_ref(), false, &translate([0.0; 3]));
                if f.plain {
                    gl.uniform_4_f32(uc.as_ref(), 0.42, 0.52, 0.64, 0.45);
                } else {
                    gl.uniform_4_f32(uc.as_ref(), 0.12, 0.32, 0.55, 0.55);
                }
                gl.bind_vertex_array(Some(vao));
                gl.draw_arrays(glow::TRIANGLES, 0, n);
                gl.depth_mask(true);
            }
        }
        if let (Some(p), false) = (self.line_prog, f.lines.is_empty()) {
            gl.use_program(Some(p));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(p, "u_vp").as_ref(), false, &f.view_proj);
            gl.bind_vertex_array(self.line_vao);
            gl.bind_buffer(glow::ARRAY_BUFFER, self.line_vbo);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&f.lines), glow::STREAM_DRAW);
            gl.draw_arrays(glow::LINES, 0, (f.lines.len() / 7) as i32);
        }
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.disable(glow::DEPTH_TEST);
    }
}

/// Flat-shaded triangle soup (pos + face normal); `xf` maps each position (e.g. world -> render).
fn flat(mesh: &Mesh, xf: impl Fn([f32; 3]) -> [f32; 3]) -> Vec<f32> {
    let mut out = Vec::with_capacity(mesh.indices.len() * 6);
    for t in mesh.indices.chunks_exact(3) {
        let (Some(&a), Some(&b), Some(&c)) = (mesh.positions.get(t[0] as usize), mesh.positions.get(t[1] as usize), mesh.positions.get(t[2] as usize)) else {
            continue;
        };
        let (a, b, c) = (xf(a), xf(b), xf(c));
        let n = norm(cross(sub(b, a), sub(c, a)));
        for p in [a, b, c] {
            out.extend_from_slice(&p);
            out.extend_from_slice(&n);
        }
    }
    out
}

/// Like [`flat`] plus per-vertex colour, world -> render space (z flipped).
/// Map-object geometry for the GPU: shared vertices + indices (28 bytes per vertex:
/// position f32x3, uv f32x2, atlas tile f32 (-1 = untextured), colour u8x4). Faces are
/// flat-shaded in the shader (normal from screen-space derivatives), so no normals are stored.
#[derive(Default)]
struct ObjGeom {
    verts: Vec<u8>,
    indices: Vec<u32>,
}

const OBJ_VERT_BYTES: usize = 28;

fn obj_geom(mesh: &Mesh, r: impl Fn([f32; 3]) -> [f32; 3]) -> ObjGeom {
    let textured = mesh.corner_uv.len() == mesh.indices.len() && mesh.tri_tex.len() * 3 == mesh.indices.len();
    let mut g = ObjGeom::default();
    let mut seen: HashMap<([u32; 3], [u32; 2], u32, [u8; 3]), u32> = HashMap::new();
    for (ti, t) in mesh.indices.chunks_exact(3).enumerate() {
        if t.iter().any(|&i| mesh.positions.get(i as usize).is_none()) {
            continue;
        }
        let tile = if textured && mesh.tri_tex[ti] != u16::MAX { mesh.tri_tex[ti] as f32 } else { -1.0 };
        for (k, &i) in t.iter().enumerate() {
            let p = r(mesh.positions[i as usize]);
            let uv = if textured { mesh.corner_uv[ti * 3 + k] } else { [0.0, 0.0] };
            let col = mesh.colors.get(i as usize).copied().unwrap_or([153, 148, 140]);
            let key = (p.map(f32::to_bits), uv.map(f32::to_bits), tile.to_bits(), col);
            let next = (g.verts.len() / OBJ_VERT_BYTES) as u32;
            let idx = *seen.entry(key).or_insert_with(|| {
                for v in p.iter().chain(uv.iter()).chain(std::iter::once(&tile)) {
                    g.verts.extend_from_slice(&v.to_le_bytes());
                }
                g.verts.extend_from_slice(&[col[0], col[1], col[2], 255]);
                next
            });
            g.indices.push(idx);
        }
    }
    g
}

fn flat_colored(mesh: &Mesh) -> ObjGeom {
    obj_geom(mesh, |p| [p[0], p[1], -p[2]])
}

/// VAO + index buffer for object geometry; returns (vao, index count).
unsafe fn obj_vao(gl: &glow::Context, g: &ObjGeom) -> Option<(glow::VertexArray, i32)> {
    if g.indices.is_empty() {
        return None;
    }
    let vao = gl.create_vertex_array().ok()?;
    gl.bind_vertex_array(Some(vao));
    let vbo = gl.create_buffer().ok();
    gl.bind_buffer(glow::ARRAY_BUFFER, vbo);
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &g.verts, glow::STATIC_DRAW);
    let st = OBJ_VERT_BYTES as i32;
    gl.enable_vertex_attrib_array(0);
    gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, st, 0);
    gl.enable_vertex_attrib_array(3);
    gl.vertex_attrib_pointer_f32(3, 2, glow::FLOAT, false, st, 12);
    gl.enable_vertex_attrib_array(4);
    gl.vertex_attrib_pointer_f32(4, 1, glow::FLOAT, false, st, 20);
    gl.enable_vertex_attrib_array(2);
    gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, true, st, 24);
    let ibo = gl.create_buffer().ok();
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, ibo);
    gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&g.indices), glow::STATIC_DRAW);
    gl.bind_vertex_array(None);
    Some((vao, g.indices.len() as i32))
}

/// Everything needed to build a 3D view (produced by an [`super::player::Assets`] source).
pub struct Scene3D {
    pub arena: ArenaDef,
    pub terrain: Terrain,
    pub minimap: Option<Image>,
    pub map: MapScene,
    pub tanks: HashMap<String, TankModel>,
    /// Map-event prefab meshes by prefab name (prefab space).
    pub prefabs: HashMap<String, Mesh>,
}

pub struct View3D {
    terrain: Terrain,
    /// vehicle type -> (body, turret, gun) mesh keys + pivots
    tanks: HashMap<String, (usize, usize, usize, [f32; 3], [f32; 3])>,
    prefabs: HashMap<String, usize>,
    /// vehicle type -> gun length from its pivot to the muzzle (m)
    gun_len: HashMap<String, f32>,
    gl: Arc<Mutex<Gl>>,
    cam: Option<Camera>,
    /// Tank the camera follows (right-click a tank / "Follow" in its panel; panning stops it).
    follow: Option<i32>,
}

impl View3D {
    pub fn new(scene: Scene3D) -> Self {
        let Scene3D { arena, terrain, minimap, map, tanks, prefabs } = scene;
        let [bx0, bz0] = arena.bbox_min;
        let [bx1, bz1] = arena.bbox_max;

        // Ground texture: in-game albedo over its world extent, or the minimap over the battle area.
        let (texture, [tx0, tz0, tx1, tz1]) = match map.albedo {
            Some(dds) => (TexSrc::Dds(dds), map.albedo_extent),
            None => (TexSrc::Rgba(minimap.unwrap_or(Image { width: 1, height: 1, rgba: vec![70, 80, 60, 255] })), [bx0, bz0, bx1, bz1]),
        };

        // Terrain mesh, cropped to the battle area (no outland), in render space (z flipped).
        let (w, h) = (terrain.w, terrain.h);
        let hgt = |x: usize, z: usize| terrain.heights[z.min(h - 1) * w + x.min(w - 1)];
        let col = |x: f32| ((x - terrain.x0) / terrain.spacing).round() as i64;
        let (ix0, ix1) = (col(bx0).max(0) as usize, (col(bx1).max(0) as usize).min(w - 1));
        let (iz0, iz1) = (((bz0 - terrain.z0) / terrain.spacing).round().max(0.0) as usize, ((((bz1 - terrain.z0) / terrain.spacing).round()).max(0.0) as usize).min(h - 1));
        let (cw, ch) = (ix1 - ix0 + 1, iz1 - iz0 + 1);
        let mut verts = Vec::with_capacity(cw * ch * 8);
        for z in iz0..=iz1 {
            for x in ix0..=ix1 {
                let wx = terrain.x0 + x as f32 * terrain.spacing;
                let wz = terrain.z0 + z as f32 * terrain.spacing;
                let y = hgt(x, z);
                let dx = hgt(x + 1, z) - hgt(x.saturating_sub(1), z);
                let dz = hgt(x, z + 1) - hgt(x, z.saturating_sub(1));
                let n = norm([-dx, 2.0 * terrain.spacing, dz]);
                verts.extend_from_slice(&[wx, y, -wz, (wx - tx0) / (tx1 - tx0), (tz1 - wz) / (tz1 - tz0), n[0], n[1], n[2]]);
            }
        }
        let mut idx = Vec::with_capacity((cw - 1) * (ch - 1) * 6);
        for z in 0..ch - 1 {
            for x in 0..cw - 1 {
                let i = (z * cw + x) as u32;
                idx.extend_from_slice(&[i, i + 1, i + cw as u32, i + 1, i + cw as u32 + 1, i + cw as u32]);
            }
        }

        let objects = flat_colored(&map.objects);
        let (atlas, atlas_grid) = (map.atlas, map.atlas_grid);
        let mut water = Vec::new();
        for wq in &map.water {
            let [x0, z0, x1, z1, y] = *wq;
            for (x, z) in [(x0, z0), (x1, z0), (x1, z1), (x0, z0), (x1, z1), (x0, z1)] {
                water.extend_from_slice(&[x, y, -z, 0.0, 1.0, 0.0]);
            }
        }

        let mut gl = Gl { pending: Some(Pending { terrain: (verts, idx), texture, objects, atlas, atlas_grid, water }), ..Default::default() };
        let mut keys = HashMap::new();
        let mut gun_len = HashMap::new();
        let mut next = 0usize;
        for (vt, m) in tanks {
            gun_len.insert(vt.clone(), m.gun.bounds().1[2].max(0.0));
            let mut ids = [0usize; 3];
            for (i, mesh) in [&m.body, &m.turret, &m.gun].into_iter().enumerate() {
                gl.mesh_data.push((next, flat(mesh, |p| p)));
                ids[i] = next;
                next += 1;
            }
            keys.insert(vt, (ids[0], ids[1], ids[2], m.turret_pivot, m.gun_pivot));
        }
        // Prefab meshes stay in their own (left-handed) space; the per-frame matrix handles placement.
        let mut prefab_keys = HashMap::new();
        for (i, (name, mesh)) in prefabs.into_iter().enumerate() {
            gl.prefab_data.push((i, obj_geom(&mesh, |p| p)));
            prefab_keys.insert(name, i);
        }
        Self { terrain, tanks: keys, prefabs: prefab_keys, gun_len, gl: Arc::new(Mutex::new(gl)), cam: None, follow: None }
    }

    /// Start the camera looking at a world position.
    pub fn set_target(&mut self, x: f32, z: f32) {
        self.cam = Some(Camera { target: [x, self.terrain.height_at(x, z), z], yaw: 0.0, pitch: 0.85, dist: 260.0 });
    }

    /// Orbit angles (radians) and distance (m) around the current target.
    pub fn set_orbit(&mut self, yaw: f32, pitch: f32, dist: f32) {
        if let Some(c) = &mut self.cam {
            (c.yaw, c.pitch, c.dist) = (yaw, pitch, dist);
        }
    }

    /// Hull model matrix (world space) from the recorded yaw / pitch / roll; falls back to the
    /// terrain slope when the replay has no pitch/roll for this moment.
    fn hull_matrix(&self, pos: [f32; 3], yaw: f32, pitch: f32, roll: f32) -> Mat4 {
        if pitch != 0.0 || roll != 0.0 {
            let [right, up, fwd] = crate::replay::battle::hull_axes(yaw, pitch, roll);
            return [right[0], right[1], right[2], 0.0, up[0], up[1], up[2], 0.0, fwd[0], fwd[1], fwd[2], 0.0, pos[0], pos[1], pos[2], 1.0];
        }
        let (s, c) = yaw.sin_cos();
        let (l, w) = (2.6, 1.6);
        let h = |x: f32, z: f32| self.terrain.height_at(x, z);
        let fwd = norm([s, (h(pos[0] + s * l, pos[2] + c * l) - h(pos[0] - s * l, pos[2] - c * l)) / (2.0 * l), c]);
        let right0 = norm([c, (h(pos[0] + c * w, pos[2] - s * w) - h(pos[0] - c * w, pos[2] + s * w)) / (2.0 * w), -s]);
        // Left-handed: up = forward x right; then re-orthogonalise right.
        let up = norm(cross(fwd, right0));
        let right = cross(up, fwd);
        [right[0], right[1], right[2], 0.0, up[0], up[1], up[2], 0.0, fwd[0], fwd[1], fwd[2], 0.0, pos[0], pos[1], pos[2], 1.0]
    }
}

fn rgba(c: egui::Color32, a: f32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, a]
}

impl BattleView for View3D {
    fn follow(&self) -> Option<i32> {
        self.follow
    }

    fn set_follow(&mut self, id: Option<i32>) {
        self.follow = id;
    }

    fn look_at(&mut self, x: f32, z: f32) {
        self.follow = None;
        match &mut self.cam {
            Some(c) => {
                c.target[0] = x;
                c.target[2] = z;
            }
            None => self.set_target(x, z),
        }
    }

    fn camera(&self) -> Option<[f32; 5]> {
        let c = self.cam.as_ref()?;
        Some([c.target[0], c.target[2], c.yaw, c.pitch, c.dist])
    }

    fn camera_mark(&self) -> Option<[f32; 3]> {
        let c = self.cam.as_ref()?;
        let e = c.eye();
        Some([e[0], e[2], c.yaw])
    }

    fn show(&mut self, ui: &mut egui::Ui, input: &ViewInput) -> ViewOutput {
        let st = input.settings;
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());

        // Camera: start behind the recording player's tank.
        let cam = self.cam.get_or_insert_with(|| {
            let me = input.units.iter().find(|u| u.is_recorder).or(input.units.first());
            let (target, yaw) = me.map(|u| (u.pos, u.yaw)).unwrap_or(([0.0; 3], 0.0));
            Camera { target, yaw, pitch: 0.75, dist: 220.0 }
        });
        if response.dragged_by(egui::PointerButton::Primary) {
            let d = response.drag_delta();
            cam.yaw += d.x * 0.008;
            cam.pitch = (cam.pitch + d.y * 0.006).clamp(0.06, 1.52);
        }
        if response.dragged_by(egui::PointerButton::Secondary) {
            let d = response.drag_delta();
            // Panning stops following a tank.
            if d.length_sq() > 0.0 {
                self.follow = None;
            }
            let k = cam.dist * 0.0022;
            let (s, c) = cam.yaw.sin_cos();
            cam.target[0] += (-d.x * c + d.y * s) * k;
            cam.target[2] += (d.x * s + d.y * c) * k;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                cam.dist = (cam.dist * (-scroll * 0.0018).exp()).clamp(12.0, 1800.0);
            }
        }
        if let Some(u) = self.follow.and_then(|id| input.units.iter().find(|u| u.id == id)) {
            cam.target[0] = u.pos[0];
            cam.target[2] = u.pos[2];
        }
        cam.target[1] = self.terrain.height_at(cam.target[0], cam.target[2]);
        let aspect = rect.width() / rect.height().max(1.0);
        let vp = cam.view_proj(aspect);

        let project = |p: [f32; 3]| -> Option<Pos2> {
            let c = transform(&vp, [p[0], p[1], -p[2]]);
            if c[3] <= 0.1 {
                return None;
            }
            let (x, y) = (c[0] / c[3], c[1] / c[3]);
            Some(Pos2::new(rect.left() + (x + 1.0) * 0.5 * rect.width(), rect.top() + (1.0 - y) * 0.5 * rect.height()))
        };

        // Tanks: hull follows the ground, turret traverses, gun on the turret.
        let mut meshes = Vec::new();
        for u in input.units {
            let Some(&(body, turret, gun, tp, gp)) = self.tanks.get(&u.vehicle_type) else { continue };
            let last_known = matches!(u.visibility, super::scene::Visibility::LastKnown { .. });
            let mut color = if !u.alive { egui::Color32::from_gray(70) } else { team_color(u.is_ally) };
            if u.flash > 0.0 {
                let k = u.flash;
                let l = |a: u8| (a as f32 + (255.0 - a as f32) * k) as u8;
                color = egui::Color32::from_rgb(l(color.r()), l(color.g()), l(color.b()));
            }
            let c = rgba(color, if last_known { 0.35 } else { 1.0 });
            let m_body = mul(&FLIP_Z, &self.hull_matrix(u.pos, u.yaw, u.pitch, u.roll));
            let m_turret = mul(&m_body, &mul(&translate(tp), &rot_y(u.turret_yaw)));
            let m_gun = mul(&m_turret, &mul(&translate(gp), &pitch_up(u.gun_pitch)));
            meshes.push((body, m_body, c));
            meshes.push((turret, m_turret, c));
            meshes.push((gun, m_gun, c));
        }

        let mut lines = Vec::new();
        if st.show_shots {
            for s in input.shots {
                let c = if s.is_ally { [1.0, 0.85, 0.45] } else { [1.0, 0.55, 0.35] };
                // The recorded start point is the gun's pivot: the shell leaves the barrel tip.
                let barrel = input
                    .units
                    .iter()
                    .find(|u| u.id == s.shooter)
                    .and_then(|u| self.gun_len.get(&u.vehicle_type))
                    .copied()
                    .unwrap_or(0.0);
                let path = s.path(barrel);
                for seg in path.windows(2) {
                    for p in seg {
                        lines.extend_from_slice(&[p[0], p[1], -p[2], c[0], c[1], c[2], s.alpha]);
                    }
                }
            }
        }
        // Other uploaders' view-range circles (merged battles); the main one is painted by the ground shader.
        if st.range_circle {
            for me in input.units.iter().filter(|u| u.is_viewpoint && u.alive) {
                let n = 160;
                let pt = |i: usize| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    let (x, z) = (me.pos[0] + a.cos() * 564.0, me.pos[2] + a.sin() * 564.0);
                    [x, self.terrain.height_at(x, z) + 0.8, -z]
                };
                for i in 0..n {
                    for p in [pt(i), pt(i + 1)] {
                        lines.extend_from_slice(&[p[0], p[1], p[2], 0.9, 0.84, 0.27, 0.8]);
                    }
                }
            }
        }
        // Capture circles on the ground (white; capturing team's colour while being capped).
        if st.show_bases {
            for c in input.captures {
                let col = match c.capturing_team {
                    Some(team) => {
                        let tc = if c.blocked { egui::Color32::from_gray(160) } else { team_color(team == input.recorder_team) };
                        [tc.r() as f32 / 255.0, tc.g() as f32 / 255.0, tc.b() as f32 / 255.0]
                    }
                    None => [1.0, 1.0, 1.0],
                };
                let n = 72;
                let pt = |i: usize| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    let (x, z) = (c.pos[0] + a.cos() * c.radius, c.pos[1] + a.sin() * c.radius);
                    [x, self.terrain.height_at(x, z) + 0.6, -z]
                };
                for i in 0..n {
                    for p in [pt(i), pt(i + 1)] {
                        lines.extend_from_slice(&[p[0], p[1], p[2], col[0], col[1], col[2], 0.9]);
                    }
                }
            }
        }
        let circle = match input.units.iter().find(|u| u.is_recorder && u.alive) {
            Some(me) if st.range_circle => [me.pos[0], me.pos[2], 564.0],
            _ => [0.0; 3],
        };

        let prefabs: Vec<(usize, Mat4)> = input
            .scenario
            .iter()
            .filter_map(|s| Some((*self.prefabs.get(&s.prefab)?, mul(&FLIP_Z, &mul(&translate(s.pos), &rot_y(s.yaw))))))
            .collect();
        let frame = FrameDraw { view_proj: vp, brightness: st.map_brightness.clamp(0.2, 1.5), circle, meshes, prefabs, lines, plain: !st.textures };
        let gl = self.gl.clone();
        let cb = eframe::egui_glow::CallbackFn::new(move |info, painter| unsafe {
            gl.lock().unwrap().paint(painter.gl(), &info, &frame);
        });
        ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });

        // Markers above the tanks: same drawing (and settings) as the 2D minimap.
        let painter = ui.painter_at(rect);
        let mut out = ViewOutput { hovered: None, clicked_unit: None, clicked_empty: false, clicked_world: None, context_at: None, follow_unit: None, anchors: HashMap::new(), rect };
        let pointer = response.hover_pos();
        let mut best: Option<(f32, i32)> = None;
        let icon_r = 7.0 * st.marker_scale;
        let font = FontId::proportional(13.0 * st.marker_scale.sqrt());
        let mut placed = Vec::new();
        for u in input.units {
            let Some(p) = project([u.pos[0], u.pos[1] + 5.0, u.pos[2]]) else { continue };
            if !rect.contains(p) {
                continue;
            }
            out.anchors.insert(u.id, p);
            if let Some(ptr) = pointer {
                let d = ptr.distance(p);
                if d < icon_r + 10.0 && best.map(|b| d < b.0).unwrap_or(true) {
                    best = Some((d, u.id));
                }
            }
            placed.push((u, p));
        }
        let hovered_here = best.map(|b| b.1);
        for (u, p) in placed {
            let highlight = if input.selected == Some(u.id) {
                Highlight::Selected
            } else if input.hovered == Some(u.id) || hovered_here == Some(u.id) {
                Highlight::Hovered
            } else {
                Highlight::None
            };
            draw_marker(&painter, p, u, st, icon_r, &font, highlight, true);
        }
        out.hovered = hovered_here;
        if response.clicked() {
            match out.hovered {
                Some(id) => out.clicked_unit = Some(id),
                None => out.clicked_empty = true,
            }
        }
        // Right-click (without dragging) on a tank: follow it; elsewhere: the map menu.
        if response.secondary_clicked() {
            match out.hovered {
                Some(id) => self.follow = Some(id),
                None => out.context_at = response.interact_pointer_pos(),
            }
        }
        out
    }
}
