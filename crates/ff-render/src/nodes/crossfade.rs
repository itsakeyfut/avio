use super::RenderNodeCpu;

// Pipeline cache

#[cfg(feature = "wgpu")]
struct CrossfadePipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

// CrossfadeNode

/// Linear crossfade between two RGBA frames.
///
/// - `factor = 0.0` → output equals the input frame (`inputs[0]` / `process_cpu` argument).
/// - `factor = 1.0` → output equals `to_rgba`.
/// - `factor = 0.5` → arithmetic mean of both frames.
///
/// The "to" frame is stored in the node at construction time.
pub struct CrossfadeNode {
    /// Blend factor: 0.0 (from) → 1.0 (to).
    pub factor: f32,
    /// The "to" frame as RGBA bytes. Length must equal `to_width × to_height × 4`.
    pub to_rgba: Vec<u8>,
    /// Width of `to_rgba`.
    pub to_width: u32,
    /// Height of `to_rgba`.
    pub to_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<CrossfadePipeline>,
}

impl CrossfadeNode {
    /// Construct a crossfade node.
    ///
    /// `to_rgba` is the "to" frame (second input). It must be
    /// `to_width × to_height × 4` bytes of RGBA data.
    #[must_use]
    pub fn new(factor: f32, to_rgba: Vec<u8>, to_width: u32, to_height: u32) -> Self {
        Self {
            factor,
            to_rgba,
            to_width,
            to_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

// CPU path

impl RenderNodeCpu for CrossfadeNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.to_rgba.len() != rgba.len() {
            log::warn!(
                "CrossfadeNode::process_cpu skipped: size mismatch from={} to={}",
                rgba.len(),
                self.to_rgba.len()
            );
            return;
        }
        for (src, dst) in rgba.iter_mut().zip(self.to_rgba.iter()) {
            let blended = (1.0 - self.factor) * f32::from(*src) + self.factor * f32::from(*dst);
            *src = (blended + 0.5) as u8;
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
impl CrossfadeNode {
    #[allow(clippy::too_many_lines)]
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &CrossfadePipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Crossfade shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/crossfade.wgsl").into()),
            });

            // binding 0: tex_from, 1: tex_to, 2: sampler, 3: uniforms
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Crossfade BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Crossfade layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

            let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Crossfade pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Crossfade sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            // 4 × f32 = 16 bytes — matches CrossfadeUniforms in the shader.
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Crossfade uniforms"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            CrossfadePipeline {
                render_pipeline,
                bind_group_layout: bgl,
                sampler,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for CrossfadeNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(tex_from) = inputs.first() else {
            log::warn!("CrossfadeNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("CrossfadeNode::process called with no outputs");
            return;
        };

        let pd = self.get_or_create_pipeline(ctx);

        // Write factor uniform.
        let uniform_bytes: Vec<u8> = [self.factor, 0.0_f32, 0.0_f32, 0.0_f32]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniform_bytes);

        // Upload the "to" frame to a temporary GPU texture.
        let to_tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Crossfade to_tex"),
            size: wgpu::Extent3d {
                width: self.to_width,
                height: self.to_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &to_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.to_rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.to_width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: self.to_width,
                height: self.to_height,
                depth_or_array_layers: 1,
            },
        );

        let from_view = tex_from.create_view(&wgpu::TextureViewDescriptor::default());
        let to_view = to_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Crossfade BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&from_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&to_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&pd.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pd.uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Crossfade pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Crossfade pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &out_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pd.render_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        ctx.queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfade_node_factor_zero_should_return_from_frame() {
        let to = vec![200u8, 200, 200, 255];
        let node = CrossfadeNode::new(0.0, to, 1, 1);
        let mut rgba = vec![50u8, 60, 70, 255];
        let original = rgba.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba[0], original[0], "factor=0 must keep from-frame R");
    }

    #[test]
    fn crossfade_node_factor_one_should_return_to_frame() {
        let to = vec![200u8, 200, 200, 255];
        let node = CrossfadeNode::new(1.0, to.clone(), 1, 1);
        let mut rgba = vec![50u8, 50, 50, 255];
        node.process_cpu(&mut rgba, 1, 1);
        // Allow ±1 for float rounding.
        assert!(
            (rgba[0] as i32 - 200).abs() <= 1,
            "factor=1 must return to-frame R; got {}",
            rgba[0]
        );
    }

    #[test]
    fn crossfade_node_factor_half_should_produce_arithmetic_mean() {
        // from = 0, to = 200 → expected mean = 100
        let to = vec![200u8, 200, 200, 255];
        let node = CrossfadeNode::new(0.5, to, 1, 1);
        let mut rgba = vec![0u8, 0, 0, 255];
        node.process_cpu(&mut rgba, 1, 1);
        let diff = (rgba[0] as i32 - 100).abs();
        assert!(
            diff <= 1,
            "factor=0.5 must produce arithmetic mean ~100; got {}",
            rgba[0]
        );
    }

    #[test]
    fn crossfade_node_size_mismatch_should_leave_rgba_unchanged() {
        let to = vec![200u8; 8]; // 2 pixels
        let node = CrossfadeNode::new(0.5, to, 2, 1);
        let original = vec![50u8, 50, 50, 255]; // 1 pixel
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1); // size mismatch — must be a no-op
        assert_eq!(rgba, original, "size mismatch must leave rgba unchanged");
    }
}
