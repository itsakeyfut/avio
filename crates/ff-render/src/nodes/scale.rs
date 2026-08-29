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
/// The GPU path renders into a `width` x `height` output target (the executor
/// allocates a differently sized target from [`output_dimensions`]) using the
/// node's [`ScaleAlgorithm`] sampler, so it truly resizes rather than blitting
/// same-size. The CPU path is exposed as [`scale_cpu`](Self::scale_cpu), which
/// returns a new resized buffer (the in-place [`RenderNodeCpu::process_cpu`]
/// cannot change dimensions, so it stays a no-op).
///
/// A `width` or `height` of `0` means "keep the input size" (passthrough).
///
/// [`output_dimensions`]: crate::nodes::RenderNode::output_dimensions
pub struct ScaleNode {
    /// Target width in pixels (`0` = keep input width).
    pub width: u32,
    /// Target height in pixels (`0` = keep input height).
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

    /// Output size for the given input size: the configured `width` x `height`,
    /// or the input size when either is `0` (passthrough).
    #[must_use]
    pub fn target_size(&self, in_w: u32, in_h: u32) -> (u32, u32) {
        if self.width == 0 || self.height == 0 {
            (in_w, in_h)
        } else {
            (self.width, self.height)
        }
    }

    /// Resize an RGBA frame on the CPU, returning `(pixels, out_w, out_h)`.
    ///
    /// `src` is `in_w` x `in_h` RGBA (`in_w * in_h * 4` bytes). The output is
    /// [`target_size`](Self::target_size) at the node's [`ScaleAlgorithm`]
    /// (Bilinear -> triangle, Bicubic -> Catmull-Rom, Lanczos -> Lanczos3). A
    /// malformed `src` (wrong length) is returned unchanged.
    #[must_use]
    pub fn scale_cpu(&self, src: &[u8], in_w: u32, in_h: u32) -> (Vec<u8>, u32, u32) {
        // A zero-dimension source has nothing to resample; return it as-is.
        if in_w == 0 || in_h == 0 {
            return (src.to_vec(), in_w, in_h);
        }
        let (out_w, out_h) = self.target_size(in_w, in_h);
        let Some(img) = image::RgbaImage::from_raw(in_w, in_h, src.to_vec()) else {
            return (src.to_vec(), in_w, in_h);
        };
        let filter = match self.algorithm {
            ScaleAlgorithm::Bilinear => image::imageops::FilterType::Triangle,
            ScaleAlgorithm::Bicubic => image::imageops::FilterType::CatmullRom,
            ScaleAlgorithm::Lanczos => image::imageops::FilterType::Lanczos3,
        };
        let resized = image::imageops::resize(&img, out_w, out_h, filter);
        (resized.into_raw(), out_w, out_h)
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
        // Resizing changes dimensions, which the in-place `process_cpu(&mut [u8])`
        // signature cannot express (the buffer size is fixed). Use
        // [`ScaleNode::scale_cpu`] for a real CPU resize; here it is a no-op so
        // a ScaleNode in the CPU fallback chain passes the frame through.
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
    fn output_dimensions(&self, in_w: u32, in_h: u32) -> (u32, u32) {
        self.target_size(in_w, in_h)
    }

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

    #[test]
    fn scale_cpu_should_resize_not_passthrough() {
        // 4x2 frame: left half red, right half blue. Downscale to 2x2. A real
        // resize yields a 2x2 buffer with a red-dominant left column and a
        // blue-dominant right column; a no-op/passthrough would not change dims.
        let node = ScaleNode::new(2, 2, ScaleAlgorithm::Bilinear);
        let mut src = Vec::new();
        for _y in 0..2 {
            for x in 0..4 {
                if x < 2 {
                    src.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    src.extend_from_slice(&[0, 0, 255, 255]);
                }
            }
        }

        let (out, out_w, out_h) = node.scale_cpu(&src, 4, 2);
        assert_eq!(
            (out_w, out_h),
            (2, 2),
            "must resize to the requested dimensions"
        );
        assert_eq!(out.len(), 2 * 2 * 4, "output must be a 2x2 RGBA buffer");
        assert_ne!(
            out, src,
            "output must differ from the input (not a passthrough)"
        );
        // Pixel (col, row) at index (row * 2 + col) * 4.
        let left = &out[0..4]; // (0, 0)
        let right = &out[4..8]; // (1, 0)
        assert!(
            left[0] > left[2],
            "left column must stay red-dominant after resize; got {left:?}"
        );
        assert!(
            right[2] > right[0],
            "right column must stay blue-dominant after resize; got {right:?}"
        );
    }

    #[test]
    fn scale_cpu_zero_dimensions_should_passthrough() {
        // A ScaleNode with width/height 0 keeps the input size.
        let node = ScaleNode::new(0, 0, ScaleAlgorithm::Bilinear);
        let src = vec![10u8, 20, 30, 255, 40, 50, 60, 255]; // 2x1 RGBA
        let (out, out_w, out_h) = node.scale_cpu(&src, 2, 1);
        assert_eq!((out_w, out_h), (2, 1), "0 dimensions keep the input size");
        assert_eq!(out, src, "passthrough must return the input unchanged");
    }
}
