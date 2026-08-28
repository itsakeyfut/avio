use super::RenderNodeCpu;

/// YUV sub-sampling format for [`YuvUploadNode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YuvFormat {
    /// Planar 4:2:0 — Y at full resolution; Cb/Cr at half width and height.
    #[default]
    Yuv420p,
    /// Planar 4:2:2 — Y at full resolution; Cb/Cr at half width.
    Yuv422p,
    /// Planar 4:4:4 — all planes at full resolution.
    Yuv444p,
}

// Pipeline cache

#[cfg(feature = "wgpu")]
struct YuvPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    y_tex: wgpu::Texture,
    cb_tex: wgpu::Texture,
    cr_tex: wgpu::Texture,
    uniform_buf: wgpu::Buffer,
}

// YuvUploadNode

/// Upload raw YUV plane buffers to the GPU and convert to RGBA in a fragment
/// shader, bypassing CPU-side `sws_scale`.
///
/// The node has `input_count() = 0`; it sources all pixel data from the plane
/// buffers set via [`YuvUploadNode::set_planes`]. Call `set_planes` once per
/// frame before the graph processes it.
pub struct YuvUploadNode {
    /// Pixel sub-sampling format.
    pub format: YuvFormat,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    y_plane: Vec<u8>,
    cb_plane: Vec<u8>,
    cr_plane: Vec<u8>,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<YuvPipeline>,
}

impl YuvUploadNode {
    /// Create a new node. Plane buffers are initialised to neutral values (Y = 0, Cb = Cr = 128).
    #[must_use]
    pub fn new(format: YuvFormat, width: u32, height: u32) -> Self {
        let (cw, ch) = chroma_dims(format, width, height);
        Self {
            format,
            width,
            height,
            y_plane: vec![0u8; (width * height) as usize],
            cb_plane: vec![128u8; (cw * ch) as usize],
            cr_plane: vec![128u8; (cw * ch) as usize],
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }

    /// Replace the stored plane buffers.
    ///
    /// Expected sizes for `width × height` at `format`:
    /// - `y`:       `width × height` bytes
    /// - `cb`, `cr`: `chroma_w × chroma_h` bytes (sub-sampled per [`YuvFormat`])
    pub fn set_planes(&mut self, y: Vec<u8>, cb: Vec<u8>, cr: Vec<u8>) {
        self.y_plane = y;
        self.cb_plane = cb;
        self.cr_plane = cr;
    }
}

impl Default for YuvUploadNode {
    fn default() -> Self {
        Self::new(YuvFormat::Yuv420p, 0, 0)
    }
}

/// Returns `(chroma_width, chroma_height)` for a given format and luma dimensions.
pub(crate) fn chroma_dims(format: YuvFormat, w: u32, h: u32) -> (u32, u32) {
    match format {
        YuvFormat::Yuv420p => (w.div_ceil(2), h.div_ceil(2)),
        YuvFormat::Yuv422p => (w.div_ceil(2), h),
        YuvFormat::Yuv444p => (w, h),
    }
}

fn chroma_divs(format: YuvFormat) -> (u32, u32) {
    match format {
        YuvFormat::Yuv420p => (2, 2),
        YuvFormat::Yuv422p => (2, 1),
        YuvFormat::Yuv444p => (1, 1),
    }
}

// CPU path

impl RenderNodeCpu for YuvUploadNode {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::many_single_char_names
    )]
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        if self.y_plane.is_empty() || self.width == 0 || self.height == 0 {
            return;
        }
        let (cw, _) = chroma_dims(self.format, self.width, self.height);
        let (x_div, y_div) = chroma_divs(self.format);
        let rows = h.min(self.height) as usize;
        let cols = w.min(self.width) as usize;
        for row in 0..rows {
            for col in 0..cols {
                let y_val = f32::from(self.y_plane[row * self.width as usize + col]) / 255.0;
                let cx = col / x_div as usize;
                let cy = row / y_div as usize;
                let ci = cy * cw as usize + cx;
                let cb = f32::from(self.cb_plane[ci]) / 255.0 - 0.5;
                let cr = f32::from(self.cr_plane[ci]) / 255.0 - 0.5;
                // BT.601 full-range YCbCr → linear RGB.
                let r = (y_val + 1.402 * cr).clamp(0.0, 1.0);
                let g = (y_val - 0.344 * cb - 0.714 * cr).clamp(0.0, 1.0);
                let b = (y_val + 1.772 * cb).clamp(0.0, 1.0);
                let idx = (row * w as usize + col) * 4;
                rgba[idx] = (r * 255.0 + 0.5) as u8;
                rgba[idx + 1] = (g * 255.0 + 0.5) as u8;
                rgba[idx + 2] = (b * 255.0 + 0.5) as u8;
                rgba[idx + 3] = 255;
            }
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
impl YuvUploadNode {
    #[allow(clippy::too_many_lines, clippy::similar_names)]
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &YuvPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let (cw, ch) = chroma_dims(self.format, self.width, self.height);

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("YuvUpload shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/yuv_upload.wgsl").into()),
            });

            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("YuvUpload BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
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
                label: Some("YuvUpload layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });

            let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("YuvUpload pipeline"),
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

            // Y luma plane (R8Unorm, full resolution).
            let y_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("YuvUpload Y"),
                size: wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });

            // Cb chroma plane (R8Unorm, sub-sampled).
            let cb_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("YuvUpload Cb"),
                size: wgpu::Extent3d {
                    width: cw,
                    height: ch,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });

            // Cr chroma plane (R8Unorm, sub-sampled).
            let cr_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("YuvUpload Cr"),
                size: wgpu::Extent3d {
                    width: cw,
                    height: ch,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });

            // Uniform buffer: [chroma_x_div, chroma_y_div, pad, pad] = 16 bytes.
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("YuvUpload uniforms"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            YuvPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                y_tex,
                cb_tex,
                cr_tex,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for YuvUploadNode {
    fn input_count(&self) -> usize {
        0
    }

    #[allow(clippy::too_many_lines, clippy::similar_names)]
    fn process(
        &self,
        _inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        if self.width == 0 || self.height == 0 || self.y_plane.is_empty() {
            log::warn!("YuvUploadNode::process called with empty frame data");
            return;
        }
        let Some(output) = outputs.first() else {
            log::warn!("YuvUploadNode::process called with no outputs");
            return;
        };

        let pd = self.get_or_create_pipeline(ctx);
        let (cw, ch) = chroma_dims(self.format, self.width, self.height);
        let (x_div, y_div) = chroma_divs(self.format);

        // Upload Y luma plane.
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &pd.y_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.y_plane,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        // Upload Cb chroma plane.
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &pd.cb_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.cb_plane,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cw),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: cw,
                height: ch,
                depth_or_array_layers: 1,
            },
        );

        // Upload Cr chroma plane.
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &pd.cr_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.cr_plane,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cw),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: cw,
                height: ch,
                depth_or_array_layers: 1,
            },
        );

        // Write chroma sub-sampling divisors to the uniform buffer.
        ctx.queue
            .write_buffer(&pd.uniform_buf, 0, &pack_u32(&[x_div, y_div, 0, 0]));

        let y_view = pd
            .y_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        let cb_view = pd
            .cb_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        let cr_view = pd
            .cr_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("YuvUpload BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&cb_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&cr_view),
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
                label: Some("YuvUpload pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("YuvUpload pass"),
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
    fn yuv_format_default_should_be_yuv420p() {
        assert_eq!(YuvFormat::default(), YuvFormat::Yuv420p);
    }

    #[test]
    fn chroma_dims_420p_should_halve_both_dimensions() {
        assert_eq!(chroma_dims(YuvFormat::Yuv420p, 4, 4), (2, 2));
        // Odd dimensions: ceiling division.
        assert_eq!(chroma_dims(YuvFormat::Yuv420p, 3, 3), (2, 2));
    }

    #[test]
    fn chroma_dims_422p_should_halve_width_only() {
        assert_eq!(chroma_dims(YuvFormat::Yuv422p, 4, 4), (2, 4));
        assert_eq!(chroma_dims(YuvFormat::Yuv422p, 3, 5), (2, 5));
    }

    #[test]
    fn chroma_dims_444p_should_be_full_resolution() {
        assert_eq!(chroma_dims(YuvFormat::Yuv444p, 4, 6), (4, 6));
    }

    #[test]
    fn yuv_upload_node_cpu_black_frame_should_produce_black() {
        let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 2, 2);
        node.set_planes(
            vec![0u8; 4],   // Y = 0
            vec![128u8; 1], // Cb = neutral
            vec![128u8; 1], // Cr = neutral
        );
        let mut rgba = vec![0u8; 16];
        node.process_cpu(&mut rgba, 2, 2);
        for pixel in rgba.chunks_exact(4) {
            assert!(pixel[0] <= 1, "R should be ~0 for Y=0; got {}", pixel[0]);
            assert!(pixel[1] <= 1, "G should be ~0 for Y=0; got {}", pixel[1]);
            assert!(pixel[2] <= 1, "B should be ~0 for Y=0; got {}", pixel[2]);
            assert_eq!(pixel[3], 255, "alpha must be opaque");
        }
    }

    #[test]
    fn yuv_upload_node_cpu_white_frame_should_produce_white() {
        let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 2, 2);
        node.set_planes(
            vec![255u8; 4], // Y = 255
            vec![128u8; 1], // Cb = neutral
            vec![128u8; 1], // Cr = neutral
        );
        let mut rgba = vec![0u8; 16];
        node.process_cpu(&mut rgba, 2, 2);
        for pixel in rgba.chunks_exact(4) {
            assert!(
                pixel[0] >= 254,
                "R should be ~255 for Y=255, neutral chroma; got {}",
                pixel[0]
            );
            assert!(
                pixel[1] >= 254,
                "G should be ~255 for Y=255, neutral chroma; got {}",
                pixel[1]
            );
            assert!(
                pixel[2] >= 254,
                "B should be ~255 for Y=255, neutral chroma; got {}",
                pixel[2]
            );
        }
    }

    #[test]
    fn yuv_upload_node_cpu_neutral_chroma_should_produce_grey() {
        let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 2, 2);
        // Y=128 → y_val ≈ 0.502, Cb=Cr=128 → cb=cr=0 → R=G=B ≈ 128.
        node.set_planes(vec![128u8; 4], vec![128u8; 1], vec![128u8; 1]);
        let mut rgba = vec![0u8; 16];
        node.process_cpu(&mut rgba, 2, 2);
        for pixel in rgba.chunks_exact(4) {
            let r = pixel[0] as i32;
            let g = pixel[1] as i32;
            let b = pixel[2] as i32;
            assert!(
                (r - 128).abs() <= 2,
                "R should be ~128 for neutral YUV; got {r}"
            );
            assert!(
                (g - 128).abs() <= 2,
                "G should be ~128 for neutral YUV; got {g}"
            );
            assert!(
                (b - 128).abs() <= 2,
                "B should be ~128 for neutral YUV; got {b}"
            );
        }
    }

    #[test]
    fn yuv_upload_node_cpu_422p_should_use_half_width_chroma() {
        // 4×2 frame, 422p: chroma planes are 2×2.
        let mut node = YuvUploadNode::new(YuvFormat::Yuv422p, 4, 2);
        node.set_planes(
            vec![128u8; 8], // 4×2 luma — neutral grey
            vec![128u8; 4], // 2×2 Cb
            vec![128u8; 4], // 2×2 Cr
        );
        let mut rgba = vec![0u8; 32];
        node.process_cpu(&mut rgba, 4, 2);
        for pixel in rgba.chunks_exact(4) {
            let r = pixel[0] as i32;
            assert!(
                (r - 128).abs() <= 2,
                "422p neutral: R should be ~128; got {r}"
            );
        }
    }

    #[test]
    fn yuv_upload_node_set_planes_should_update_stored_data() {
        let mut node = YuvUploadNode::new(YuvFormat::Yuv444p, 1, 1);
        // Default: Y=0, Cb=Cr=128 → near-black (128/255 ≈ 0.502, not exact 0.5).
        let mut rgba = vec![0u8; 4];
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[0] <= 2,
            "default Y=0 must produce near-black; got {}",
            rgba[0]
        );
        // After set_planes: Y=200, Cb=Cr=128 → bright grey.
        node.set_planes(vec![200], vec![128], vec![128]);
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[0] > 150,
            "Y=200 must produce bright output; got {}",
            rgba[0]
        );
    }

    #[test]
    fn yuv_upload_node_variant_and_error_types_should_compile() {
        let _ = YuvFormat::Yuv420p;
        let _ = YuvFormat::Yuv422p;
        let _ = YuvFormat::Yuv444p;
        let _ = YuvUploadNode::new(YuvFormat::Yuv420p, 320, 240);
        let _ = YuvUploadNode::default();
    }
}

// helpers

#[cfg(feature = "wgpu")]
fn pack_u32(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
