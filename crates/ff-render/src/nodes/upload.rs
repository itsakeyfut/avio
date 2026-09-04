use super::RenderNodeCpu;

/// YUV sub-sampling format for [`YuvUploadNode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum YuvFormat {
    /// Planar 4:2:0 — Y at full resolution; Cb/Cr at half width and height.
    #[default]
    Yuv420p,
    /// Planar 4:2:2 — Y at full resolution; Cb/Cr at half width.
    Yuv422p,
    /// Planar 4:4:4 — all planes at full resolution.
    Yuv444p,
}

/// How the sample data for a [`YuvUploadNode`] is laid out in memory.
///
/// One enum rather than a pair of `bool`s (`high_bit_depth` + `semi_planar`)
/// because the fourth combination — 8-bit semi-planar, i.e. NV12 — is not a
/// layout this node supports, and a pair of flags would let it be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaneLayout {
    /// Three planes, one byte per sample.
    Planar8,
    /// Three planes of little-endian `u16` samples holding `0..=1023` in the
    /// low bits (`Yuv420p10le` and friends).
    Planar10,
    /// A luma plane plus one plane of interleaved Cb/Cr, little-endian `u16`
    /// samples with the 10 significant bits in the *high* bits (`P010le`).
    SemiPlanar10,
}

// Pipeline cache

#[cfg(feature = "wgpu")]
struct YuvPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    y_tex: wgpu::Texture,
    /// Cb for a planar layout; the interleaved Cb/Cr plane for a semi-planar one.
    chroma_tex: wgpu::Texture,
    /// Cr — planar layouts only; semi-planar carries it in `chroma_tex`.
    cr_tex: Option<wgpu::Texture>,
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
    /// Memory layout of the stored planes, fixed by the constructor used. The
    /// 10-bit layouts render to an `Rgba16Float` target so precision survives.
    layout: PlaneLayout,
    y_plane: Vec<u8>,
    /// Cb — planar layouts only.
    cb_plane: Vec<u8>,
    /// Cr — planar layouts only.
    cr_plane: Vec<u8>,
    /// Interleaved Cb/Cr — semi-planar layout only; empty otherwise.
    uv_plane: Vec<u8>,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<YuvPipeline>,
}

/// Neutral chroma value for 10-bit YUV (mid of the `0..=1023` range).
const TEN_BIT_NEUTRAL_CHROMA: u16 = 512;
/// Maximum 10-bit sample value.
const TEN_BIT_MAX: f32 = 1023.0;
/// Bits P010 leaves zeroed at the bottom of each 16-bit sample. Its 10
/// significant bits are MSB-aligned, matching `FFmpeg`'s own `P010LE` pixel
/// descriptor (`depth = 10, shift = 6`).
const P010_SHIFT: u32 = 6;

impl YuvUploadNode {
    /// Create a new 8-bit node. Plane buffers are initialised to neutral values (Y = 0, Cb = Cr = 128).
    #[must_use]
    pub fn new(format: YuvFormat, width: u32, height: u32) -> Self {
        let (cw, ch) = chroma_dims(format, width, height);
        Self {
            format,
            width,
            height,
            layout: PlaneLayout::Planar8,
            y_plane: vec![0u8; (width * height) as usize],
            cb_plane: vec![128u8; (cw * ch) as usize],
            cr_plane: vec![128u8; (cw * ch) as usize],
            uv_plane: Vec::new(),
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }

    /// Create a new 10-bit planar node. Plane buffers hold little-endian `u16`
    /// samples (values `0..=1023`) and render to an `Rgba16Float` target, so
    /// 10-bit precision survives the upload. Neutral init: Y = 0, Cb = Cr = 512.
    #[must_use]
    pub fn new_high_bit_depth(format: YuvFormat, width: u32, height: u32) -> Self {
        let (cw, ch) = chroma_dims(format, width, height);
        Self {
            format,
            width,
            height,
            layout: PlaneLayout::Planar10,
            y_plane: vec![0u8; (width * height * 2) as usize],
            cb_plane: u16_le_plane(TEN_BIT_NEUTRAL_CHROMA, (cw * ch) as usize),
            cr_plane: u16_le_plane(TEN_BIT_NEUTRAL_CHROMA, (cw * ch) as usize),
            uv_plane: Vec::new(),
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }

    /// Create a new 10-bit semi-planar (`P010le`) node: a full-resolution luma
    /// plane plus one plane of interleaved Cb/Cr, both little-endian `u16`.
    ///
    /// P010 is always 4:2:0, so there is no [`YuvFormat`] to choose. Its samples
    /// are MSB-aligned — the 10 significant bits sit in the *high* bits of each
    /// 16-bit sample, with the low 6 zeroed — unlike
    /// [`new_high_bit_depth`](Self::new_high_bit_depth), whose planes hold
    /// `0..=1023` in the low bits. Renders to an `Rgba16Float` target.
    ///
    /// Neutral init: Y = 0, Cb = Cr = 512 (MSB-aligned).
    #[must_use]
    pub fn new_p010(width: u32, height: u32) -> Self {
        let format = YuvFormat::Yuv420p;
        let (cw, ch) = chroma_dims(format, width, height);
        Self {
            format,
            width,
            height,
            layout: PlaneLayout::SemiPlanar10,
            y_plane: vec![0u8; (width * height * 2) as usize],
            cb_plane: Vec::new(),
            cr_plane: Vec::new(),
            // Two samples (Cb, Cr) per chroma pixel.
            uv_plane: u16_le_plane(TEN_BIT_NEUTRAL_CHROMA << P010_SHIFT, (cw * ch * 2) as usize),
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }

    /// Replace the stored plane buffers of a planar node ([`new`](Self::new) or
    /// [`new_high_bit_depth`](Self::new_high_bit_depth)).
    ///
    /// A semi-planar node ([`new_p010`](Self::new_p010)) reads neither `cb` nor
    /// `cr`, and its interleaved plane keeps the neutral chroma the constructor
    /// gave it — which is correctly sized, so no length check catches the
    /// mistake and the frame renders **greyscale** rather than failing. Use
    /// [`set_planes_semi_planar`](Self::set_planes_semi_planar) there.
    ///
    /// Expected sizes for `width × height` at `format`, per sample:
    /// - 8-bit: 1 byte; 10-bit ([`new_high_bit_depth`](Self::new_high_bit_depth)): 2 bytes (little-endian `u16`)
    /// - `y`:       `width × height` samples
    /// - `cb`, `cr`: `chroma_w × chroma_h` samples (sub-sampled per [`YuvFormat`])
    pub fn set_planes(&mut self, y: Vec<u8>, cb: Vec<u8>, cr: Vec<u8>) {
        self.y_plane = y;
        self.cb_plane = cb;
        self.cr_plane = cr;
    }

    /// Replace the stored planes of a semi-planar node
    /// ([`new_p010`](Self::new_p010)).
    ///
    /// Both planes hold little-endian `u16` samples (2 bytes each):
    /// - `y`: `width × height` samples
    /// - `uv`: `chroma_w × chroma_h × 2` samples — Cb and Cr interleaved, one
    ///   pair per chroma pixel, so `uv` holds twice as many samples as a single
    ///   planar chroma plane would. This is the layout `ff-format` gives
    ///   `P010le` (`uv_stride = width × 2` bytes over `height / 2` rows).
    ///
    /// Planes are expected dense (no row padding). A plane too short for the
    /// node's dimensions is refused with a warning rather than read past its end.
    ///
    /// The mirror of the trap in [`set_planes`](Self::set_planes): a planar node
    /// never reads `uv`, so calling this on one updates only the luma and leaves
    /// the chroma at its neutral init, silently.
    pub fn set_planes_semi_planar(&mut self, y: Vec<u8>, uv: Vec<u8>) {
        self.y_plane = y;
        self.uv_plane = uv;
    }

    /// `true` when the stored semi-planar planes are large enough for the node's
    /// dimensions.
    ///
    /// Checked up front because a short plane would index past its end on the
    /// CPU path and hand `write_texture` an undersized slice on the GPU one.
    fn semi_planar_planes_are_complete(&self) -> bool {
        let (cw, ch) = chroma_dims(self.format, self.width, self.height);
        let luma_bytes = (self.width as usize) * (self.height as usize) * 2;
        // Cb and Cr interleaved: two `u16` samples, so 4 bytes, per chroma pixel.
        let uv_bytes = (cw as usize) * (ch as usize) * 4;
        self.y_plane.len() >= luma_bytes && self.uv_plane.len() >= uv_bytes
    }
}

/// Build a plane of `count` little-endian `u16` samples all equal to `value`.
fn u16_le_plane(value: u16, count: usize) -> Vec<u8> {
    value
        .to_le_bytes()
        .iter()
        .copied()
        .cycle()
        .take(count * 2)
        .collect()
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
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        if self.y_plane.is_empty() || self.width == 0 || self.height == 0 {
            return;
        }
        match self.layout {
            PlaneLayout::Planar8 => self.process_cpu_8bit(rgba, w, h),
            PlaneLayout::Planar10 => self.process_cpu_10bit(rgba, w, h),
            PlaneLayout::SemiPlanar10 => self.process_cpu_p010(rgba, w, h),
        }
    }
}

impl YuvUploadNode {
    /// CPU YCbCr→RGBA for 8-bit planar input (1 byte per sample).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::many_single_char_names
    )]
    fn process_cpu_8bit(&self, rgba: &mut [u8], w: u32, h: u32) {
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
                write_ycbcr_rgba(rgba, (row * w as usize + col) * 4, y_val, cb, cr);
            }
        }
    }

    /// CPU YCbCr→RGBA for 10-bit planar input (little-endian `u16` samples,
    /// values `0..=1023`). The CPU fallback still writes 8-bit RGBA, so it loses
    /// precision the GPU `Rgba16Float` path preserves.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::many_single_char_names
    )]
    fn process_cpu_10bit(&self, rgba: &mut [u8], w: u32, h: u32) {
        let (cw, _) = chroma_dims(self.format, self.width, self.height);
        let (x_div, y_div) = chroma_divs(self.format);
        let rows = h.min(self.height) as usize;
        let cols = w.min(self.width) as usize;
        for row in 0..rows {
            for col in 0..cols {
                let y_val =
                    sample_u16_le(&self.y_plane, row * self.width as usize + col) / TEN_BIT_MAX;
                let cx = col / x_div as usize;
                let cy = row / y_div as usize;
                let ci = cy * cw as usize + cx;
                let cb = sample_u16_le(&self.cb_plane, ci) / TEN_BIT_MAX - 0.5;
                let cr = sample_u16_le(&self.cr_plane, ci) / TEN_BIT_MAX - 0.5;
                write_ycbcr_rgba(rgba, (row * w as usize + col) * 4, y_val, cb, cr);
            }
        }
    }

    /// CPU YCbCr→RGBA for 10-bit semi-planar (`P010le`) input: a luma plane plus
    /// one plane of interleaved Cb/Cr, both little-endian `u16` with the 10
    /// significant bits MSB-aligned. Like the planar 10-bit leg, the CPU
    /// fallback writes 8-bit RGBA and so loses precision the GPU `Rgba16Float`
    /// path preserves.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::many_single_char_names
    )]
    fn process_cpu_p010(&self, rgba: &mut [u8], w: u32, h: u32) {
        if !self.semi_planar_planes_are_complete() {
            log::warn!(
                "YuvUploadNode P010 planes too small for the frame: width={} height={} y_len={} uv_len={}",
                self.width,
                self.height,
                self.y_plane.len(),
                self.uv_plane.len()
            );
            return;
        }
        let (cw, _) = chroma_dims(self.format, self.width, self.height);
        let (x_div, y_div) = chroma_divs(self.format);
        let rows = h.min(self.height) as usize;
        let cols = w.min(self.width) as usize;
        for row in 0..rows {
            for col in 0..cols {
                let y_val = p010_norm(&self.y_plane, row * self.width as usize + col);
                let cx = col / x_div as usize;
                let cy = row / y_div as usize;
                // Cb and Cr are adjacent samples of the same chroma pixel.
                let ci = (cy * cw as usize + cx) * 2;
                let cb = p010_norm(&self.uv_plane, ci) - 0.5;
                let cr = p010_norm(&self.uv_plane, ci + 1) - 0.5;
                write_ycbcr_rgba(rgba, (row * w as usize + col) * 4, y_val, cb, cr);
            }
        }
    }
}

/// Read the `i`-th little-endian `u16` sample of a plane.
fn raw_u16_le(plane: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([plane[i * 2], plane[i * 2 + 1]])
}

/// Read the `i`-th little-endian `u16` sample of a plane as `f32`.
fn sample_u16_le(plane: &[u8], i: usize) -> f32 {
    f32::from(raw_u16_le(plane, i))
}

/// Read the `i`-th MSB-aligned P010 sample of a plane, normalised to `[0, 1]`.
///
/// Dropping the zeroed low [`P010_SHIFT`] bits before dividing by
/// [`TEN_BIT_MAX`] is what makes a P010 sample agree with the planar 10-bit
/// path: both then divide the same `0..=1023` value.
///
/// Named for the normalisation, not for the read: [`sample_u16_le`] returns the
/// raw value and leaves the divide to its caller, so a `_sample` twin here would
/// invite dividing by [`TEN_BIT_MAX`] a second time. `p010_norm` is also what the
/// shader calls its own copy of this.
fn p010_norm(plane: &[u8], i: usize) -> f32 {
    f32::from(raw_u16_le(plane, i) >> P010_SHIFT) / TEN_BIT_MAX
}

/// BT.601 full-range YCbCr → RGBA, writing 4 bytes at `idx` (alpha = 255).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn write_ycbcr_rgba(rgba: &mut [u8], idx: usize, y_val: f32, cb: f32, cr: f32) {
    let r = (y_val + 1.402 * cr).clamp(0.0, 1.0);
    let g = (y_val - 0.344 * cb - 0.714 * cr).clamp(0.0, 1.0);
    let b = (y_val + 1.772 * cb).clamp(0.0, 1.0);
    rgba[idx] = (r * 255.0 + 0.5) as u8;
    rgba[idx + 1] = (g * 255.0 + 0.5) as u8;
    rgba[idx + 2] = (b * 255.0 + 0.5) as u8;
    rgba[idx + 3] = 255;
}

// GPU path

#[cfg(feature = "wgpu")]
impl YuvUploadNode {
    #[allow(clippy::too_many_lines, clippy::similar_names)]
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &YuvPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let (cw, ch) = chroma_dims(self.format, self.width, self.height);

            // 10-bit planes are *Uint (raw samples) divided by max_value in the
            // shader, rendering to Rgba16Float so precision survives. R16Uint and
            // Rg16Uint are core formats (unlike R16Unorm, which needs an optional
            // device feature). 8-bit planes stay R8Unorm sampled as normalised
            // floats, rendering to Rgba8Unorm. The target format is threaded
            // through rather than hardcoded, so an Rgba16Float graph gets a
            // matching attachment.
            let (luma_format, chroma_format, target_format, sample_type, shader_src) =
                match self.layout {
                    PlaneLayout::Planar8 => (
                        wgpu::TextureFormat::R8Unorm,
                        wgpu::TextureFormat::R8Unorm,
                        wgpu::TextureFormat::Rgba8Unorm,
                        wgpu::TextureSampleType::Float { filterable: false },
                        include_str!("../shaders/yuv_upload.wgsl"),
                    ),
                    PlaneLayout::Planar10 => (
                        wgpu::TextureFormat::R16Uint,
                        wgpu::TextureFormat::R16Uint,
                        wgpu::TextureFormat::Rgba16Float,
                        wgpu::TextureSampleType::Uint,
                        include_str!("../shaders/yuv_upload_10bit.wgsl"),
                    ),
                    PlaneLayout::SemiPlanar10 => (
                        wgpu::TextureFormat::R16Uint,
                        // Cb and Cr are the two channels of one texel.
                        wgpu::TextureFormat::Rg16Uint,
                        wgpu::TextureFormat::Rgba16Float,
                        wgpu::TextureSampleType::Uint,
                        include_str!("../shaders/p010_upload.wgsl"),
                    ),
                };
            let planar = self.layout != PlaneLayout::SemiPlanar10;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("YuvUpload shader"),
                source: wgpu::ShaderSource::Wgsl(shader_src.into()),
            });

            let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            };
            // Semi-planar input carries Cb and Cr in one texture, so binding 2 is
            // absent there. The uniform stays at binding 3 in every layout so all
            // three upload shaders agree on where it is.
            let mut entries = vec![texture_entry(0), texture_entry(1)];
            if planar {
                entries.push(texture_entry(2));
            }
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });

            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("YuvUpload BGL"),
                entries: &entries,
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
                        format: target_format,
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

            let plane_tex = |label: &str, format: wgpu::TextureFormat, w: u32, h: u32| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            };

            // Y luma plane (full resolution).
            let y_tex = plane_tex("YuvUpload Y", luma_format, self.width, self.height);
            // Cb, or the interleaved Cb/Cr plane for a semi-planar layout
            // (sub-sampled either way).
            let chroma_tex = plane_tex(
                if planar {
                    "YuvUpload Cb"
                } else {
                    "YuvUpload UV"
                },
                chroma_format,
                cw,
                ch,
            );
            // Cr chroma plane (sub-sampled) — planar layouts only.
            let cr_tex = planar.then(|| plane_tex("YuvUpload Cr", chroma_format, cw, ch));

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
                chroma_tex,
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
        if self.layout == PlaneLayout::SemiPlanar10 && !self.semi_planar_planes_are_complete() {
            log::warn!(
                "YuvUploadNode::process P010 planes too small for the frame: width={} height={} y_len={} uv_len={}",
                self.width,
                self.height,
                self.y_plane.len(),
                self.uv_plane.len()
            );
            return;
        }

        let pd = self.get_or_create_pipeline(ctx);
        let (cw, ch) = chroma_dims(self.format, self.width, self.height);
        let (x_div, y_div) = chroma_divs(self.format);
        // Bytes per texel: R8Unorm = 1, R16Uint = 2, Rg16Uint = 4 (Cb and Cr in
        // one texel).
        let (luma_bpt, chroma_bpt) = match self.layout {
            PlaneLayout::Planar8 => (1, 1),
            PlaneLayout::Planar10 => (2, 2),
            PlaneLayout::SemiPlanar10 => (2, 4),
        };
        let chroma_plane = if self.layout == PlaneLayout::SemiPlanar10 {
            &self.uv_plane
        } else {
            &self.cb_plane
        };

        let upload = |tex: &wgpu::Texture, data: &[u8], w: u32, h: u32, bpt: u32| {
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * bpt),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        };

        upload(&pd.y_tex, &self.y_plane, self.width, self.height, luma_bpt);
        upload(&pd.chroma_tex, chroma_plane, cw, ch, chroma_bpt);
        // Semi-planar input has no separate Cr plane; its texture is not created.
        if let Some(cr_tex) = pd.cr_tex.as_ref() {
            upload(cr_tex, &self.cr_plane, cw, ch, chroma_bpt);
        }

        // Uniforms: [chroma_x_div: u32, chroma_y_div: u32, max_value: f32, pad].
        // max_value is the 10-bit shaders' normalisation divisor (1023) — P010
        // shifts its MSB-aligned samples down first, so the divisor is the same;
        // the 8-bit shader ignores this slot (its samples are pre-normalised).
        let mut uniforms = [0u8; 16];
        uniforms[0..4].copy_from_slice(&x_div.to_le_bytes());
        uniforms[4..8].copy_from_slice(&y_div.to_le_bytes());
        uniforms[8..12].copy_from_slice(&TEN_BIT_MAX.to_le_bytes());
        ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniforms);

        let y_view = pd
            .y_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        let chroma_view = pd
            .chroma_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        let cr_view = pd
            .cr_tex
            .as_ref()
            .map(|tex| tex.create_view(&wgpu::TextureViewDescriptor::default()));
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        // Mirrors the layout built in `get_or_create_pipeline`: binding 2 exists
        // only for a planar layout, the uniform is always at binding 3.
        let mut bg_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&y_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&chroma_view),
            },
        ];
        if let Some(cr_view) = cr_view.as_ref() {
            bg_entries.push(wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(cr_view),
            });
        }
        bg_entries.push(wgpu::BindGroupEntry {
            binding: 3,
            resource: pd.uniform_buf.as_entire_binding(),
        });

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("YuvUpload BG"),
            layout: &pd.bind_group_layout,
            entries: &bg_entries,
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
    fn yuv_upload_cpu_10bit_should_decode_u16_planes() {
        // Y = 768 (10-bit) → 768/1023 ≈ 0.751 → grey ≈ 191. Neutral chroma 512.
        // A byte-truncating misread of the little-endian u16 (low byte 0x00)
        // would yield 0, so asserting ~191 proves the u16 decode (non-vacuous).
        let mut node = YuvUploadNode::new_high_bit_depth(YuvFormat::Yuv420p, 2, 2);
        node.set_planes(
            u16_le_plane(768, 4),
            u16_le_plane(512, 1),
            u16_le_plane(512, 1),
        );
        let mut rgba = vec![0u8; 16];
        node.process_cpu(&mut rgba, 2, 2);
        for pixel in rgba.chunks_exact(4) {
            let r = i32::from(pixel[0]);
            assert!(
                (r - 191).abs() <= 3,
                "10-bit Y=768 must decode to ~191; got {r}"
            );
            assert_eq!(pixel[3], 255, "alpha must be opaque");
        }
    }

    /// A plane of the given little-endian `u16` samples.
    fn u16_le_samples(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// The same samples MSB-aligned the way P010 stores them.
    fn p010_samples(values: &[u16]) -> Vec<u8> {
        let shifted: Vec<u16> = values.iter().map(|v| v << P010_SHIFT).collect();
        u16_le_samples(&shifted)
    }

    #[test]
    fn yuv_upload_cpu_p010_should_decode_msb_aligned_samples() {
        // Y = 768 once the 6-bit shift is undone → 768/1023 ≈ 0.751 → grey ≈ 191.
        // Non-vacuous: reading the sample without shifting divides 49152 by 1023
        // and clamps to white (255), which is nowhere near 191.
        let mut node = YuvUploadNode::new_p010(2, 2);
        node.set_planes_semi_planar(p010_samples(&[768; 4]), p010_samples(&[512; 2]));
        let mut rgba = vec![0u8; 16];
        node.process_cpu(&mut rgba, 2, 2);
        for pixel in rgba.chunks_exact(4) {
            let r = i32::from(pixel[0]);
            assert!(
                (r - 191).abs() <= 3,
                "P010 Y=768 must decode to ~191; got {r} (255 means the shift was skipped)"
            );
            assert_eq!(pixel[3], 255, "alpha must be opaque");
        }
    }

    #[test]
    fn yuv_upload_cpu_p010_should_deinterleave_cb_and_cr() {
        // 4×2 at 4:2:0 → a 2×1 chroma plane, so the UV plane holds two pairs:
        // [Cb0, Cr0, Cb1, Cr1]. Giving the two chroma columns opposite Cb/Cr
        // makes a swapped or mis-strided read visible: the channel that moves
        // changes. A single chroma column, or w == h, would hide both.
        let mut node = YuvUploadNode::new_p010(4, 2);
        node.set_planes_semi_planar(p010_samples(&[512; 8]), p010_samples(&[512, 800, 800, 512]));
        let mut rgba = vec![0u8; 32];
        node.process_cpu(&mut rgba, 4, 2);

        let red_and_blue = |x: usize, y: usize| -> (i32, i32) {
            let i = (y * 4 + x) * 4;
            (i32::from(rgba[i]), i32::from(rgba[i + 2]))
        };
        // Chroma column 0 covers x = 0..2: Cr is high, so red rises and blue
        // stays neutral.
        for x in [0, 1] {
            for y in [0, 1] {
                let (r, b) = red_and_blue(x, y);
                assert!(r > 200, "Cr=800 must push R high at ({x},{y}); got {r}");
                assert!(b < 160, "Cb=512 must leave B neutral at ({x},{y}); got {b}");
            }
        }
        // Chroma column 1 covers x = 2..4, with the roles swapped.
        for x in [2, 3] {
            for y in [0, 1] {
                let (r, b) = red_and_blue(x, y);
                assert!(r < 160, "Cr=512 must leave R neutral at ({x},{y}); got {r}");
                assert!(b > 200, "Cb=800 must push B high at ({x},{y}); got {b}");
            }
        }
    }

    #[test]
    fn yuv_upload_cpu_p010_should_match_planar_10bit_for_the_same_samples() {
        // The strongest pin on the shift, and unlike the GPU tests it runs
        // everywhere: P010 fed `v << 6` must land on exactly the pixels the
        // already-verified planar 10-bit path produces from `v`. Samples vary per
        // pixel and the two chroma columns differ, so a transposed or constant
        // read cannot pass.
        const Y: [u16; 8] = [100, 300, 500, 700, 900, 200, 400, 600];
        const CB: [u16; 2] = [300, 700];
        const CR: [u16; 2] = [800, 200];

        let mut planar = YuvUploadNode::new_high_bit_depth(YuvFormat::Yuv420p, 4, 2);
        planar.set_planes(u16_le_samples(&Y), u16_le_samples(&CB), u16_le_samples(&CR));
        let mut expected = vec![0u8; 32];
        planar.process_cpu(&mut expected, 4, 2);

        let mut p010 = YuvUploadNode::new_p010(4, 2);
        p010.set_planes_semi_planar(
            p010_samples(&Y),
            p010_samples(&[CB[0], CR[0], CB[1], CR[1]]),
        );
        let mut got = vec![0u8; 32];
        p010.process_cpu(&mut got, 4, 2);

        assert_eq!(
            got, expected,
            "P010 must decode to the same pixels as the planar 10-bit path"
        );
        // Non-vacuous: a flat frame on both sides would satisfy the comparison
        // without exercising anything, so check the fixture actually varies.
        assert!(
            expected.chunks_exact(4).any(|p| p[0] != expected[0]),
            "the fixture must produce varying pixels"
        );
    }

    #[test]
    fn yuv_upload_cpu_p010_should_refuse_planes_too_small_for_the_frame() {
        // 4×2 needs 4 UV samples; two must be refused rather than read past the
        // end of the plane.
        let mut node = YuvUploadNode::new_p010(4, 2);
        node.set_planes_semi_planar(p010_samples(&[512; 8]), p010_samples(&[512, 800]));
        let mut rgba = vec![7u8; 32];
        node.process_cpu(&mut rgba, 4, 2);
        assert!(
            rgba.iter().all(|&b| b == 7),
            "an undersized UV plane must leave the output untouched"
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
