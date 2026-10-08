//! The room drawn by the GPU, off screen, and read back as pixels.
//!
//! GPUI does not hand out the device it draws its own interface with, so
//! the Den opens one of its own ([`Gpu::new`]), draws the room into a
//! texture and reads the pixels back ([`Gpu::draw`]): the picture then
//! reaches the window as an image, the way the pixel-art one does. Two
//! passes: the sun's view of the room for the shadows, then the room
//! itself, its faces flat-shaded and its hairlines over them, four samples
//! a pixel.
//!
//! On Linux the device is an OpenGL one unless `WGPU_BACKEND` says
//! otherwise: reading a picture back from a Vulkan one was some fifty times
//! slower on the hardware this was measured on.
//!
//! Nothing here panics on a GPU that goes wrong. Every way a draw can fail,
//! no adapter, a lost device, an error the driver reports, comes back as an
//! `Err`, and the caller falls back to the pixel art.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::camera::{Camera, SUN};
use super::mesh::{unit, Mesh, Rgb, FACE_FLOATS, LINE_FLOATS};

const UNIFORMS: &str = r"
struct Uniforms {
    camera: mat4x4<f32>,
    sun_view: mat4x4<f32>,
    sun: vec4<f32>,
    light: vec4<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;
";

const FACES: &str = r"
@group(0) @binding(1) var shadows: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>,
          @location(2) color: vec3<f32>) -> Out {
    var out: Out;
    out.position = uniforms.camera * vec4<f32>(position, 1.0);
    out.color = color;
    out.normal = normal;
    out.world = position;
    return out;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    var light = 1.0;
    if (dot(in.normal, in.normal) > 0.25) {
        let normal = normalize(in.normal);
        let facing = max(dot(normal, uniforms.sun.xyz), 0.0);
        let seen = uniforms.sun_view * vec4<f32>(in.world + normal * 0.03, 1.0);
        let at = vec2<f32>(seen.x * 0.5 + 0.5, 0.5 - seen.y * 0.5);
        var open = 0.0;
        for (var x = -1; x <= 1; x++) {
            for (var y = -1; y <= 1; y++) {
                let near = at + vec2<f32>(f32(x), f32(y)) / 2048.0;
                open += textureSampleCompareLevel(shadows, shadow_sampler, near, seen.z - 0.0012);
            }
        }
        open = open / 9.0;
        let sunlit = mix(1.0 - uniforms.light.z, 1.0, open);
        light = uniforms.light.x + uniforms.light.y * facing * sunlit;
        // A face the sun does not reach at all is in the same shade.
        light = light - uniforms.light.y * 0.12 * (1.0 - open) * (1.0 - facing);
    }
    let color = in.color * light;
    // Blue first: the order GPUI wants its pixels in.
    return vec4<f32>(color.b, color.g, color.r, 1.0);
}
";

const SHADOW: &str = r"
@vertex
fn vertex(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return uniforms.sun_view * vec4<f32>(position, 1.0);
}
";

const LINES: &str = r"
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>) -> Out {
    var out: Out;
    out.position = uniforms.camera * vec4<f32>(position, 1.0);
    // A hairline lies on its face: bring it a hair toward the eye.
    out.position.z = out.position.z - 0.0004;
    out.color = color;
    return out;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.b, in.color.g, in.color.r, 1.0);
}
";

const SAMPLES: u32 = 4;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_SIDE: u32 = 2048;
/// The largest side of a picture: a view larger than this is not drawn in
/// 2.5D.
pub const MAX_SIDE: u32 = 8192;

struct Target {
    size: (u32, u32),
    many: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolved: wgpu::Texture,
    resolved_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    stride: u32,
}

/// A buffer of corners that grows when a mesh needs more.
struct Corners {
    buffer: wgpu::Buffer,
    room: u64,
}

/// The device, and what it draws the room with.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    faces: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    faces_bind: wgpu::BindGroup,
    lines_bind: wgpu::BindGroup,
    shadow_bind: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    target: Option<Target>,
    face_corners: Option<Corners>,
    line_corners: Option<Corners>,
    /// Set by the device when it reports an error or is lost.
    broken: Arc<AtomicBool>,
    /// What the adapter calls itself.
    pub adapter: String,
}

fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias: Default::default(),
    }
}

fn bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_ne_bytes());
    }
    out
}

impl Gpu {
    /// Opens a device. `Err` says why there is none.
    pub fn new() -> Result<Gpu, String> {
        let mut wanted = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        if cfg!(target_os = "linux") && std::env::var_os("WGPU_BACKEND").is_none() {
            wanted.backends = wgpu::Backends::GL;
        }
        let instance = wgpu::Instance::new(wanted);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|error| format!("no adapter: {error}"))?;
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|error| format!("no device: {error}"))?;
        let broken = Arc::new(AtomicBool::new(false));
        let flag = broken.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            if !flag.swap(true, Ordering::Relaxed) {
                eprintln!("leon-den: the GPU reported an error: {error}");
            }
        }));
        let flag = broken.clone();
        device.set_device_lost_callback(move |_, _| flag.store(true, Ordering::Relaxed));

        let module = |label, body: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(format!("{UNIFORMS}{body}").into()),
            })
        };
        let (faces_module, lines_module, shadow_module) = (
            module("faces", FACES),
            module("lines", LINES),
            module("shadow", SHADOW),
        );
        let face_attributes =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
        let line_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let multisample = wgpu::MultisampleState {
            count: SAMPLES,
            ..Default::default()
        };
        let face_stride = (FACE_FLOATS * 4) as u64;
        let faces = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("faces"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &faces_module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: face_stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &face_attributes,
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &faces_module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let lines = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lines"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &lines_module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: (LINE_FLOATS * 4) as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &line_attributes,
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(depth_state(false, wgpu::CompareFunction::LessEqual)),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &lines_module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shadow_module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: face_stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &face_attributes[..1],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: 160,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadows"),
            size: wgpu::Extent3d {
                width: SHADOW_SIDE,
                height: SHADOW_SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_map.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadows"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let only_uniforms = |pipeline: &wgpu::RenderPipeline| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                }],
            })
        };
        let (lines_bind, shadow_bind) = (only_uniforms(&lines), only_uniforms(&shadow));
        let faces_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("faces"),
            layout: &faces.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let _ = device.poll(wgpu::PollType::Poll);
        if broken.load(Ordering::Relaxed) {
            return Err("the device refused the room's shaders".to_owned());
        }
        Ok(Gpu {
            device,
            queue,
            faces,
            lines,
            shadow,
            uniforms,
            faces_bind,
            lines_bind,
            shadow_bind,
            shadow_view,
            target: None,
            face_corners: None,
            line_corners: None,
            broken,
            adapter: format!("{} ({:?})", info.name, info.backend),
        })
    }

    fn target(&mut self, size: (u32, u32)) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.size == size)
        {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let texture = |format, samples, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let many = texture(FORMAT, SAMPLES, attachment);
        let depth = texture(DEPTH, SAMPLES, attachment);
        let resolved = texture(FORMAT, 1, attachment | wgpu::TextureUsages::COPY_SRC);
        let stride = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(stride) * u64::from(size.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.target = Some(Target {
            size,
            many: many.create_view(&Default::default()),
            depth: depth.create_view(&Default::default()),
            resolved_view: resolved.create_view(&Default::default()),
            resolved,
            readback,
            stride,
        });
    }

    /// Puts corners in a buffer that is kept from one draw to the next and
    /// made larger only when a mesh needs more room.
    fn fill(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        kept: &mut Option<Corners>,
        values: &[f32],
    ) {
        let data = bytes(values);
        let needed = (data.len() as u64).max(64);
        if kept.as_ref().is_none_or(|corners| corners.room < needed) {
            let room = needed.next_power_of_two();
            *kept = Some(Corners {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("corners"),
                    size: room,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                room,
            });
        }
        if let Some(corners) = kept {
            queue.write_buffer(&corners.buffer, 0, &data);
        }
    }

    /// The picture of a room: `width * height * 4` bytes, blue first, as
    /// GPUI wants them. `light` is the light on a face in shade, the sun's
    /// part and how dark a shadow is.
    pub fn draw(
        &mut self,
        mesh: &Mesh,
        camera: &Camera,
        background: Rgb,
        light: [f32; 3],
        size: (u32, u32),
    ) -> Result<Vec<u8>, String> {
        if size.0 == 0 || size.1 == 0 || size.0 > MAX_SIDE || size.1 > MAX_SIDE {
            return Err(format!(
                "a picture of {} by {} cannot be drawn",
                size.0, size.1
            ));
        }
        if self.broken.load(Ordering::Relaxed) {
            return Err("the device is lost".to_owned());
        }
        self.target(size);
        let sun = unit(SUN);
        let mut uniforms = Vec::with_capacity(40);
        uniforms.extend(camera.matrix());
        uniforms.extend(camera.sun_matrix());
        uniforms.extend([sun[0], sun[1], sun[2], 0.]);
        uniforms.extend([light[0], light[1], light[2], 0.]);
        self.queue
            .write_buffer(&self.uniforms, 0, &bytes(&uniforms));
        Self::fill(
            &self.device,
            &self.queue,
            &mut self.face_corners,
            &mesh.faces,
        );
        Self::fill(
            &self.device,
            &self.queue,
            &mut self.line_corners,
            &mesh.lines,
        );
        let (Some(target), Some(faces), Some(lines)) =
            (&self.target, &self.face_corners, &self.line_corners)
        else {
            return Err("nothing to draw into".to_owned());
        };
        let face_count = (mesh.faces.len() / FACE_FLOATS) as u32;
        let line_count = (mesh.lines.len() / LINE_FLOATS) as u32;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow);
            pass.set_bind_group(0, &self.shadow_bind, &[]);
            pass.set_vertex_buffer(0, faces.buffer.slice(..));
            pass.draw(0..face_count, 0..1);
        }
        {
            let [r, g, b] = background.map(f64::from);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("room"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.many,
                    depth_slice: None,
                    resolve_target: Some(&target.resolved_view),
                    ops: wgpu::Operations {
                        // Blue first, as the fragments are.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: b,
                            g,
                            b: r,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.faces);
            pass.set_bind_group(0, &self.faces_bind, &[]);
            pass.set_vertex_buffer(0, faces.buffer.slice(..));
            pass.draw(0..face_count, 0..1);
            if line_count > 0 {
                pass.set_pipeline(&self.lines);
                pass.set_bind_group(0, &self.lines_bind, &[]);
                pass.set_vertex_buffer(0, lines.buffer.slice(..));
                pass.draw(0..line_count, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            target.resolved.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.stride),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = target.readback.slice(..);
        let mapped = Arc::new(AtomicBool::new(false));
        let done = mapped.clone();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            done.store(result.is_ok(), Ordering::Relaxed);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .map_err(|error| format!("the GPU did not answer: {error}"))?;
        if !mapped.load(Ordering::Relaxed) || self.broken.load(Ordering::Relaxed) {
            return Err("the picture could not be read back".to_owned());
        }
        let row = size.0 as usize * 4;
        let mut out = Vec::with_capacity(row * size.1 as usize);
        {
            let view = slice.get_mapped_range();
            for line in view.chunks_exact(target.stride as usize) {
                out.extend_from_slice(&line[..row]);
            }
        }
        target.readback.unmap();
        Ok(out)
    }
}
