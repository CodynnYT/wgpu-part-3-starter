use std::sync::Arc;

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Quat, Vec3, Vec4};
use wgpu::util::DeviceExt;
use winit::{
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::OwnedDisplayHandle,
    keyboard::KeyCode,
    window::Window,
};

use crate::config::*;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const CAMERA_RADIUS: f32 = 9.0;
const GROUND_HALF_SIZE: f32 = 40.0;

struct Camera {
    eye: Vec3,
    target: Vec3,
    up: Vec3,
    aspect: f32,
    fovy: f32,
    znear: f32,
    zfar: f32,
}

impl Camera {
    // Builds the combined projection and view matrix.
    fn build_view_projection(&self) -> Mat4 {
        let view = view::look_at_mat4(self.eye, self.target, self.up);
        let proj = directx::perspective(self.fovy, self.aspect, self.znear, self.zfar);
        proj * view
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: Mat4,
    eye: Vec4,
    grid: [u32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshVertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
}

impl MeshVertex {
    // Describes the sphere and ground vertex layout to the pipeline.
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 6]>() as wgpu::BufferAddress,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

// Creates the depth texture view that matches the surface size.
fn create_depth_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth texture"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

// Generates a UV sphere with smooth normals.
fn build_sphere(radius: f32, color: [f32; 4]) -> (Vec<MeshVertex>, Vec<u32>) {
    let rings = 48u32;
    let segments = 64u32;

    let mut vertices = Vec::new();
    for i in 0..=rings {
        let theta = i as f32 * std::f32::consts::PI / rings as f32;
        for j in 0..=segments {
            let phi = j as f32 * std::f32::consts::TAU / segments as f32;
            let normal = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            vertices.push(MeshVertex {
                position: (normal * radius).to_array(),
                normal: normal.to_array(),
                color,
            });
        }
    }

    let mut indices = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let a = i * (segments + 1) + j;
            let b = a + segments + 1;
            indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }

    (vertices, indices)
}

// Generates a large flat ground quad at the given height.
fn build_ground(y: f32, color: [f32; 4]) -> (Vec<MeshVertex>, Vec<u32>) {
    let h = GROUND_HALF_SIZE;
    let corners = [[-h, y, -h], [h, y, -h], [h, y, h], [-h, y, h]];

    let vertices = corners
        .iter()
        .map(|&position| MeshVertex {
            position,
            normal: [0.0, 1.0, 0.0],
            color,
        })
        .collect();

    (vertices, vec![0, 1, 2, 0, 2, 3])
}

// Creates the flat starting grid of cloth particles, with the pin flag stored in w.
fn initial_positions() -> Vec<[f32; 4]> {
    let n = GRID_N;
    let tilt = Quat::from_rotation_z(CLOTH_TILT_DEG.to_radians());

    let mut positions = Vec::with_capacity((n * n) as usize);
    for y in 0..n {
        for x in 0..n {
            let u = x as f32 / (n - 1) as f32 - 0.5;
            let v = y as f32 / (n - 1) as f32 - 0.5;

            let mut p = tilt * Vec3::new(u * CLOTH_SIZE, 0.0, v * CLOTH_SIZE);
            p.y += CLOTH_HEIGHT;

            let corner_x = x == 0 || x == n - 1;
            let corner_y = y == 0 || y == n - 1;
            let pinned = match PIN_MODE {
                1 => corner_x && corner_y,
                2 => corner_x && y == 0,
                _ => false,
            };

            positions.push([p.x, p.y, p.z, if pinned { 0.0 } else { 1.0 }]);
        }
    }

    positions
}

// Builds triangle indices for the solid cloth and line indices for the net view.
fn build_cloth_indices(n: u32, stride: u32) -> (Vec<u32>, Vec<u32>) {
    let mut triangles = Vec::new();
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            let i = y * n + x;
            triangles.extend_from_slice(&[i, i + n, i + 1, i + 1, i + n, i + n + 1]);
        }
    }

    let on_net = |k: u32| k % stride == 0 || k == n - 1;

    let mut lines = Vec::new();
    for y in (0..n).filter(|&y| on_net(y)) {
        for x in 0..n - 1 {
            lines.extend_from_slice(&[y * n + x, y * n + x + 1]);
        }
    }
    for x in (0..n).filter(|&x| on_net(x)) {
        for y in 0..n - 1 {
            lines.extend_from_slice(&[y * n + x, (y + 1) * n + x]);
        }
    }

    (triangles, lines)
}

pub struct State {
    window: Arc<Window>,
    #[allow(dead_code)]
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    #[allow(dead_code)]
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    mesh_pipeline: wgpu::RenderPipeline,
    solid_pipeline: wgpu::RenderPipeline,
    net_pipeline: wgpu::RenderPipeline,
    mesh_vertex_buffer: wgpu::Buffer,
    mesh_index_buffer: wgpu::Buffer,
    mesh_index_count: u32,
    cloth_buffers: [wgpu::Buffer; 2],
    cloth_render_groups: [wgpu::BindGroup; 2],
    solid_index_buffer: wgpu::Buffer,
    solid_index_count: u32,
    net_index_buffer: wgpu::Buffer,
    net_index_count: u32,
    uniforms: Uniforms,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    camera: Camera,
    current: usize,
    net_view: bool,
    mouse_pressed: bool,
    yaw: f32,
    pitch: f32,
    last_frame: std::time::Instant,
    fps_timer: f32,
    fps_frames: u32,
}

impl State {
    // Sets up the GPU, the scene meshes, the cloth buffers and the render pipelines.
    pub async fn new(window: Arc<Window>, display: OwnedDisplayHandle) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(display),
        ));

        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create the surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .expect("no suitable GPU adapter found");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("failed to create the device");

        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(capabilities.formats[0]);

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("the surface is not supported by this adapter");
        config.format = format;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);

        let depth_view = create_depth_view(&device, &config);

        let (mut mesh_vertices, mut mesh_indices) =
            build_sphere(SPHERE_RADIUS, [0.70, 0.03, 0.03, 0.6]);
        let (ground_vertices, ground_indices) =
            build_ground(-SPHERE_RADIUS, [0.10, 0.17, 0.28, 0.0]);
        let ground_offset = mesh_vertices.len() as u32;
        mesh_vertices.extend(ground_vertices);
        mesh_indices.extend(ground_indices.iter().map(|i| i + ground_offset));

        let mesh_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vertex buffer"),
            contents: bytemuck::cast_slice(&mesh_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let mesh_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh index buffer"),
            contents: bytemuck::cast_slice(&mesh_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let positions = initial_positions();
        let make_cloth_buffer = |label: &str| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(&positions),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };
        let cloth_buffers = [
            make_cloth_buffer("cloth buffer a"),
            make_cloth_buffer("cloth buffer b"),
        ];

        let (solid_indices, net_indices) = build_cloth_indices(GRID_N, NET_STRIDE);

        let solid_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cloth solid indices"),
            contents: bytemuck::cast_slice(&solid_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let net_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cloth net indices"),
            contents: bytemuck::cast_slice(&net_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let camera = Camera {
            eye: Vec3::new(0.0, 3.0, CAMERA_RADIUS),
            target: Vec3::new(0.0, 0.3, 0.0),
            up: Vec3::Y,
            aspect: config.width as f32 / config.height as f32,
            fovy: 45.0_f32.to_radians(),
            znear: 0.1,
            zfar: 200.0,
        };

        let uniforms = Uniforms {
            view_proj: camera.build_view_projection(),
            eye: camera.eye.extend(1.0),
            grid: [GRID_N, 0, 0, 0],
        };

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniform buffer"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform bind group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let cloth_render_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cloth render bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let make_render_group = |buffer: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cloth render bind group"),
                layout: &cloth_render_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            })
        };
        let cloth_render_groups = [
            make_render_group(&cloth_buffers[0]),
            make_render_group(&cloth_buffers[1]),
        ];

        let shader = device.create_shader_module(wgpu::include_wgsl!("render.wgsl"));

        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });

        let cloth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cloth pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&cloth_render_layout)],
            immediate_size: 0,
        });

        let depth_state = || {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };

        let color_targets = [Some(wgpu::ColorTargetState {
            format: config.format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[Some(MeshVertex::desc())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &color_targets,
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth_state(),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let solid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cloth solid pipeline"),
            layout: Some(&cloth_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_cloth"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_cloth"),
                compilation_options: Default::default(),
                targets: &color_targets,
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth_state(),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let net_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cloth net pipeline"),
            layout: Some(&cloth_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_net"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_net"),
                compilation_options: Default::default(),
                targets: &color_targets,
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth_state(),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            window,
            instance,
            surface,
            adapter,
            device,
            queue,
            config,
            depth_view,
            mesh_pipeline,
            solid_pipeline,
            net_pipeline,
            mesh_vertex_buffer,
            mesh_index_buffer,
            mesh_index_count: mesh_indices.len() as u32,
            cloth_buffers,
            cloth_render_groups,
            solid_index_buffer,
            solid_index_count: solid_indices.len() as u32,
            net_index_buffer,
            net_index_count: net_indices.len() as u32,
            uniforms,
            uniform_buffer,
            uniform_bind_group,
            camera,
            current: 0,
            net_view: false,
            mouse_pressed: false,
            yaw: 0.0,
            pitch: 0.3,
            last_frame: std::time::Instant::now(),
            fps_timer: 0.0,
            fps_frames: 0,
        }
    }

    // Returns the window so the event loop can request redraws.
    pub fn window(&self) -> &Window {
        &self.window
    }

    // Reconfigures the surface, camera aspect and depth buffer after a resize.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
        self.camera.aspect = self.config.width as f32 / self.config.height as f32;
        self.depth_view = create_depth_view(&self.device, &self.config);
    }

    // Tracks whether the left mouse button is held for orbiting.
    pub fn input(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.mouse_pressed = *state == ElementState::Pressed;
                true
            }
            _ => false,
        }
    }

    // Handles the keyboard shortcuts: R resets the cloth and V switches between solid and net view.
    pub fn key_pressed(&mut self, code: KeyCode) {
        match code {
            KeyCode::KeyR => self.reset(),
            KeyCode::KeyV => self.net_view = !self.net_view,
            _ => {}
        }
    }

    // Puts the cloth back at its starting grid.
    fn reset(&mut self) {
        let positions = initial_positions();
        for buffer in &self.cloth_buffers {
            self.queue
                .write_buffer(buffer, 0, bytemuck::cast_slice(&positions));
        }
        self.current = 0;
    }

    // Rotates the camera around the scene while the mouse button is held.
    pub fn mouse_move(&mut self, dx: f64, dy: f64) {
        if !self.mouse_pressed {
            return;
        }

        let sensitivity = 0.005;
        self.yaw -= dx as f32 * sensitivity;
        self.pitch += dy as f32 * sensitivity;
        self.pitch = self.pitch.clamp(-0.2, 1.5);
    }

    // Updates the FPS counter and uploads the camera uniforms.
    pub fn update(&mut self) {
        let now = std::time::Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.fps_timer += dt;
        self.fps_frames += 1;
        if self.fps_timer >= 1.0 {
            println!("FPS: {}", self.fps_frames);
            self.fps_frames = 0;
            self.fps_timer = 0.0;
        }

        self.camera.eye = Vec3::new(
            CAMERA_RADIUS * self.yaw.sin() * self.pitch.cos(),
            CAMERA_RADIUS * self.pitch.sin(),
            CAMERA_RADIUS * self.yaw.cos() * self.pitch.cos(),
        );

        self.uniforms.view_proj = self.camera.build_view_projection();
        self.uniforms.eye = self.camera.eye.extend(1.0);
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[self.uniforms]),
        );
    }

    // Draws the sphere, the ground and the cloth for one frame.
    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.05,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_pipeline(&self.mesh_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.mesh_vertex_buffer.slice(..));
            pass.set_index_buffer(self.mesh_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.mesh_index_count, 0, 0..1);

            let (pipeline, index_buffer, index_count) = if self.net_view {
                (
                    &self.net_pipeline,
                    &self.net_index_buffer,
                    self.net_index_count,
                )
            } else {
                (
                    &self.solid_pipeline,
                    &self.solid_index_buffer,
                    self.solid_index_count,
                )
            };

            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_bind_group(1, &self.cloth_render_groups[self.current], &[]);
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);
    }
}
