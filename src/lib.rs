use imgui::{
    Context, DrawCmd::Elements, DrawData, DrawIdx, DrawList, DrawVert, TextureId, Textures,
};
use std::mem::size_of;
use wgpu::util::DeviceExt;
use wgpu::*;

pub type RendererResult<T> = Result<T, RendererError>;

#[derive(Clone, Debug)]
pub enum RendererError {
    BadTexture(TextureId),
}

#[allow(dead_code)]
enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

#[cfg(feature = "glsl-to-spirv")]
struct Shaders;

#[cfg(feature = "glsl-to-spirv")]
impl Shaders {
    fn compile_glsl(code: &str, stage: ShaderStage) -> Vec<u32> {
        let ty = match stage {
            ShaderStage::Vertex => glsl_to_spirv::ShaderType::Vertex,
            ShaderStage::Fragment => glsl_to_spirv::ShaderType::Fragment,
            ShaderStage::Compute => glsl_to_spirv::ShaderType::Compute,
        };

        read_spirv(glsl_to_spirv::compile(&code, ty).unwrap()).unwrap()
    }

    fn get_program_code() -> (&'static str, &'static str) {
        (include_str!("imgui.vert"), include_str!("imgui.frag"))
    }
}

/// A container for a bindable texture to be used internally.
pub struct Texture {
    bind_group: BindGroup,
}

impl Texture {
    /// Creates a new imgui texture from a wgpu texture.
    pub fn new(texture: wgpu::Texture, layout: &BindGroupLayout, device: &Device) -> Self {
        // Extract the texture view.
        let view = texture.create_view(&Default::default());

        // Create the texture sampler.
        let sampler = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Linear,
            lod_min_clamp: -100.0,
            lod_max_clamp: 100.0,
            anisotropy_clamp: None,
            compare: None,
            label: Some("imgui sampler"),
        });

        // Create the texture bind group from the layout.
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
        });

        Texture { bind_group }
    }
}

#[allow(dead_code)]
pub struct Renderer {
    pipeline: RenderPipeline,
    uniform_buffer: Buffer,
    uniform_bind_group: BindGroup,
    textures: Textures<Texture>,
    texture_layout: BindGroupLayout,
    clear_color: Option<Color>,
    index_buffers: Vec<Buffer>,
    vertex_buffers: Vec<Buffer>,
}

impl Renderer {
    /// Create a new imgui wgpu renderer with newly compiled shaders.
    #[cfg(feature = "glsl-to-spirv")]
    pub fn new_glsl(
        imgui: &mut Context,
        device: &Device,
        queue: &Queue,
        format: TextureFormat,
        clear_color: Option<Color>,
        sample_count: u32,
    ) -> Renderer {
        let (vs_code, fs_code) = Shaders::get_program_code();
        let vs_raw = Shaders::compile_glsl(vs_code, ShaderStage::Vertex);
        let fs_raw = Shaders::compile_glsl(fs_code, ShaderStage::Fragment);
        Self::new_impl(imgui, device, queue, format, clear_color, vs_raw, fs_raw, sample_count)
    }

    /// Create a new imgui wgpu renderer, using prebuilt spirv shaders.
    pub fn new(
        imgui: &mut Context,
        device: &Device,
        queue: &Queue,
        format: TextureFormat,
        depth_format: Option<TextureFormat>,
        clear_color: Option<Color>,
        sample_count: u32,
    ) -> Renderer {
        let vs_raw = include_spirv!("imgui.vert.spv");
        let fs_raw = include_spirv!("imgui.frag.spv");

        Self::new_impl(
            imgui,
            device,
            queue,
            format,
            depth_format,
            clear_color,
            vs_raw,
            fs_raw,
            sample_count,
        )
    }

    #[deprecated(note = "Renderer::new now uses static shaders by default")]
    pub fn new_static(
        imgui: &mut Context,
        device: &Device,
        queue: &Queue,
        format: TextureFormat,
        depth_format: Option<TextureFormat>,
        clear_color: Option<Color>,
        sample_count: u32,
    ) -> Renderer {
        Renderer::new(imgui, device, queue, format, depth_format, clear_color, sample_count)
    }

    /// Create an entirely new imgui wgpu renderer.
    fn new_impl(
        imgui: &mut Context,
        device: &Device,
        queue: &Queue,
        format: TextureFormat,
        depth_format: Option<TextureFormat>,
        clear_color: Option<Color>,
        vs_raw: ShaderModuleSource,
        fs_raw: ShaderModuleSource,
        sample_count: u32,
    ) -> Renderer {
        // Load shaders.
        let vs_module = device.create_shader_module(vs_raw);
        let fs_module = device.create_shader_module(fs_raw);

        // Create the uniform matrix buffer.
        let size = 64;
        let uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: None,
            size,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            mapped_at_creation: false,
        });

        // Create the uniform matrix buffer bind group layout.
        let uniform_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStage::VERTEX,
                ty: BindingType::UniformBuffer {
                    dynamic: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Create the uniform matrix buffer bind group.
        let uniform_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &uniform_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(uniform_buffer.slice(..)),
            }],
        });

        // Create the texture layout for further usage.
        let texture_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: BindingType::SampledTexture {
                        multisampled: false,
                        component_type: TextureComponentType::Float,
                        dimension: TextureViewDimension::D2,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: BindingType::Sampler { comparison: false },
                    count: None,
                },
            ],
        });

        // Create the render pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            bind_group_layouts: &[&uniform_layout, &texture_layout],
            push_constant_ranges: &[],
            label: None,
        });

        // Create the render pipeline.
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex_stage: ProgrammableStageDescriptor {
                module: &vs_module,
                entry_point: "main",
            },
            fragment_stage: Some(ProgrammableStageDescriptor {
                module: &fs_module,
                entry_point: "main",
            }),
            rasterization_state: Some(RasterizationStateDescriptor {
                front_face: FrontFace::Cw,
                cull_mode: CullMode::None,
                depth_bias: 0,
                depth_bias_slope_scale: 0.0,
                depth_bias_clamp: 0.0,
                clamp_depth: false,
            }),
            primitive_topology: PrimitiveTopology::TriangleList,
            color_states: &[ColorStateDescriptor {
                format,
                color_blend: BlendDescriptor {
                    src_factor: BlendFactor::SrcAlpha,
                    dst_factor: BlendFactor::OneMinusSrcAlpha,
                    operation: BlendOperation::Add,
                },
                alpha_blend: BlendDescriptor {
                    src_factor: BlendFactor::OneMinusDstAlpha,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                },
                write_mask: ColorWrite::ALL,
            }],
            depth_stencil_state: depth_format.map(|format| {
                wgpu::DepthStencilStateDescriptor {
                    format,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Always,
                    stencil: Default::default(),
                }
            }),
            vertex_state: VertexStateDescriptor {
                index_format: IndexFormat::Uint16,
                vertex_buffers: &[VertexBufferDescriptor {
                    stride: size_of::<DrawVert>() as BufferAddress,
                    step_mode: InputStepMode::Vertex,
                    attributes: &[
                        VertexAttributeDescriptor {
                            format: VertexFormat::Float2,
                            shader_location: 0,
                            offset: 0,
                        },
                        VertexAttributeDescriptor {
                            format: VertexFormat::Float2,
                            shader_location: 1,
                            offset: 8,
                        },
                        VertexAttributeDescriptor {
                            format: VertexFormat::Uint,
                            shader_location: 2,
                            offset: 16,
                        },
                    ],
                }],
            },
            sample_count,
            sample_mask: !0,
            alpha_to_coverage_enabled: false,
        });

        let mut renderer = Renderer {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            textures: Textures::new(),
            texture_layout,
            clear_color,
            vertex_buffers: vec![],
            index_buffers: vec![],
        };

        // Immediately load the fon texture to the GPU.
        renderer.reload_font_texture(imgui, device, queue);

        renderer
    }

    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        draw_data: &DrawData,
    ) -> RendererResult<()> {
        let fb_width = draw_data.display_size[0] * draw_data.framebuffer_scale[0];
        let fb_height = draw_data.display_size[1] * draw_data.framebuffer_scale[1];

        // If the render area is <= 0, exit here and now.
        if !(fb_width > 0.0 && fb_height > 0.0) {
            return Ok(());
        }

        let width = draw_data.display_size[0];
        let height = draw_data.display_size[1];

        // Create and update the transform matrix for the current frame.
        // This is required to adapt to vulkan coordinates.
        // let matrix = [
        //     [2.0 / width, 0.0, 0.0, 0.0],
        //     [0.0, 2.0 / height as f32, 0.0, 0.0],
        //     [0.0, 0.0, -1.0, 0.0],
        //     [-1.0, -1.0, 0.0, 1.0],
        // ];
        let matrix = [
            [2.0 / width, 0.0, 0.0, 0.0],
            [0.0, 2.0 / -height as f32, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0, 1.0],
        ];
        self.update_uniform_buffer(queue, &matrix);

        self.vertex_buffers.clear();
        self.index_buffers.clear();

        for draw_list in draw_data.draw_lists() {
            self.vertex_buffers
                .push(self.upload_vertex_buffer(device, draw_list.vtx_buffer()));
            self.index_buffers
                .push(self.upload_index_buffer(device, draw_list.idx_buffer()));
        }

        Ok(())
    }

    /// Render the current imgui frame.
    pub fn render<'r>(
        &'r mut self,
        draw_data: &DrawData,
        device: &Device,
        queue: &Queue,
        encoder: &'r mut CommandEncoder,
        view: &TextureView,
    ) -> RendererResult<()> {
        let fb_width = draw_data.display_size[0] * draw_data.framebuffer_scale[0];
        let fb_height = draw_data.display_size[1] * draw_data.framebuffer_scale[1];

        // If the render area is <= 0, exit here and now.
        if !(fb_width > 0.0 && fb_height > 0.0) {
            return Ok(());
        }

        self.prepare(device, queue, draw_data)?;

        // Start a new renderpass and prepare it properly.
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            color_attachments: &[RenderPassColorAttachmentDescriptor {
                attachment: &view,
                resolve_target: None,
                ops: Operations {
                    load: match self.clear_color {
                        Some(c) => LoadOp::Clear(c),
                        None => LoadOp::Load,
                    },
                    store: true,
                },
            }],
            depth_stencil_attachment: None,
        });
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.uniform_bind_group, &[]);

        // Execute all the imgui render work.
        for (draw_list_buffers_index, draw_list) in draw_data.draw_lists().enumerate() {
            self.render_draw_list(
                &mut rpass,
                &draw_list,
                draw_data.display_pos,
                draw_data.framebuffer_scale,
                draw_list_buffers_index,
            )?;
        }

        Ok(())
    }

    /// Render the current imgui frame.
    pub fn render_to_pass<'r>(
        &'r mut self,
        draw_data: &DrawData,
        rpass: &mut RenderPass<'r>,
    ) -> RendererResult<()> {
        let fb_width = draw_data.display_size[0] * draw_data.framebuffer_scale[0];
        let fb_height = draw_data.display_size[1] * draw_data.framebuffer_scale[1];

        // If the render area is <= 0, exit here and now.
        if !(fb_width > 0.0 && fb_height > 0.0) {
            return Ok(());
        }

        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.uniform_bind_group, &[]);

        // Execute all the imgui render work.
        for (draw_list_buffers_index, draw_list) in draw_data.draw_lists().enumerate() {
            self.render_draw_list(
                &mut *rpass,
                &draw_list,
                draw_data.display_pos,
                draw_data.framebuffer_scale,
                draw_list_buffers_index,
            )?;
        }

        Ok(())
    }

    /// Render a given `DrawList` from imgui onto a wgpu frame.
    fn render_draw_list<'render>(
        &'render self,
        rpass: &mut RenderPass<'render>,
        draw_list: &DrawList,
        clip_off: [f32; 2],
        clip_scale: [f32; 2],
        draw_list_buffers_index: usize,
    ) -> RendererResult<()> {
        let mut start = 0;

        let index_buffer = &self.index_buffers[draw_list_buffers_index];
        let vertex_buffer = &self.vertex_buffers[draw_list_buffers_index];

        // Make sure the current buffers are attached to the render pass.
        rpass.set_index_buffer(index_buffer.slice(..));
        rpass.set_vertex_buffer(0, vertex_buffer.slice(..));

        for cmd in draw_list.commands() {
            match cmd {
                Elements { count, cmd_params } => {
                    let clip_rect = [
                        (cmd_params.clip_rect[0] - clip_off[0]) * clip_scale[0],
                        (cmd_params.clip_rect[1] - clip_off[1]) * clip_scale[1],
                        (cmd_params.clip_rect[2] - clip_off[0]) * clip_scale[0],
                        (cmd_params.clip_rect[3] - clip_off[1]) * clip_scale[1],
                    ];

                    // Set the current texture bind group on the renderpass.
                    let texture_id = cmd_params.texture_id;
                    let tex = self
                        .textures
                        .get(texture_id)
                        .ok_or_else(|| RendererError::BadTexture(texture_id))?;
                    rpass.set_bind_group(1, &tex.bind_group, &[]);

                    // Set scissors on the renderpass.
                    let scissors = (
                        clip_rect[0].max(0.0).floor() as u32,
                        clip_rect[1].max(0.0).floor() as u32,
                        (clip_rect[2] - clip_rect[0]).abs().ceil() as u32,
                        (clip_rect[3] - clip_rect[1]).abs().ceil() as u32,
                    );
                    rpass.set_scissor_rect(scissors.0, scissors.1, scissors.2, scissors.3);

                    // Draw the current batch of vertices with the renderpass.
                    let end = start + count as u32;
                    rpass.draw_indexed(start..end, 0, 0..1);
                    start = end;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Updates the current uniform buffer containing the transform matrix.
    fn update_uniform_buffer(&mut self, queue: &Queue, matrix: &[[f32; 4]; 4]) {
        let data = as_byte_slice(matrix);
        queue.write_buffer(&self.uniform_buffer, 0, data);
    }

    /// Upload the vertex buffer to the gPU.
    fn upload_vertex_buffer(&self, device: &Device, vertices: &[DrawVert]) -> Buffer {
        let data = as_byte_slice(&vertices);
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            contents: data,
            usage: BufferUsage::VERTEX,
            label: None,
        })
    }

    /// Upload the index buffer to the GPU.
    fn upload_index_buffer(&self, device: &Device, indices: &[DrawIdx]) -> Buffer {
        let data = as_byte_slice(&indices);
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            contents: data,
            usage: BufferUsage::INDEX,
            label: None,
        })
    }

    /// Updates the texture on the GPU corresponding to the current imgui font atlas.
    ///
    /// This has to be called after loading a font.
    pub fn reload_font_texture(&mut self, imgui: &mut Context, device: &Device, queue: &Queue) {
        let mut atlas = imgui.fonts();
        let handle = atlas.build_rgba32_texture();
        let font_texture_id =
            self.upload_texture(device, queue, &handle.data, handle.width, handle.height);

        atlas.tex_id = font_texture_id;
    }

    /// Creates and uploads a new wgpu texture made from the imgui font atlas.
    pub fn upload_texture(
        &mut self,
        device: &Device,
        queue: &Queue,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> TextureId {
        // Create the wgpu texture.
        let texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: Extent3d {
                width,
                height,
                depth: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        });

        // Upload the actual data to a wgpu buffer.
        let bytes = data.len();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            contents: data,
            usage: BufferUsage::COPY_SRC,
            label: None,
        });

        // Make sure we have an active encoder.
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: None });

        // Schedule a copy from the buffer to the texture.
        encoder.copy_buffer_to_texture(
            BufferCopyView {
                buffer: &buffer,
                layout: TextureDataLayout {
                    offset: 0,
                    bytes_per_row: bytes as u32 / height,
                    rows_per_image: height,
                },
            },
            TextureCopyView {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d { x: 0, y: 0, z: 0 },
            },
            Extent3d {
                width,
                height,
                depth: 1,
            },
        );

        // Resolve the actual copy process.
        queue.submit(std::iter::once(encoder.finish()));

        let texture = Texture::new(texture, &self.texture_layout, device);
        self.textures.insert(texture)
    }
}

fn as_byte_slice<T>(slice: &[T]) -> &[u8] {
    let len = slice.len() * std::mem::size_of::<T>();
    let ptr = slice.as_ptr() as *const u8;
    unsafe { std::slice::from_raw_parts(ptr, len) }
}
