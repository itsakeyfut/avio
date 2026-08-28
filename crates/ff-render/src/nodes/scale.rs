use super::RenderNodeCpu;

/// Resampling algorithm for [`ScaleNode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleAlgorithm {
    /// Bilinear — fast, good quality for moderate scaling (default).
    #[default]
    Bilinear,
    /// Bicubic — medium quality.
    Bicubic,
    /// Lanczos — high quality, best for downscaling.
    Lanczos,
}

// Pipeline cache

#[cfg(feature = "wgpu")]
struct ScalePipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

// ScaleNode

/// Resample a frame to a target resolution.
///
/// In Phase 1 the GPU path renders into the output texture at whatever size
/// it was allocated — the graph allocates same-size textures, so `ScaleNode`
/// acts as a bilinear blit pass. Full dimension-changing support (variable
/// output texture size) is a Phase 2 addition.
///
/// The CPU path is a no-op; use an offline scaler (e.g. `image` crate) for
/// CPU-side resizing.
pub struct ScaleNode {
    /// Target width in pixels (used as metadata; output size depends on the graph).
    pub width: u32,
    /// Target height in pixels.
    pub height: u32,
    /// Sampling algorithm.
    pub algorithm: ScaleAlgorithm,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<ScalePipeline>,
}

impl ScaleNode {
    #[must_use]
    pub fn new(width: u32, height: u32, algorithm: ScaleAlgorithm) -> Self {
        Self {
            width,
            height,
            algorithm,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for ScaleNode {
    fn default() -> Self {
        Self::new(0, 0, ScaleAlgorithm::Bilinear)
    }
}

// CPU path — no-op

impl RenderNodeCpu for ScaleNode {
    fn process_cpu(&self, _rgba: &mut [u8], _w: u32, _h: u32) {
        // CPU-side resize is not implemented in Phase 1.
        // Use an offline scaler (e.g. `image::imageops::resize`) for CPU paths.
    }
}

// GPU path

#[cfg(feature = "wgpu")]
impl ScaleNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &ScalePipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Scale shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/scale.wgsl").into()),
            });

            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Scale BGL"),
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
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Scale layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

            // Use linear filtering for Bilinear (default) and Bicubic.
            // Lanczos would require a custom kernel — Phase 3 addition.
            let filter = match self.algorithm {
                ScaleAlgorithm::Bilinear | ScaleAlgorithm::Bicubic | ScaleAlgorithm::Lanczos => {
                    wgpu::FilterMode::Linear
                }
            };

            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Scale sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            });

            let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Scale pipeline"),
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

            ScalePipeline {
                render_pipeline,
                bind_group_layout: bgl,
                sampler,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for ScaleNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("ScaleNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("ScaleNode::process called with no outputs");
            return;
        };

        let pd = self.get_or_create_pipeline(ctx);

        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Scale BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pd.sampler),
                },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Scale pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Scale pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
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
    fn scale_node_cpu_path_is_passthrough() {
        let node = ScaleNode::new(100, 100, ScaleAlgorithm::Bilinear);
        let original = vec![10u8, 20, 30, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "ScaleNode CPU path must be a no-op");
    }

    #[test]
    fn scale_algorithm_default_should_be_bilinear() {
        assert_eq!(ScaleAlgorithm::default(), ScaleAlgorithm::Bilinear);
    }
}
