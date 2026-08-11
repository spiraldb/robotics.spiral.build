//! The WebGL half of the bench.
//!
//! This module knows about triangles and nothing about robots: it is handed a
//! camera and a list of already-placed shapes and it draws them. Deciding which
//! shapes those are, and where, belongs to [`crate::robot`].
//!
//! The primitives a URDF is built from are drawn from three unit meshes — a
//! box, a cylinder, and a sphere — scaled per instance. A whole arm is around
//! twenty draw calls, so nothing here needs instancing or batching to keep up
//! with the telemetry stream.

use spiral_robotics_core::glam::{Mat4, Vec3};
use spiral_robotics_core::urdf::Geometry;
use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as Gl, WebGlProgram, WebGlUniformLocation,
    WebGlVertexArrayObject,
};

use crate::camera::Camera;
use crate::scene::{Condition, Entity, Interaction, Scene};

/// A light neutral bench matching `--tone-viewport` in the stylesheet.
const BACKGROUND: [f32; 4] = [0.965, 0.965, 0.955, 1.0];

/// A sparse 20 cm grid. The axes remain identifiable without dominating the
/// robot geometry or turning the viewer into graph paper.
const GRID_MAJOR: [f32; 4] = [0.84, 0.84, 0.82, 1.0];
const AXIS_X: [f32; 4] = [0.78, 0.50, 0.49, 1.0];
const AXIS_Y: [f32; 4] = [0.48, 0.68, 0.56, 1.0];

const VERTEX_SHADER: &str = r#"#version 300 es
in vec3 position;
in vec3 normal;
uniform mat4 model;
uniform mat4 view_projection;
out vec3 world_normal;
void main() {
    // Every shape is scaled along its own axes and its normals are axis
    // aligned, so rotating and renormalizing is the whole normal transform —
    // no inverse transpose is needed to keep the lighting honest.
    world_normal = normalize(mat3(model) * normal);
    gl_Position = view_projection * model * vec4(position, 1.0);
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;
in vec3 world_normal;
uniform vec4 color;
uniform float shaded;
out vec4 fragment;
void main() {
    // A broad bench light keeps white printed shells distinct from the pale
    // stage without the high-contrast facets of a dramatic product render.
    vec3 n = normalize(world_normal);
    float key = max(dot(n, normalize(vec3(0.45, -0.8, 0.9))), 0.0);
    float fill = max(dot(n, normalize(vec3(-0.7, 0.4, 0.2))), 0.0);
    vec3 lit = color.rgb * (0.70 + 0.26 * key + 0.08 * fill);
    fragment = vec4(mix(color.rgb, lit, shaded), color.a);
}
"#;

/// A vertex array holding interleaved positions and normals.
struct Mesh {
    array: WebGlVertexArrayObject,
    vertices: i32,
}

struct Uniforms {
    model: Option<WebGlUniformLocation>,
    view_projection: Option<WebGlUniformLocation>,
    color: Option<WebGlUniformLocation>,
    shaded: Option<WebGlUniformLocation>,
}

/// Everything the viewport needs to draw, held for as long as the canvas lives.
pub struct Renderer {
    gl: Gl,
    program: WebGlProgram,
    uniforms: Uniforms,
    cube: Mesh,
    cylinder: Mesh,
    sphere: Mesh,
    grid_major: Mesh,
    axis_x: Mesh,
    axis_y: Mesh,
    /// Framebuffer size in device pixels, so a resize is noticed on the next draw.
    size: (u32, u32),
}

impl Renderer {
    /// Set up WebGL on `canvas`, or say why the browser could not.
    pub fn new(canvas: &HtmlCanvasElement) -> Result<Self, String> {
        let gl = canvas
            .get_context("webgl2")
            .map_err(|_| "this browser refused a WebGL2 context".to_owned())?
            .ok_or_else(|| "this browser has no WebGL2 support".to_owned())?
            .dyn_into::<Gl>()
            .map_err(|_| "the canvas returned something that is not WebGL2".to_owned())?;

        let program = link_program(&gl)?;
        gl.use_program(Some(&program));
        let uniforms = Uniforms {
            model: gl.get_uniform_location(&program, "model"),
            view_projection: gl.get_uniform_location(&program, "view_projection"),
            color: gl.get_uniform_location(&program, "color"),
            shaded: gl.get_uniform_location(&program, "shaded"),
        };

        let cube = mesh(&gl, &program, &cube_vertices())?;
        let cylinder = mesh(&gl, &program, &cylinder_vertices(28))?;
        let sphere = mesh(&gl, &program, &sphere_vertices(12, 18))?;
        let grid_major = mesh(&gl, &program, &grid_vertices())?;
        let axis_x = mesh(&gl, &program, &axis_vertices(true))?;
        let axis_y = mesh(&gl, &program, &axis_vertices(false))?;

        gl.enable(Gl::DEPTH_TEST);
        gl.clear_color(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], BACKGROUND[3]);

        Ok(Self {
            gl,
            program,
            uniforms,
            cube,
            cylinder,
            sphere,
            grid_major,
            axis_x,
            axis_y,
            size: (0, 0),
        })
    }

    /// Match the drawing buffer to the space the canvas actually occupies.
    ///
    /// Returns the aspect ratio, or `None` while the canvas has no area — a
    /// panel that is still laid out at zero height would otherwise produce a
    /// degenerate projection.
    pub fn resize(&mut self, canvas: &HtmlCanvasElement, pixel_ratio: f64) -> Option<f32> {
        let scale = pixel_ratio.clamp(1.0, 3.0);
        let width = (canvas.client_width().max(0) as f64 * scale).round() as u32;
        let height = (canvas.client_height().max(0) as f64 * scale).round() as u32;
        if width == 0 || height == 0 {
            return None;
        }
        if self.size != (width, height) {
            canvas.set_width(width);
            canvas.set_height(height);
            self.gl.viewport(0, 0, width as i32, height as i32);
            self.size = (width, height);
        }
        Some(width as f32 / height as f32)
    }

    /// Draw one frame.
    pub fn draw(&self, camera: &Camera, aspect: f32, scene: &Scene) {
        let gl = &self.gl;
        gl.clear(Gl::COLOR_BUFFER_BIT | Gl::DEPTH_BUFFER_BIT);
        gl.use_program(Some(&self.program));
        gl.uniform_matrix4fv_with_f32_array(
            self.uniforms.view_projection.as_ref(),
            false,
            &camera.view_projection(aspect).to_cols_array(),
        );

        if scene.grid {
            // One sparse grid rhythm communicates scale without turning the
            // largest surface in the studio into a high-frequency texture.
            gl.uniform1f(self.uniforms.shaded.as_ref(), 0.0);
            gl.uniform_matrix4fv_with_f32_array(
                self.uniforms.model.as_ref(),
                false,
                &Mat4::IDENTITY.to_cols_array(),
            );
            let draw_lines = |mesh: &Mesh, color: &[f32; 4]| {
                gl.uniform4fv_with_f32_array(self.uniforms.color.as_ref(), color);
                gl.bind_vertex_array(Some(&mesh.array));
                gl.draw_arrays(Gl::LINES, 0, mesh.vertices);
            };
            draw_lines(&self.grid_major, &GRID_MAJOR);
            draw_lines(&self.axis_x, &AXIS_X);
            draw_lines(&self.axis_y, &AXIS_Y);
        }

        for entity in &scene.entities {
            let (mesh, scale) = match &entity.geometry {
                Geometry::Box { size } => (&self.cube, *size),
                Geometry::Cylinder { radius, length } => {
                    (&self.cylinder, Vec3::new(*radius, *radius, *length))
                }
                Geometry::Sphere { radius } => (&self.sphere, Vec3::splat(*radius)),
                // Nothing in this build loads mesh files; skipping keeps a
                // description that references one drawable rather than blank.
                Geometry::Mesh { .. } => continue,
            };
            gl.uniform1f(
                self.uniforms.shaded.as_ref(),
                if entity.wireframe { 0.0 } else { 1.0 },
            );
            gl.uniform4fv_with_f32_array(self.uniforms.color.as_ref(), &state_color(entity));
            gl.uniform_matrix4fv_with_f32_array(
                self.uniforms.model.as_ref(),
                false,
                &(entity.transform * Mat4::from_scale(scale)).to_cols_array(),
            );
            gl.bind_vertex_array(Some(&mesh.array));
            gl.draw_arrays(
                if entity.wireframe {
                    Gl::LINE_STRIP
                } else {
                    Gl::TRIANGLES
                },
                0,
                mesh.vertices,
            );
        }
        gl.bind_vertex_array(None);
    }
}

/// How a hovered, selected, or unwell entity is painted.
///
/// Hover lifts less than selection, so the two are never mistaken for each
/// other: a pointer passing over the bench should feel like a highlight and a
/// click should feel like a decision. Condition is a wash toward the studio's
/// caution and danger inks rather than a replacement colour, because an arm's
/// accent is the thing that says which arm it is, and a hot elbow is still that
/// arm's elbow.
fn state_color(entity: &Entity) -> [f32; 4] {
    let color = match entity.interaction {
        Interaction::Resting => entity.color,
        Interaction::Hovered => lift(entity.color, 1.10, 0.04),
        Interaction::Selected => lift(entity.color, 1.22, 0.08),
    };
    match entity.condition {
        Condition::Nominal => color,
        Condition::Caution => toward(color, CAUTION),
        Condition::Fault => toward(color, DANGER),
    }
}

/// Bench ochre and brick red, matching the studio's caution and danger tokens.
const CAUTION: [f32; 3] = [0.49, 0.42, 0.25];
const DANGER: [f32; 3] = [0.75, 0.26, 0.24];

fn lift(color: [f32; 4], gain: f32, floor: f32) -> [f32; 4] {
    [
        (color[0] * gain + floor).min(1.0),
        (color[1] * gain + floor).min(1.0),
        (color[2] * gain + floor).min(1.0),
        color[3],
    ]
}

/// Mix a colour most of the way toward a state ink, keeping some of its own.
fn toward(color: [f32; 4], target: [f32; 3]) -> [f32; 4] {
    const MIX: f32 = 0.62;
    [
        color[0] + (target[0] - color[0]) * MIX,
        color[1] + (target[1] - color[1]) * MIX,
        color[2] + (target[2] - color[2]) * MIX,
        color[3],
    ]
}

fn link_program(gl: &Gl) -> Result<WebGlProgram, String> {
    let vertex = compile(gl, Gl::VERTEX_SHADER, VERTEX_SHADER)?;
    let fragment = compile(gl, Gl::FRAGMENT_SHADER, FRAGMENT_SHADER)?;
    let program = gl.create_program().ok_or("could not create a GL program")?;
    gl.attach_shader(&program, &vertex);
    gl.attach_shader(&program, &fragment);
    gl.link_program(&program);
    if gl
        .get_program_parameter(&program, Gl::LINK_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        // The program holds them now, so the shader objects can go.
        gl.delete_shader(Some(&vertex));
        gl.delete_shader(Some(&fragment));
        Ok(program)
    } else {
        Err(gl
            .get_program_info_log(&program)
            .unwrap_or_else(|| "the GL program would not link".to_owned()))
    }
}

fn compile(gl: &Gl, kind: u32, source: &str) -> Result<web_sys::WebGlShader, String> {
    let shader = gl.create_shader(kind).ok_or("could not create a shader")?;
    gl.shader_source(&shader, source);
    gl.compile_shader(&shader);
    if gl
        .get_shader_parameter(&shader, Gl::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(shader)
    } else {
        Err(gl
            .get_shader_info_log(&shader)
            .unwrap_or_else(|| "a shader would not compile".to_owned()))
    }
}

/// Upload interleaved position and normal triples as a vertex array.
fn mesh(gl: &Gl, program: &WebGlProgram, data: &[f32]) -> Result<Mesh, String> {
    let array = gl
        .create_vertex_array()
        .ok_or("could not create a vertex array")?;
    gl.bind_vertex_array(Some(&array));

    let buffer = gl.create_buffer().ok_or("could not create a buffer")?;
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&buffer));
    // SAFETY: the view aliases the wasm heap, so it must reach WebGL before
    // anything can allocate and move that heap out from under it. Nothing
    // between the two lines allocates.
    unsafe {
        let view = js_sys::Float32Array::view(data);
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::STATIC_DRAW);
    }

    let stride = 6 * size_of::<f32>() as i32;
    for (name, offset) in [("position", 0), ("normal", 3 * size_of::<f32>() as i32)] {
        let location = gl.get_attrib_location(program, name);
        if location < 0 {
            return Err(format!("the shader has no {name} attribute"));
        }
        let location = location as u32;
        gl.vertex_attrib_pointer_with_i32(location, 3, Gl::FLOAT, false, stride, offset);
        gl.enable_vertex_attrib_array(location);
    }

    gl.bind_vertex_array(None);
    Ok(Mesh {
        array,
        vertices: (data.len() / 6) as i32,
    })
}

/// Append one vertex to an interleaved position-and-normal buffer.
fn push(data: &mut Vec<f32>, position: Vec3, normal: Vec3) {
    data.extend_from_slice(&[
        position.x, position.y, position.z, normal.x, normal.y, normal.z,
    ]);
}

/// A unit cube centered on the origin, so a box scales to its stated size.
fn cube_vertices() -> Vec<f32> {
    let mut data = Vec::with_capacity(36 * 6);
    for axis in 0..3 {
        for sign in [1.0_f32, -1.0] {
            let mut normal = Vec3::ZERO;
            normal[axis] = sign;
            // The two directions spanning this face, picked so the pair with
            // the normal is right handed and the winding stays consistent.
            let mut u = Vec3::ZERO;
            let mut v = Vec3::ZERO;
            u[(axis + 1) % 3] = 0.5;
            v[(axis + 2) % 3] = 0.5 * sign;
            let center = normal * 0.5;
            let corners = [
                center - u - v,
                center + u - v,
                center + u + v,
                center - u + v,
            ];
            for index in [0, 1, 2, 0, 2, 3] {
                push(&mut data, corners[index], normal);
            }
        }
    }
    data
}

/// A unit cylinder about the Z axis: radius one, length one, centered.
fn cylinder_vertices(segments: usize) -> Vec<f32> {
    let mut data = Vec::with_capacity(segments * 12 * 6);
    let angle = |index: usize| index as f32 / segments as f32 * std::f32::consts::TAU;
    for index in 0..segments {
        let (first, second) = (angle(index), angle(index + 1));
        let (near, far) = (
            Vec3::new(first.cos(), first.sin(), 0.0),
            Vec3::new(second.cos(), second.sin(), 0.0),
        );
        let half = Vec3::new(0.0, 0.0, 0.5);

        // Side, as two triangles with normals pointing straight out.
        push(&mut data, near - half, near);
        push(&mut data, far - half, far);
        push(&mut data, far + half, far);
        push(&mut data, near - half, near);
        push(&mut data, far + half, far);
        push(&mut data, near + half, near);

        // Caps, as fans from the centre of each end.
        push(&mut data, half, Vec3::Z);
        push(&mut data, near + half, Vec3::Z);
        push(&mut data, far + half, Vec3::Z);
        push(&mut data, -half, -Vec3::Z);
        push(&mut data, far - half, -Vec3::Z);
        push(&mut data, near - half, -Vec3::Z);
    }
    data
}

/// A unit sphere, as a latitude and longitude grid of quads.
fn sphere_vertices(rings: usize, segments: usize) -> Vec<f32> {
    let mut data = Vec::with_capacity(rings * segments * 6 * 6);
    let point = |ring: usize, segment: usize| {
        let polar = ring as f32 / rings as f32 * std::f32::consts::PI;
        let azimuth = segment as f32 / segments as f32 * std::f32::consts::TAU;
        Vec3::new(
            polar.sin() * azimuth.cos(),
            polar.sin() * azimuth.sin(),
            polar.cos(),
        )
    };
    for ring in 0..rings {
        for segment in 0..segments {
            let corners = [
                point(ring, segment),
                point(ring + 1, segment),
                point(ring + 1, segment + 1),
                point(ring, segment + 1),
            ];
            for index in [0, 1, 2, 0, 2, 3] {
                // On a unit sphere the position is the normal.
                push(&mut data, corners[index], corners[index]);
            }
        }
    }
    data
}

/// The bench the arms stand on: one quiet 20 cm rhythm.
fn grid_vertices() -> Vec<f32> {
    const HALF: f32 = 0.75;
    const STEP: f32 = 0.05;
    let mut data = Vec::new();
    let lines = (HALF / STEP) as i32;
    for index in -lines..=lines {
        if index == 0 || index % 4 != 0 {
            continue;
        }
        let offset = index as f32 * STEP;
        for (from, to) in [
            (Vec3::new(offset, -HALF, 0.0), Vec3::new(offset, HALF, 0.0)),
            (Vec3::new(-HALF, offset, 0.0), Vec3::new(HALF, offset, 0.0)),
        ] {
            push(&mut data, from, Vec3::Z);
            push(&mut data, to, Vec3::Z);
        }
    }
    data
}

fn axis_vertices(x_axis: bool) -> Vec<f32> {
    const HALF: f32 = 0.75;
    let mut data = Vec::new();
    let (from, to) = if x_axis {
        (Vec3::new(-HALF, 0.0, 0.001), Vec3::new(HALF, 0.0, 0.001))
    } else {
        (Vec3::new(0.0, -HALF, 0.001), Vec3::new(0.0, HALF, 0.001))
    };
    push(&mut data, from, Vec3::Z);
    push(&mut data, to, Vec3::Z);
    data
}
