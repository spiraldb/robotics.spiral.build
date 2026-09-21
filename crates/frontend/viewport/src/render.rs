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

use glam::{Mat4, Vec3};

use crate::urdf::Geometry;
use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlUniformLocation, WebGlVertexArrayObject,
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
    // Imported meshes may use nonuniform scale.
    world_normal = normalize(transpose(inverse(mat3(model))) * normal);
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
struct GpuMesh {
    context: Gl,
    array: WebGlVertexArrayObject,
    buffer: WebGlBuffer,
    vertices: i32,
}

impl Drop for GpuMesh {
    fn drop(&mut self) {
        self.context.delete_vertex_array(Some(&self.array));
        self.context.delete_buffer(Some(&self.buffer));
    }
}

struct Uniforms {
    model: Option<WebGlUniformLocation>,
    view_projection: Option<WebGlUniformLocation>,
    color: Option<WebGlUniformLocation>,
    shaded: Option<WebGlUniformLocation>,
}

/// Everything the viewport needs to draw, held for as long as the canvas lives.
pub(crate) struct Renderer {
    pipeline: WebGlPipeline,
    primitives: PrimitiveMeshes,
    bench: BenchMeshes,
    meshes: std::collections::BTreeMap<String, GpuMesh>,
}

struct WebGlPipeline {
    context: Gl,
    program: WebGlProgram,
    uniforms: Uniforms,
    /// Framebuffer size in device pixels, so a resize is noticed on the next draw.
    framebuffer_size: (u32, u32),
}

impl Drop for WebGlPipeline {
    fn drop(&mut self) {
        self.context.delete_program(Some(&self.program));
    }
}

struct PrimitiveMeshes {
    cube: GpuMesh,
    cylinder: GpuMesh,
    sphere: GpuMesh,
}

struct BenchMeshes {
    major_grid: GpuMesh,
    x_axis: GpuMesh,
    y_axis: GpuMesh,
}

pub(crate) struct RenderFrame<'a> {
    camera: &'a Camera,
    aspect_ratio: f32,
    scene: &'a Scene,
}

impl<'a> RenderFrame<'a> {
    pub(crate) fn new(camera: &'a Camera, aspect_ratio: f32, scene: &'a Scene) -> Self {
        Self {
            camera,
            aspect_ratio,
            scene,
        }
    }
}

impl Renderer {
    pub(crate) fn load_meshes(
        &mut self,
        meshes: &std::collections::BTreeMap<String, Vec<f32>>,
    ) -> Result<(), String> {
        self.meshes = meshes
            .iter()
            .map(|(name, values)| {
                Ok((
                    name.clone(),
                    GpuMesh::upload(
                        &self.pipeline,
                        &MeshVertices {
                            values: values.clone(),
                        },
                    )?,
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(())
    }
    /// Set up WebGL on `canvas`, or say why the browser could not.
    pub(crate) fn new(canvas: &HtmlCanvasElement) -> Result<Self, String> {
        let pipeline = WebGlPipeline::new(canvas)?;
        let primitives = PrimitiveMeshes::new(&pipeline)?;
        let bench = BenchMeshes::new(&pipeline)?;

        Ok(Self {
            pipeline,
            primitives,
            bench,
            meshes: std::collections::BTreeMap::new(),
        })
    }

    /// Match the drawing buffer to the space the canvas actually occupies.
    ///
    /// Returns the aspect ratio, or `None` while the canvas has no area — a
    /// panel that is still laid out at zero height would otherwise produce a
    /// degenerate projection.
    pub(crate) fn resize(&mut self, canvas: &HtmlCanvasElement, pixel_ratio: f64) -> Option<f32> {
        self.pipeline.resize(canvas, pixel_ratio)
    }

    /// Draw one frame.
    pub(crate) fn draw(&self, frame: RenderFrame<'_>) {
        let gl = &self.pipeline.context;
        gl.clear(Gl::COLOR_BUFFER_BIT | Gl::DEPTH_BUFFER_BIT);
        gl.use_program(Some(&self.pipeline.program));
        gl.uniform_matrix4fv_with_f32_array(
            self.pipeline.uniforms.view_projection.as_ref(),
            false,
            &frame
                .camera
                .view_projection(frame.aspect_ratio)
                .to_cols_array(),
        );

        if frame.scene.grid_enabled() {
            // One sparse grid rhythm communicates scale without turning the
            // largest surface in the studio into a high-frequency texture.
            gl.uniform1f(self.pipeline.uniforms.shaded.as_ref(), 0.0);
            gl.uniform_matrix4fv_with_f32_array(
                self.pipeline.uniforms.model.as_ref(),
                false,
                &Mat4::IDENTITY.to_cols_array(),
            );
            let draw_lines = |mesh: &GpuMesh, color: &[f32; 4]| {
                gl.uniform4fv_with_f32_array(self.pipeline.uniforms.color.as_ref(), color);
                gl.bind_vertex_array(Some(&mesh.array));
                gl.draw_arrays(Gl::LINES, 0, mesh.vertices);
            };
            draw_lines(&self.bench.major_grid, &GRID_MAJOR);
            draw_lines(&self.bench.x_axis, &AXIS_X);
            draw_lines(&self.bench.y_axis, &AXIS_Y);
        }

        for entity in frame.scene.entities() {
            let (mesh, scale) = match entity.geometry() {
                Geometry::Box { size } => (&self.primitives.cube, *size),
                Geometry::Cylinder { radius, length } => (
                    &self.primitives.cylinder,
                    Vec3::new(*radius, *radius, *length),
                ),
                Geometry::Sphere { radius } => (&self.primitives.sphere, Vec3::splat(*radius)),
                Geometry::Mesh { filename, scale } => match self.meshes.get(filename) {
                    Some(mesh) => (mesh, *scale),
                    None => continue,
                },
            };
            gl.uniform1f(
                self.pipeline.uniforms.shaded.as_ref(),
                if entity.is_wireframe() { 0.0 } else { 1.0 },
            );
            gl.uniform4fv_with_f32_array(
                self.pipeline.uniforms.color.as_ref(),
                &entity.render_color(),
            );
            gl.uniform_matrix4fv_with_f32_array(
                self.pipeline.uniforms.model.as_ref(),
                false,
                &(entity.transform() * Mat4::from_scale(scale)).to_cols_array(),
            );
            gl.bind_vertex_array(Some(&mesh.array));
            gl.draw_arrays(
                if entity.is_wireframe() {
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

impl WebGlPipeline {
    fn new(canvas: &HtmlCanvasElement) -> Result<Self, String> {
        let context = canvas
            .get_context("webgl2")
            .map_err(|_| "this browser refused a WebGL2 context".to_owned())?
            .ok_or_else(|| "this browser has no WebGL2 support".to_owned())?
            .dyn_into::<Gl>()
            .map_err(|_| "the canvas returned something that is not WebGL2".to_owned())?;

        let program = Self::link_program(&context)?;
        context.use_program(Some(&program));
        let uniforms = Uniforms {
            model: context.get_uniform_location(&program, "model"),
            view_projection: context.get_uniform_location(&program, "view_projection"),
            color: context.get_uniform_location(&program, "color"),
            shaded: context.get_uniform_location(&program, "shaded"),
        };

        context.enable(Gl::DEPTH_TEST);
        context.clear_color(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], BACKGROUND[3]);

        Ok(Self {
            context,
            program,
            uniforms,
            framebuffer_size: (0, 0),
        })
    }

    fn resize(&mut self, canvas: &HtmlCanvasElement, pixel_ratio: f64) -> Option<f32> {
        let scale = pixel_ratio.clamp(1.0, 3.0);
        let width = (canvas.client_width().max(0) as f64 * scale).round() as u32;
        let height = (canvas.client_height().max(0) as f64 * scale).round() as u32;
        if width == 0 || height == 0 {
            return None;
        }
        if self.framebuffer_size != (width, height) {
            canvas.set_width(width);
            canvas.set_height(height);
            self.context.viewport(0, 0, width as i32, height as i32);
            self.framebuffer_size = (width, height);
        }
        Some(width as f32 / height as f32)
    }

    fn link_program(context: &Gl) -> Result<WebGlProgram, String> {
        let vertex_shader =
            Self::compile_shader(context, ShaderSource::new(Gl::VERTEX_SHADER, VERTEX_SHADER))?;
        let fragment_shader = Self::compile_shader(
            context,
            ShaderSource::new(Gl::FRAGMENT_SHADER, FRAGMENT_SHADER),
        )?;
        let program = context
            .create_program()
            .ok_or("could not create a GL program")?;
        context.attach_shader(&program, &vertex_shader);
        context.attach_shader(&program, &fragment_shader);
        context.link_program(&program);
        if context
            .get_program_parameter(&program, Gl::LINK_STATUS)
            .as_bool()
            .unwrap_or(false)
        {
            // The program holds them now, so the shader objects can go.
            context.delete_shader(Some(&vertex_shader));
            context.delete_shader(Some(&fragment_shader));
            Ok(program)
        } else {
            Err(context
                .get_program_info_log(&program)
                .unwrap_or_else(|| "the GL program would not link".to_owned()))
        }
    }

    fn compile_shader(
        context: &Gl,
        shader_source: ShaderSource<'_>,
    ) -> Result<WebGlShader, String> {
        let shader = context
            .create_shader(shader_source.kind)
            .ok_or("could not create a shader")?;
        context.shader_source(&shader, shader_source.code);
        context.compile_shader(&shader);
        if context
            .get_shader_parameter(&shader, Gl::COMPILE_STATUS)
            .as_bool()
            .unwrap_or(false)
        {
            Ok(shader)
        } else {
            Err(context
                .get_shader_info_log(&shader)
                .unwrap_or_else(|| "a shader would not compile".to_owned()))
        }
    }
}

/// Bench ochre and brick red, matching the studio's caution and danger tokens.
const CAUTION: [f32; 3] = [0.49, 0.42, 0.25];
const DANGER: [f32; 3] = [0.75, 0.26, 0.24];

struct ShaderSource<'a> {
    kind: u32,
    code: &'a str,
}

impl<'a> ShaderSource<'a> {
    fn new(kind: u32, code: &'a str) -> Self {
        Self { kind, code }
    }
}

impl PrimitiveMeshes {
    fn new(pipeline: &WebGlPipeline) -> Result<Self, String> {
        Ok(Self {
            cube: GpuMesh::upload(pipeline, &MeshVertices::cube())?,
            cylinder: GpuMesh::upload(pipeline, &MeshVertices::cylinder(28))?,
            sphere: GpuMesh::upload(pipeline, &MeshVertices::sphere(12, 18))?,
        })
    }
}

impl BenchMeshes {
    fn new(pipeline: &WebGlPipeline) -> Result<Self, String> {
        Ok(Self {
            major_grid: GpuMesh::upload(pipeline, &MeshVertices::bench_grid())?,
            x_axis: GpuMesh::upload(pipeline, &MeshVertices::axis(GridAxis::X))?,
            y_axis: GpuMesh::upload(pipeline, &MeshVertices::axis(GridAxis::Y))?,
        })
    }
}

impl GpuMesh {
    /// Upload interleaved position and normal triples as a vertex array.
    fn upload(pipeline: &WebGlPipeline, vertices: &MeshVertices) -> Result<Self, String> {
        let gl = &pipeline.context;
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
            let vertex_view = js_sys::Float32Array::view(&vertices.values);
            gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &vertex_view, Gl::STATIC_DRAW);
        }

        let stride = 6 * size_of::<f32>() as i32;
        for (attribute_name, offset) in [("position", 0), ("normal", 3 * size_of::<f32>() as i32)] {
            let location = gl.get_attrib_location(&pipeline.program, attribute_name);
            if location < 0 {
                return Err(format!("the shader has no {attribute_name} attribute"));
            }
            let location = location as u32;
            gl.vertex_attrib_pointer_with_i32(location, 3, Gl::FLOAT, false, stride, offset);
            gl.enable_vertex_attrib_array(location);
        }

        gl.bind_vertex_array(None);
        Ok(Self {
            context: gl.clone(),
            array,
            buffer,
            vertices: (vertices.values.len() / 6) as i32,
        })
    }
}

/// A CPU-side interleaved position-and-normal buffer.
struct MeshVertices {
    values: Vec<f32>,
}

impl MeshVertices {
    /// A unit cube centered on the origin, so a box scales to its stated size.
    fn cube() -> Self {
        let mut vertices = Self::with_capacity(36 * 6);
        for axis_index in 0..3 {
            for normal_sign in [1.0_f32, -1.0] {
                let mut normal = Vec3::ZERO;
                normal[axis_index] = normal_sign;
                // The two directions spanning this face, picked so the pair
                // with the normal is right handed and winding stays consistent.
                let mut first_span = Vec3::ZERO;
                let mut second_span = Vec3::ZERO;
                first_span[(axis_index + 1) % 3] = 0.5;
                second_span[(axis_index + 2) % 3] = 0.5 * normal_sign;
                let center = normal * 0.5;
                let corners = [
                    center - first_span - second_span,
                    center + first_span - second_span,
                    center + first_span + second_span,
                    center - first_span + second_span,
                ];
                for corner_index in [0, 1, 2, 0, 2, 3] {
                    vertices.push(corners[corner_index], normal);
                }
            }
        }
        vertices
    }

    /// A unit cylinder about the Z axis: radius one, length one, centered.
    fn cylinder(segment_count: usize) -> Self {
        let mut vertices = Self::with_capacity(segment_count * 12 * 6);
        let angle = |segment_index: usize| {
            segment_index as f32 / segment_count as f32 * std::f32::consts::TAU
        };
        for segment_index in 0..segment_count {
            let (first_angle, second_angle) = (angle(segment_index), angle(segment_index + 1));
            let (first_edge, second_edge) = (
                Vec3::new(first_angle.cos(), first_angle.sin(), 0.0),
                Vec3::new(second_angle.cos(), second_angle.sin(), 0.0),
            );
            let half_height = Vec3::new(0.0, 0.0, 0.5);

            // Side, as two triangles with normals pointing straight out.
            vertices.push(first_edge - half_height, first_edge);
            vertices.push(second_edge - half_height, second_edge);
            vertices.push(second_edge + half_height, second_edge);
            vertices.push(first_edge - half_height, first_edge);
            vertices.push(second_edge + half_height, second_edge);
            vertices.push(first_edge + half_height, first_edge);

            // Caps, as fans from the centre of each end.
            vertices.push(half_height, Vec3::Z);
            vertices.push(first_edge + half_height, Vec3::Z);
            vertices.push(second_edge + half_height, Vec3::Z);
            vertices.push(-half_height, -Vec3::Z);
            vertices.push(second_edge - half_height, -Vec3::Z);
            vertices.push(first_edge - half_height, -Vec3::Z);
        }
        vertices
    }

    /// A unit sphere, as a latitude and longitude grid of quads.
    fn sphere(ring_count: usize, segment_count: usize) -> Self {
        let mut vertices = Self::with_capacity(ring_count * segment_count * 6 * 6);
        let point = |ring_index: usize, segment_index: usize| {
            let polar_angle = ring_index as f32 / ring_count as f32 * std::f32::consts::PI;
            let azimuth = segment_index as f32 / segment_count as f32 * std::f32::consts::TAU;
            Vec3::new(
                polar_angle.sin() * azimuth.cos(),
                polar_angle.sin() * azimuth.sin(),
                polar_angle.cos(),
            )
        };
        for ring_index in 0..ring_count {
            for segment_index in 0..segment_count {
                let corners = [
                    point(ring_index, segment_index),
                    point(ring_index + 1, segment_index),
                    point(ring_index + 1, segment_index + 1),
                    point(ring_index, segment_index + 1),
                ];
                for corner_index in [0, 1, 2, 0, 2, 3] {
                    // On a unit sphere the position is the normal.
                    vertices.push(corners[corner_index], corners[corner_index]);
                }
            }
        }
        vertices
    }

    /// The bench the arms stand on: one quiet 20 cm rhythm.
    fn bench_grid() -> Self {
        const HALF_EXTENT: f32 = 0.75;
        const STEP: f32 = 0.05;
        let mut vertices = Self::with_capacity(48 * 6);
        let line_count = (HALF_EXTENT / STEP) as i32;
        for line_index in -line_count..=line_count {
            if line_index == 0 || line_index % 4 != 0 {
                continue;
            }
            let offset = line_index as f32 * STEP;
            for (line_start, line_end) in [
                (
                    Vec3::new(offset, -HALF_EXTENT, 0.0),
                    Vec3::new(offset, HALF_EXTENT, 0.0),
                ),
                (
                    Vec3::new(-HALF_EXTENT, offset, 0.0),
                    Vec3::new(HALF_EXTENT, offset, 0.0),
                ),
            ] {
                vertices.push(line_start, Vec3::Z);
                vertices.push(line_end, Vec3::Z);
            }
        }
        vertices
    }

    fn axis(axis: GridAxis) -> Self {
        const HALF_EXTENT: f32 = 0.75;
        let mut vertices = Self::with_capacity(2 * 6);
        let (line_start, line_end) = match axis {
            GridAxis::X => (
                Vec3::new(-HALF_EXTENT, 0.0, 0.001),
                Vec3::new(HALF_EXTENT, 0.0, 0.001),
            ),
            GridAxis::Y => (
                Vec3::new(0.0, -HALF_EXTENT, 0.001),
                Vec3::new(0.0, HALF_EXTENT, 0.001),
            ),
        };
        vertices.push(line_start, Vec3::Z);
        vertices.push(line_end, Vec3::Z);
        vertices
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            values: Vec::with_capacity(capacity),
        }
    }

    /// Append one vertex to the interleaved position-and-normal buffer.
    fn push(&mut self, position: Vec3, normal: Vec3) {
        self.values.extend_from_slice(&[
            position.x, position.y, position.z, normal.x, normal.y, normal.z,
        ]);
    }
}

#[derive(Clone, Copy)]
enum GridAxis {
    X,
    Y,
}

#[derive(Clone, Copy)]
struct RenderColor([f32; 4]);

impl RenderColor {
    fn lifted(self, gain: f32, floor: f32) -> Self {
        Self([
            (self.0[0] * gain + floor).min(1.0),
            (self.0[1] * gain + floor).min(1.0),
            (self.0[2] * gain + floor).min(1.0),
            self.0[3],
        ])
    }

    /// Mix most of the way toward a state ink, keeping some of the original.
    fn blended_toward(self, target: [f32; 3]) -> Self {
        const MIX: f32 = 0.62;
        Self([
            self.0[0] + (target[0] - self.0[0]) * MIX,
            self.0[1] + (target[1] - self.0[1]) * MIX,
            self.0[2] + (target[2] - self.0[2]) * MIX,
            self.0[3],
        ])
    }
}

impl Entity {
    /// How a hovered, selected, or unwell entity is painted.
    fn render_color(&self) -> [f32; 4] {
        let base_color = RenderColor(self.color());
        let interaction_color = match self.interaction() {
            Interaction::Resting => base_color,
            Interaction::Hovered => base_color.lifted(1.10, 0.04),
            Interaction::Selected => base_color.lifted(1.22, 0.08),
        };
        match self.condition() {
            Condition::Nominal => interaction_color,
            Condition::Caution => interaction_color.blended_toward(CAUTION),
            Condition::Fault => interaction_color.blended_toward(DANGER),
        }
        .0
    }
}
