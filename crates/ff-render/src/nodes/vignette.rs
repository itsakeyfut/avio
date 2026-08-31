use super::RenderNodeCpu;

// Pipeline cache

#[cfg(feature = "wgpu")]
struct VignettePipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
}

// VignetteNode

/// Position-based radial vignette: darkens toward the corners with a smooth
/// falloff. A pixel's output depends only on its own colour and its normalised
/// distance from the centre, so the centre is left unmodified and `strength = 0`
/// is a no-op.
pub struct VignetteNode {
    /// Normalised distance from the centre where darkening begins (0.0 – 1.0; a
    /// corner is ~1.0).
    pub radius: f32,
    /// Maximum darkening applied at the corners (0.0 = no vignette; 1.0 = to
    /// black at full falloff).
    pub strength: f32,
    /// Width of the falloff band past `radius` (0.0 = hard edge).
    pub feather: f32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<VignettePipeline>,
}

impl VignetteNode {
    /// Creates a vignette with the given radius, strength, and feather.
    #[must_use]
    pub fn new(radius: f32, strength: f32, feather: f32) -> Self {
        Self {
            radius,
            strength,
            feather,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for VignetteNode {
    /// Identity node (no darkening).
    fn default() -> Self {
        Self::new(0.5, 0.0, 0.2)
    }
}

// CPU path

/// Smoothstep matching WGSL semantics; `edge1` is guarded against `feather == 0`
/// by the caller.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl RenderNodeCpu for VignetteNode {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::many_single_char_names
    )]
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let wf = w as f32;
        let hf = h as f32;
        let edge1 = self.radius + self.feather.max(1e-5);
        for (i, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = (i as u32 % w) as f32 + 0.5;
            let y = (i as u32 / w) as f32 + 0.5;
            // Centre-relative UV in [-0.5, 0.5]; distance normalised so a corner
            // is ~1.0.
            let dx = x / wf - 0.5;
            let dy = y / hf - 0.5;
            let d = (dx * dx + dy * dy).sqrt() * 2.0;
            let v = smoothstep(self.radius, edge1, d);
            let factor = 1.0 - v * self.strength;

            for c in &mut pixel[0..3] {
                *c = (f32::from(*c) * factor).clamp(0.0, 255.0) as u8;
            }
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
impl VignetteNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &VignettePipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Vignette shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/vignette.wgsl").into()),
            });

            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Vignette BGL"),
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
                label: Some("Vignette layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

            let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Vignette pipeline"),
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

            // 4 x f32 = 16 bytes: matches VignetteUniforms in the shader.
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vignette uniforms"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            VignettePipeline {
                render_pipeline,
                bind_group_layout: bgl,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for VignetteNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("VignetteNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("VignetteNode::process called with no outputs");
            return;
        };

        let pd = self.get_or_create_pipeline(ctx);

        let uniform_bytes: Vec<u8> = [self.radius, self.strength, self.feather, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniform_bytes);

        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Vignette BG"),
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
                label: Some("Vignette pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Vignette pass"),
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

    // A 9×9 frame has pixel (4,4) exactly at uv (0.5, 0.5), so the centre pixel
    // can be checked for exact invariance.
    fn solid_frame(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            buf.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        buf
    }

    #[test]
    fn vignette_node_should_darken_corners() {
        let node = VignetteNode::new(0.5, 0.8, 0.2);
        let (w, h) = (9u32, 9u32);
        let mut rgba = solid_frame(w, h, [200, 200, 200]);
        node.process_cpu(&mut rgba, w, h);
        // Top-left corner pixel must be darker than the original.
        let corner = rgba[0];
        assert!(
            corner < 200,
            "corner must be darkened by the vignette; got {corner}"
        );
    }

    #[test]
    fn vignette_node_should_leave_centre_unmodified() {
        let node = VignetteNode::new(0.5, 0.8, 0.2);
        let (w, h) = (9u32, 9u32);
        let mut rgba = solid_frame(w, h, [200, 200, 200]);
        node.process_cpu(&mut rgba, w, h);
        // Centre pixel (4,4) is at uv (0.5,0.5): dist 0, factor 1, unmodified.
        let centre = ((4 * w + 4) * 4) as usize;
        assert_eq!(rgba[centre], 200, "centre R must be unmodified");
        assert_eq!(rgba[centre + 1], 200, "centre G must be unmodified");
        assert_eq!(rgba[centre + 2], 200, "centre B must be unmodified");
    }

    #[test]
    fn vignette_node_strength_zero_should_be_noop() {
        let node = VignetteNode::new(0.5, 0.0, 0.2);
        let (w, h) = (8u32, 8u32);
        let original = solid_frame(w, h, [200, 150, 100]);
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, w, h);
        assert_eq!(rgba, original, "strength=0 must be a no-op everywhere");
    }
}

#[cfg(all(test, feature = "wgpu"))]
mod gpu_tests {
    use super::*;
    use crate::context::RenderContext;
    use crate::graph::RenderGraph;
    use std::sync::Arc;

    /// A headless GPU context, or `None` when no adapter is available (CI).
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

    #[test]
    fn vignette_gpu_should_darken_corners_and_keep_centre() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (9u32, 9u32);
        let frame = solid(w, h, 200);
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(VignetteNode::new(0.5, 0.8, 0.2))
            .process_gpu(&frame, w, h)
            .expect("gpu vignette");

        let centre = ((4 * w + 4) * 4) as usize;
        assert!(
            i32::from(gpu[centre]) >= 199,
            "centre must stay ~unmodified; got {}",
            gpu[centre]
        );
        assert!(gpu[0] < 200, "corner must be darkened; got {}", gpu[0]);
    }
}
