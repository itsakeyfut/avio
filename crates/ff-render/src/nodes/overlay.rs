use super::RenderNodeCpu;

// ── Pipeline cache ────────────────────────────────────────────────────────────

#[cfg(feature = "wgpu")]
struct OverlayPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

// ── OverlayNode ───────────────────────────────────────────────────────────────

/// Porter-Duff "src over dst" alpha compositing.
///
/// The input frame (`inputs[0]` / `process_cpu` argument) is the base layer.
/// `overlay_rgba` is composited on top using its alpha channel.
///
/// The CPU path performs the same `src_over` formula as the shader:
/// ```text
/// out_rgb = overlay.rgb * overlay.a + base.rgb * (1 − overlay.a)
/// out_a   = overlay.a + base.a * (1 − overlay.a)
/// ```
pub struct OverlayNode {
    /// The overlay frame (top layer) as RGBA bytes.
    pub overlay_rgba: Vec<u8>,
    /// Width of `overlay_rgba`.
    pub overlay_width: u32,
    /// Height of `overlay_rgba`.
    pub overlay_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<OverlayPipeline>,
}

impl OverlayNode {
    #[must_use]
    pub fn new(overlay_rgba: Vec<u8>, overlay_width: u32, overlay_height: u32) -> Self {
        Self {
            overlay_rgba,
            overlay_width,
            overlay_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

// ── CPU path ──────────────────────────────────────────────────────────────────

impl RenderNodeCpu for OverlayNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.overlay_rgba.len() != rgba.len() {
            log::warn!(
                "OverlayNode::process_cpu skipped: size mismatch base={} overlay={}",
                rgba.len(),
                self.overlay_rgba.len()
            );
            return;
        }
        for (base, ov) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.overlay_rgba.as_chunks::<4>().0.iter())
        {
            let ov_a = f32::from(ov[3]) / 255.0;
            let base_a = f32::from(base[3]) / 255.0;
            let out_a = ov_a + base_a * (1.0 - ov_a);
            for ch in 0..3 {
                let ov_c = f32::from(ov[ch]) / 255.0;
                let base_c = f32::from(base[ch]) / 255.0;
                let out_c = ov_c * ov_a + base_c * (1.0 - ov_a);
                base[ch] = (out_c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            base[3] = (out_a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
}

// ── GPU path ──────────────────────────────────────────────────────────────────

#[cfg(feature = "wgpu")]
impl OverlayNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &OverlayPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Overlay shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/overlay.wgsl").into()),
            });

            // binding 0: base, 1: overlay, 2: sampler
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Overlay BGL"),
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
                ],
            });

            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Overlay layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

            let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Overlay pipeline"),
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
                label: Some("Overlay sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            OverlayPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                sampler,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for OverlayNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(tex_base) = inputs.first() else {
            log::warn!("OverlayNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("OverlayNode::process called with no outputs");
            return;
        };

        let pd = self.get_or_create_pipeline(ctx);

        // Upload the overlay frame to a temporary GPU texture.
        let ov_tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Overlay ov_tex"),
            size: wgpu::Extent3d {
                width: self.overlay_width,
                height: self.overlay_height,
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
                texture: &ov_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.overlay_rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.overlay_width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: self.overlay_width,
                height: self.overlay_height,
                depth_or_array_layers: 1,
            },
        );

        let base_view = tex_base.create_view(&wgpu::TextureViewDescriptor::default());
        let ov_view = ov_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Overlay BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ov_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&pd.sampler),
                },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Overlay pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Overlay pass"),
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_node_fully_opaque_overlay_should_replace_base() {
        let base = vec![50u8, 50, 50, 255];
        let overlay = vec![200u8, 100, 50, 255]; // alpha=255 → fully opaque
        let node = OverlayNode::new(overlay.clone(), 1, 1);
        let mut rgba = base;
        node.process_cpu(&mut rgba, 1, 1);
        // With overlay.alpha=255, output must equal overlay.
        assert!(
            (rgba[0] as i32 - 200).abs() <= 1,
            "R must match overlay; got {}",
            rgba[0]
        );
        assert!(
            (rgba[1] as i32 - 100).abs() <= 1,
            "G must match overlay; got {}",
            rgba[1]
        );
    }

    #[test]
    fn overlay_node_fully_transparent_overlay_should_preserve_base() {
        let base = vec![50u8, 80, 120, 255];
        let overlay = vec![200u8, 100, 50, 0]; // alpha=0 → invisible
        let node = OverlayNode::new(overlay, 1, 1);
        let mut rgba = base.clone();
        node.process_cpu(&mut rgba, 1, 1);
        // With overlay.alpha=0, output must equal base.
        assert!(
            (rgba[0] as i32 - 50).abs() <= 1,
            "R must match base; got {}",
            rgba[0]
        );
    }

    #[test]
    fn overlay_node_size_mismatch_should_be_noop() {
        let overlay = vec![200u8; 8]; // 2 pixels
        let node = OverlayNode::new(overlay, 2, 1);
        let original = vec![50u8, 80, 120, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1); // size mismatch
        assert_eq!(rgba, original);
    }
}
