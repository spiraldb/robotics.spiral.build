//! The wgpu half of the bench.
//!
//! This module knows about triangles and nothing about robots: it is handed a
//! view-projection and a list of already-placed shapes, and it draws them into
//! a texture — a browser canvas's or an off-screen picture's alike. Deciding
//! which shapes those are, and where, belongs to [`crate::robot`].
//!
//! The primitives a URDF is built from are drawn from three unit meshes — a
//! box, a cylinder, and a sphere — scaled per instance. A whole arm is around
//! twenty draw calls, so each draw simply gets its own slice of one uniform
//! buffer; nothing here needs instancing or batching to keep up.

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt as _;

use crate::RobotModel;
use crate::robot::Frame;
use crate::scene::{Condition, Entity, Interaction, Scene};
use crate::urdf::Geometry;

/// Why the bench could not be drawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewportError {
    Rendering(String),
}

impl fmt::Display for ViewportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rendering(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for ViewportError {}

/// A light neutral bench matching `--tone-viewport` in the stylesheet.
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.965,
    g: 0.965,
    b: 0.955,
    a: 1.0,
};

/// A sparse 20 cm grid. The axes remain identifiable without dominating the
/// robot geometry or turning the viewer into graph paper.
const GRID_MAJOR: [f32; 4] = [0.84, 0.84, 0.82, 1.0];
const AXIS_X: [f32; 4] = [0.78, 0.50, 0.49, 1.0];
const AXIS_Y: [f32; 4] = [0.48, 0.68, 0.56, 1.0];

/// Edges stay smooth at every pixel ratio, as the browser's own WebGL
/// antialiasing kept them before.
const SAMPLE_COUNT: u32 = 4;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

const SHADER: &str = r#"
struct Draw {
    view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
    // Imported meshes may use nonuniform scale, so normals get the inverse
    // transpose, which WGSL leaves to the CPU.
    normal_matrix: mat4x4<f32>,
    color: vec4<f32>,
    shaded: f32,
}

@group(0) @binding(0) var<uniform> draw: Draw;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
}

@vertex
fn vertex_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> Varyings {
    var out: Varyings;
    out.world_normal = normalize((draw.normal_matrix * vec4<f32>(normal, 0.0)).xyz);
    out.position = draw.view_projection * draw.model * vec4<f32>(position, 1.0);
    return out;
}

@fragment
fn fragment_main(in: Varyings) -> @location(0) vec4<f32> {
    // A broad bench light keeps white printed shells distinct from the pale
    // stage without the high-contrast facets of a dramatic product render.
    let n = normalize(in.world_normal);
    let key = max(dot(n, normalize(vec3<f32>(0.45, -0.8, 0.9))), 0.0);
    let fill = max(dot(n, normalize(vec3<f32>(-0.7, 0.4, 0.2))), 0.0);
    let lit = draw.color.rgb * (0.70 + 0.26 * key + 0.08 * fill);
    return vec4<f32>(mix(draw.color.rgb, lit, draw.shaded), draw.color.a);
}
"#;

/// A device and the queue its work goes to.
pub(crate) struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Gpu {
    /// Open a device, on an adapter able to present to `surface` when there
    /// is one, and return that adapter beside it.
    pub(crate) async fn request(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<(Self, wgpu::Adapter), ViewportError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: surface,
                ..Default::default()
            })
            .await
            .map_err(|error| ViewportError::Rendering(format!("no graphics adapter: {error}")))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("bench"),
                // WebGL2 is the floor every target clears; only the largest
                // picture size follows what this adapter can actually do.
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|error| {
                ViewportError::Rendering(format!("the graphics device refused: {error}"))
            })?;
        Ok((Self { device, queue }, adapter))
    }

    pub(crate) const fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub(crate) const fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}

/// What one draw needs: where the shapes are seen from, and the shapes.
pub(crate) struct RenderFrame<'a> {
    view_projection: Mat4,
    scene: &'a Scene,
}

impl<'a> RenderFrame<'a> {
    pub(crate) fn new(view_projection: Mat4, scene: &'a Scene) -> Self {
        Self {
            view_projection,
            scene,
        }
    }
}

/// Everything the bench needs to draw, held for as long as its device lives.
pub(crate) struct Renderer {
    pipelines: Pipelines,
    meshes: MeshLibrary,
    uniforms: DrawUniforms,
    /// The multisampled attachments for the size last drawn.
    attachments: Option<Attachments>,
}

impl Renderer {
    /// Compile the pipelines for pictures in `format`.
    pub(crate) fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Self {
        let device = gpu.device();
        let uniforms = DrawUniforms::new(device, 64);
        let pipelines = Pipelines::new(device, format, uniforms.layout());
        Self {
            pipelines,
            meshes: MeshLibrary {
                primitives: PrimitiveMeshes::new(device),
                bench: BenchMeshes::new(device),
                robots: RobotMeshes::default(),
            },
            uniforms,
            attachments: None,
        }
    }

    /// Upload the meshes of the robots in `frame` unless they are already
    /// resident, and say whether the set of robots changed.
    pub(crate) fn load_models(&mut self, gpu: &Gpu, frame: &Frame) -> bool {
        self.meshes.robots.load(gpu.device(), frame)
    }

    /// Draw one frame into `target`, which must be in this renderer's format.
    pub(crate) fn draw(&mut self, gpu: &Gpu, target: &wgpu::Texture, frame: RenderFrame<'_>) {
        let size = (target.width(), target.height());
        let attachments = match self.attachments.take() {
            Some(attachments) if attachments.size == size => attachments,
            _ => Attachments::new(gpu.device(), self.pipelines.format, size),
        };

        let calls = self.meshes.draw_calls(&frame);
        self.uniforms.write(gpu, &calls, frame.view_projection);

        let resolve = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bench"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &attachments.color,
                    depth_slice: None,
                    resolve_target: Some(&resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &attachments.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for (index, call) in calls.iter().enumerate() {
                pass.set_pipeline(self.pipelines.for_topology(call.topology));
                pass.set_bind_group(
                    0,
                    self.uniforms.bind_group(),
                    &[self.uniforms.offset(index)],
                );
                pass.set_vertex_buffer(0, call.mesh.buffer.slice(..));
                pass.draw(0..call.mesh.vertices, 0..1);
            }
        }
        gpu.queue().submit([encoder.finish()]);
        self.attachments = Some(attachments);
    }
}

/// Every mesh a frame can name: the primitives, the bench, and robot parts.
struct MeshLibrary {
    primitives: PrimitiveMeshes,
    bench: BenchMeshes,
    robots: RobotMeshes,
}

impl MeshLibrary {
    /// The grid, then every entity, as meshes with their uniforms.
    fn draw_calls(&self, frame: &RenderFrame<'_>) -> Vec<DrawCall<'_>> {
        let mut calls = Vec::new();
        if frame.scene.grid_enabled() {
            // One sparse grid rhythm communicates scale without turning the
            // largest surface in the studio into a high-frequency texture.
            let grid = Mat4::from_translation(Vec3::new(0.0, 0.0, frame.scene.grid_height()));
            for (mesh, color) in [
                (&self.bench.major_grid, GRID_MAJOR),
                (&self.bench.x_axis, AXIS_X),
                (&self.bench.y_axis, AXIS_Y),
            ] {
                calls.push(DrawCall::new(mesh, Topology::Lines, grid, (color, false)));
            }
        }
        for entity in frame.scene.entities() {
            let (mesh, scale) = match entity.geometry() {
                Geometry::Box { size } => (&self.primitives.cube, *size),
                Geometry::Cylinder { radius, length } => (
                    &self.primitives.cylinder,
                    Vec3::new(*radius, *radius, *length),
                ),
                Geometry::Sphere { radius } => (&self.primitives.sphere, Vec3::splat(*radius)),
                Geometry::Mesh { filename, scale } => match self.robots.get(filename) {
                    Some(mesh) => (mesh, *scale),
                    None => continue,
                },
            };
            let (topology, shaded) = if entity.is_wireframe() {
                (Topology::LineStrip, false)
            } else {
                (Topology::Triangles, true)
            };
            calls.push(DrawCall::new(
                mesh,
                topology,
                entity.transform() * Mat4::from_scale(scale),
                (entity.render_color(), shaded),
            ));
        }
        calls
    }
}

/// The meshes of the robots in the last frame, keyed `robot/filename`.
#[derive(Default)]
struct RobotMeshes {
    models: Vec<(String, Rc<RobotModel>)>,
    meshes: BTreeMap<String, GpuMesh>,
}

impl RobotMeshes {
    fn load(&mut self, device: &wgpu::Device, frame: &Frame) -> bool {
        let models = frame
            .models()
            .map(|(id, model)| (id.to_owned(), model.clone()))
            .collect::<Vec<_>>();
        let unchanged = models.len() == self.models.len()
            && models
                .iter()
                .zip(&self.models)
                .all(|((id, model), (old_id, old))| id == old_id && Rc::ptr_eq(model, old));
        if unchanged {
            return false;
        }
        self.meshes = models
            .iter()
            .flat_map(|(id, model)| {
                model.meshes.iter().filter_map(move |(name, values)| {
                    let mesh = GpuMesh::upload(device, values)?;
                    Some((format!("{id}/{name}"), mesh))
                })
            })
            .collect();
        self.models = models;
        true
    }

    fn get(&self, filename: &str) -> Option<&GpuMesh> {
        self.meshes.get(filename)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Topology {
    Triangles,
    Lines,
    LineStrip,
}

/// One mesh to draw, and the uniforms it is drawn with.
struct DrawCall<'a> {
    mesh: &'a GpuMesh,
    topology: Topology,
    uniforms: DrawSlot,
}

impl<'a> DrawCall<'a> {
    fn new(
        mesh: &'a GpuMesh,
        topology: Topology,
        model: Mat4,
        (color, shaded): ([f32; 4], bool),
    ) -> Self {
        Self {
            mesh,
            topology,
            uniforms: DrawSlot::new(model, color, shaded),
        }
    }
}

/// The per-draw block of the shader's `Draw` uniform, minus the
/// view-projection every draw of a frame shares.
#[derive(Clone, Copy)]
struct DrawSlot {
    model: Mat4,
    color: [f32; 4],
    shaded: bool,
}

impl DrawSlot {
    /// Bytes in the shader's `Draw` struct: three matrices, a colour, and a
    /// float padded out to the struct's 16-byte alignment.
    const SIZE: u64 = (3 * 16 + 4 + 4) * 4;

    fn new(model: Mat4, color: [f32; 4], shaded: bool) -> Self {
        Self {
            model,
            color,
            shaded,
        }
    }

    fn write(&self, view_projection: Mat4, bytes: &mut [u8]) {
        let mut values = [0.0_f32; (Self::SIZE / 4) as usize];
        values[0..16].copy_from_slice(&view_projection.to_cols_array());
        values[16..32].copy_from_slice(&self.model.to_cols_array());
        values[32..48].copy_from_slice(&self.model.inverse().transpose().to_cols_array());
        values[48..52].copy_from_slice(&self.color);
        values[52] = if self.shaded { 1.0 } else { 0.0 };
        bytes[..Self::SIZE as usize].copy_from_slice(bytemuck::cast_slice(&values));
    }
}

/// One uniform buffer with a slot per draw, addressed by dynamic offset.
struct DrawUniforms {
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    capacity: usize,
    stride: u64,
}

impl DrawUniforms {
    fn new(device: &wgpu::Device, capacity: usize) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bench draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(DrawSlot::SIZE),
                },
                count: None,
            }],
        });
        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = DrawSlot::SIZE.div_ceil(alignment) * alignment;
        let (buffer, bind_group) = Self::allocate(device, &layout, capacity as u64 * stride);
        Self {
            layout,
            buffer,
            bind_group,
            capacity,
            stride,
        }
    }

    fn allocate(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        size: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench draw"),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bench draw"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(DrawSlot::SIZE),
                }),
            }],
        });
        (buffer, bind_group)
    }

    const fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    const fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    fn offset(&self, index: usize) -> u32 {
        (index as u64 * self.stride) as u32
    }

    /// Fill one slot per call, growing the buffer when a scene outgrows it.
    fn write(&mut self, gpu: &Gpu, calls: &[DrawCall<'_>], view_projection: Mat4) {
        if calls.len() > self.capacity {
            self.capacity = calls.len().next_power_of_two();
            (self.buffer, self.bind_group) = Self::allocate(
                gpu.device(),
                &self.layout,
                self.capacity as u64 * self.stride,
            );
        }
        let mut bytes = vec![0_u8; calls.len() * self.stride as usize];
        for (call, slot) in calls.iter().zip(bytes.chunks_mut(self.stride as usize)) {
            call.uniforms.write(view_projection, slot);
        }
        gpu.queue().write_buffer(&self.buffer, 0, &bytes);
    }
}

/// The three pipelines that differ only in how vertices become primitives.
struct Pipelines {
    format: wgpu::TextureFormat,
    triangles: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    line_strip: wgpu::RenderPipeline,
}

impl Pipelines {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        uniforms: &wgpu::BindGroupLayout,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bench"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bench"),
            bind_group_layouts: &[Some(uniforms)],
            immediate_size: 0,
        });
        let pipeline = |topology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("bench"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vertex_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 6 * size_of::<f32>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fragment_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            format,
            triangles: pipeline(wgpu::PrimitiveTopology::TriangleList),
            lines: pipeline(wgpu::PrimitiveTopology::LineList),
            line_strip: pipeline(wgpu::PrimitiveTopology::LineStrip),
        }
    }

    const fn for_topology(&self, topology: Topology) -> &wgpu::RenderPipeline {
        match topology {
            Topology::Triangles => &self.triangles,
            Topology::Lines => &self.lines,
            Topology::LineStrip => &self.line_strip,
        }
    }
}

/// The multisampled colour and depth a picture of one size is drawn into
/// before it resolves into the caller's texture.
struct Attachments {
    size: (u32, u32),
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Attachments {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let attachment = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: SAMPLE_COUNT,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        Self {
            size,
            color: attachment("bench colour", format),
            depth: attachment("bench depth", DEPTH_FORMAT),
        }
    }
}

/// Bench ochre and brick red, matching the studio's caution and danger tokens.
const CAUTION: [f32; 3] = [0.49, 0.42, 0.25];
const DANGER: [f32; 3] = [0.75, 0.26, 0.24];

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

impl PrimitiveMeshes {
    fn new(device: &wgpu::Device) -> Self {
        let upload = |vertices: MeshVertices| {
            GpuMesh::upload(device, &vertices.values).expect("primitives have faces")
        };
        Self {
            cube: upload(MeshVertices::cube()),
            cylinder: upload(MeshVertices::cylinder(28)),
            sphere: upload(MeshVertices::sphere(12, 18)),
        }
    }
}

impl BenchMeshes {
    fn new(device: &wgpu::Device) -> Self {
        let upload = |vertices: MeshVertices| {
            GpuMesh::upload(device, &vertices.values).expect("the bench has lines")
        };
        Self {
            major_grid: upload(MeshVertices::bench_grid()),
            x_axis: upload(MeshVertices::axis(GridAxis::X)),
            y_axis: upload(MeshVertices::axis(GridAxis::Y)),
        }
    }
}

/// A vertex buffer of interleaved positions and normals.
struct GpuMesh {
    buffer: wgpu::Buffer,
    vertices: u32,
}

impl GpuMesh {
    /// Upload interleaved position and normal triples, or nothing for a mesh
    /// with no vertices to draw.
    fn upload(device: &wgpu::Device, values: &[f32]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bench mesh"),
            contents: bytemuck::cast_slice(values),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Some(Self {
            buffer,
            vertices: (values.len() / 6) as u32,
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
