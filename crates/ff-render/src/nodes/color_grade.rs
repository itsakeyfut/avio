use super::RenderNodeCpu;

// Pipeline cache

#[cfg(feature = "wgpu")]
struct ColorGradePipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

// ColorGradeNode

/// Basic colour grading: brightness, contrast, saturation, temperature, tint.
///
/// # Processing order
///
/// brightness → contrast → temperature/tint → saturation
pub struct ColorGradeNode {
    /// Additive brightness offset (−1.0 – +1.0; 0.0 = no change).
    pub brightness: f32,
    /// Contrast multiplier around 0.5 (0.0 – 4.0; 1.0 = no change).
    pub contrast: f32,
    /// Saturation multiplier (0.0 = greyscale; 1.0 = no change; 2.0 = double).
    pub saturation: f32,
    /// Colour temperature offset (−1.0 = cool/blue; +1.0 = warm/orange).
    pub temperature: f32,
    /// Tint offset (−1.0 = magenta; +1.0 = green).
    pub tint: f32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<ColorGradePipeline>,
}

impl ColorGradeNode {
    /// Identity node (no colour change).
    #[must_use]
    pub fn new(
        brightness: f32,
        contrast: f32,
        saturation: f32,
        temperature: f32,
        tint: f32,
    ) -> Self {
        Self {
            brightness,
            contrast,
            saturation,
            temperature,
            tint,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for ColorGradeNode {
    fn default() -> Self {
        Self::new(0.0, 1.0, 1.0, 0.0, 0.0)
    }
}

// CPU path

impl RenderNodeCpu for ColorGradeNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let r = f32::from(pixel[0]) / 255.0;
            let g = f32::from(pixel[1]) / 255.0;
            let b = f32::from(pixel[2]) / 255.0;

            // Brightness
            let r = r + self.brightness;
            let g = g + self.brightness;
            let b = b + self.brightness;

            // Contrast
            let r = (r - 0.5) * self.contrast + 0.5;
            let g = (g - 0.5) * self.contrast + 0.5;
            let b = (b - 0.5) * self.contrast + 0.5;

            // Temperature
            let r = r + self.temperature * 0.1;
            let b = b - self.temperature * 0.1;

            // Tint
            let g = g + self.tint * 0.1;

            // Saturation (BT.709 luma coefficients)
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let r = luma + (r - luma) * self.saturation;
            let g = luma + (g - luma) * self.saturation;
            let b = luma + (b - luma) * self.saturation;

            pixel[0] = (r.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            pixel[1] = (g.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            pixel[2] = (b.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
impl ColorGradeNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &ColorGradePipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ColorGrade shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../shaders/color_grade.wgsl").into(),
                ),
            });

            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ColorGrade BGL"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
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
                label: Some("ColorGrade layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

            let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ColorGrade pipeline"),
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
                label: Some("ColorGrade sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            // 8 × f32 = 32 bytes — matches ColorGradeUniforms in the shader.
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ColorGrade uniforms"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            ColorGradePipeline {
                render_pipeline,
                bind_group_layout: bgl,
                sampler,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for ColorGradeNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("ColorGradeNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("ColorGradeNode::process called with no outputs");
            return;
        };

        let pd = self.get_or_create_pipeline(ctx);

        // Update uniforms for this frame.
        let uniform_bytes = pack_f32(&[
            self.brightness,
            self.contrast,
            self.saturation,
            self.temperature,
            self.tint,
            0.0,
            0.0,
            0.0,
        ]);
        ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniform_bytes);

        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ColorGrade BG"),
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pd.uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ColorGrade pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ColorGrade pass"),
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
    fn color_grade_node_default_should_be_identity() {
        let node = ColorGradeNode::default();
        let original = vec![100u8, 150, 200, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        // Identity: brightness=0, contrast=1, saturation=1, temperature=0, tint=0.
        // Allow ±1 rounding error from f32 round-trip.
        for i in 0..3 {
            let diff = (rgba[i] as i32 - original[i] as i32).abs();
            assert!(
                diff <= 1,
                "identity must preserve pixel at channel {i}: expected ~{} got {}",
                original[i],
                rgba[i]
            );
        }
        assert_eq!(rgba[3], 255, "alpha must not change");
    }

    #[test]
    fn color_grade_node_brightness_positive_should_increase_mid_grey() {
        let node = ColorGradeNode {
            brightness: 0.5,
            ..Default::default()
        };
        let mut rgba = vec![128u8, 128, 128, 255]; // mid-grey
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[0] > 128,
            "brightness +0.5 must increase R; got {}",
            rgba[0]
        );
        assert!(
            rgba[1] > 128,
            "brightness +0.5 must increase G; got {}",
            rgba[1]
        );
        assert!(
            rgba[2] > 128,
            "brightness +0.5 must increase B; got {}",
            rgba[2]
        );
        assert_eq!(rgba[3], 255, "alpha must not change");
    }

    #[test]
    fn color_grade_node_brightness_negative_should_decrease_mid_grey() {
        let node = ColorGradeNode {
            brightness: -0.5,
            ..Default::default()
        };
        let mut rgba = vec![128u8, 128, 128, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[0] < 128,
            "brightness −0.5 must decrease R; got {}",
            rgba[0]
        );
    }

    #[test]
    fn color_grade_node_saturation_zero_should_produce_greyscale() {
        let node = ColorGradeNode {
            saturation: 0.0,
            ..Default::default()
        };
        let mut rgba = vec![200u8, 100, 50, 255]; // colourful pixel
        node.process_cpu(&mut rgba, 1, 1);
        // All channels must be equal (greyscale) — allow ±1 rounding.
        let diff_rg = (rgba[0] as i32 - rgba[1] as i32).abs();
        let diff_rb = (rgba[0] as i32 - rgba[2] as i32).abs();
        assert!(
            diff_rg <= 1,
            "saturation=0 must equalise R and G; got R={} G={}",
            rgba[0],
            rgba[1]
        );
        assert!(
            diff_rb <= 1,
            "saturation=0 must equalise R and B; got R={} B={}",
            rgba[0],
            rgba[2]
        );
    }

    #[test]
    fn color_grade_node_clamp_should_not_exceed_255() {
        let node = ColorGradeNode {
            brightness: 2.0,
            ..Default::default()
        };
        let mut rgba = vec![200u8, 200, 200, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba[0], 255, "clamped R must be 255");
        assert_eq!(rgba[1], 255, "clamped G must be 255");
        assert_eq!(rgba[2], 255, "clamped B must be 255");
    }

    #[test]
    fn color_grade_node_variants_should_construct_via_default() {
        let _ = ColorGradeNode {
            brightness: 0.1,
            contrast: 1.2,
            saturation: 0.9,
            ..Default::default()
        };
    }
}

// helpers

#[cfg(feature = "wgpu")]
fn pack_f32(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|f| f.to_le_bytes()).collect()
}
