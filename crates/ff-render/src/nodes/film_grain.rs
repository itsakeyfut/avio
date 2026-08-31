use super::RenderNodeCpu;

// Pipeline cache

#[cfg(feature = "wgpu")]
struct FilmGrainPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
}

// FilmGrainNode

/// Film grain: per-pixel pseudo-random noise added in YCbCr (BT.709) so luma and
/// chroma grain can be dialled independently.
///
/// The grain pattern is seeded from the pixel position and `frame_index`, so it
/// is deterministic within a frame yet changes every frame (no temporal
/// sticking). The GPU and CPU paths share the same Wang hash.
pub struct FilmGrainNode {
    /// Luma (brightness) grain amplitude, e.g. 0.05.
    pub luma_strength: f32,
    /// Chroma (colour) grain amplitude, e.g. 0.02.
    pub chroma_strength: f32,
    /// Frame index; changing it changes the grain pattern.
    pub frame_index: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<FilmGrainPipeline>,
}

impl FilmGrainNode {
    /// Creates a film-grain node with the given strengths and frame index.
    #[must_use]
    pub fn new(luma_strength: f32, chroma_strength: f32, frame_index: u32) -> Self {
        Self {
            luma_strength,
            chroma_strength,
            frame_index,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for FilmGrainNode {
    /// Identity node (no grain).
    fn default() -> Self {
        Self::new(0.0, 0.0, 0)
    }
}

// Shared PRNG: kept byte-identical to the WGSL path.

/// Wang hash: a fast integer hash used as a per-pixel PRNG. Matches the WGSL
/// `wang_hash` (wrapping u32 arithmetic).
fn wang_hash(seed_in: u32) -> u32 {
    let mut seed = (seed_in ^ 0x3d) ^ (seed_in >> 16);
    seed = seed.wrapping_mul(9);
    seed ^= seed >> 4;
    seed = seed.wrapping_mul(0x27d4_eb2d);
    seed ^= seed >> 15;
    seed
}

#[allow(clippy::cast_precision_loss)]
fn rand01(seed: u32) -> f32 {
    wang_hash(seed) as f32 / 4_294_967_295.0
}

// CPU path

impl RenderNodeCpu for FilmGrainNode {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::many_single_char_names,
        clippy::similar_names
    )]
    fn process_cpu(&self, rgba: &mut [u8], w: u32, _h: u32) {
        if w == 0 {
            return;
        }
        for (i, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = i as u32 % w;
            let y = i as u32 / w;
            let base = x
                .wrapping_mul(1973)
                .wrapping_add(y.wrapping_mul(9277))
                .wrapping_add(self.frame_index.wrapping_mul(26699));

            let g_luma = (rand01(base) - 0.5) * self.luma_strength;
            let g_cb = (rand01(base.wrapping_add(1)) - 0.5) * self.chroma_strength;
            let g_cr = (rand01(base.wrapping_add(2)) - 0.5) * self.chroma_strength;

            let r = f32::from(pixel[0]) / 255.0;
            let g = f32::from(pixel[1]) / 255.0;
            let b = f32::from(pixel[2]) / 255.0;

            // RGB -> YCbCr (BT.709), Cb/Cr centred on 0.
            let mut ly = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let mut cb = (b - ly) / 1.8556;
            let mut cr = (r - ly) / 1.5748;

            ly += g_luma;
            cb += g_cb;
            cr += g_cr;

            // YCbCr -> RGB.
            let nr = ly + 1.5748 * cr;
            let ng = ly - 0.1873 * cb - 0.4681 * cr;
            let nb = ly + 1.8556 * cb;

            pixel[0] = (nr.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            pixel[1] = (ng.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            pixel[2] = (nb.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
impl FilmGrainNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &FilmGrainPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("FilmGrain shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/film_grain.wgsl").into()),
            });

            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("FilmGrain BGL"),
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
                label: Some("FilmGrain layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

            let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("FilmGrain pipeline"),
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

            // 2 x f32 + 2 x u32 = 16 bytes: matches FilmGrainUniforms in the shader.
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("FilmGrain uniforms"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            FilmGrainPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for FilmGrainNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("FilmGrainNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("FilmGrainNode::process called with no outputs");
            return;
        };

        let pd = self.get_or_create_pipeline(ctx);

        // 2 × f32 then 2 × u32, matching FilmGrainUniforms.
        let mut uniform_bytes = Vec::with_capacity(16);
        uniform_bytes.extend_from_slice(&self.luma_strength.to_le_bytes());
        uniform_bytes.extend_from_slice(&self.chroma_strength.to_le_bytes());
        uniform_bytes.extend_from_slice(&self.frame_index.to_le_bytes());
        uniform_bytes.extend_from_slice(&0u32.to_le_bytes());
        ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniform_bytes);

        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("FilmGrain BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pd.uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("FilmGrain pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("FilmGrain pass"),
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

    fn solid_frame(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            buf.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        buf
    }

    // Spread of the R channel across a frame; a uniform input has spread 0, so a
    // positive spread proves the grain is spatially varying (visibly noisy).
    fn r_spread(rgba: &[u8]) -> u8 {
        let mut min = 255u8;
        let mut max = 0u8;
        for px in rgba.as_chunks::<4>().0 {
            min = min.min(px[0]);
            max = max.max(px[0]);
        }
        max - min
    }

    #[test]
    fn film_grain_node_should_produce_noise() {
        let node = FilmGrainNode::new(0.05, 0.02, 0);
        let (w, h) = (16u32, 16u32);
        let mut rgba = solid_frame(w, h, [128, 128, 128]);
        node.process_cpu(&mut rgba, w, h);
        assert!(
            r_spread(&rgba) > 0,
            "grain must make a uniform frame spatially varying"
        );
    }

    #[test]
    fn film_grain_node_frames_should_differ() {
        let (w, h) = (16u32, 16u32);
        let mut frame0 = solid_frame(w, h, [128, 128, 128]);
        let mut frame1 = solid_frame(w, h, [128, 128, 128]);
        FilmGrainNode::new(0.05, 0.02, 0).process_cpu(&mut frame0, w, h);
        FilmGrainNode::new(0.05, 0.02, 1).process_cpu(&mut frame1, w, h);
        assert_ne!(
            frame0, frame1,
            "frame_index 0 and 1 must produce different grain (no temporal sticking)"
        );
    }

    #[test]
    fn film_grain_node_zero_strength_should_be_noop() {
        let node = FilmGrainNode::default();
        let (w, h) = (8u32, 8u32);
        let original = solid_frame(w, h, [200, 150, 100]);
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, w, h);
        // Zero strength adds zero grain; allow ±1 for the YCbCr round-trip.
        for (a, b) in rgba.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "zero-strength grain must preserve the frame (±1); got {a} vs {b}"
            );
        }
    }
}

#[cfg(all(test, feature = "wgpu"))]
mod gpu_tests {
    use super::*;
    use crate::context::RenderContext;
    use crate::graph::RenderGraph;
    use std::sync::Arc;

    fn ctx() -> Option<Arc<RenderContext>> {
        match futures::executor::block_on(RenderContext::init()) {
            Ok(ctx) => Some(Arc::new(ctx)),
            Err(_) => None,
        }
    }

    fn solid(w: u32, h: u32, v: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            buf.extend_from_slice(&[v, v, v, 255]);
        }
        buf
    }

    fn r_spread(rgba: &[u8]) -> u8 {
        let mut min = 255u8;
        let mut max = 0u8;
        for px in rgba.as_chunks::<4>().0 {
            min = min.min(px[0]);
            max = max.max(px[0]);
        }
        max - min
    }

    #[test]
    fn film_grain_gpu_should_produce_noise_and_vary_per_frame() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (16u32, 16u32);
        let frame = solid(w, h, 128);

        let g0 = RenderGraph::new(Arc::clone(&ctx))
            .push(FilmGrainNode::new(0.05, 0.02, 0))
            .process_gpu(&frame, w, h)
            .expect("gpu grain 0");
        let g1 = RenderGraph::new(Arc::clone(&ctx))
            .push(FilmGrainNode::new(0.05, 0.02, 1))
            .process_gpu(&frame, w, h)
            .expect("gpu grain 1");

        assert!(r_spread(&g0) > 0, "grain must make the frame noisy");
        assert_ne!(g0, g1, "frame 0 and 1 must differ (no temporal sticking)");
    }
}
