//! Gaussian blur and unsharp-mask sharpen render nodes.
//!
//! [`GaussianBlurNode`] is a two-pass separable Gaussian blur (horizontal then
//! vertical). [`SharpenNode`] is an unsharp mask: it blurs (two separable passes)
//! then combines `orig + (orig - blur) * strength` in a third pass. Both expose a
//! CPU fallback ([`RenderNodeCpu`]) that uses the same discrete kernel with
//! clamp-to-edge, so the GPU and CPU paths agree within tolerance.

use std::cell::{Cell, RefCell};

use super::RenderNodeCpu;

/// Maximum number of 1D kernel taps (matches the shader's fixed loop bound and the
/// 16-slot uniform weight array).
const MAX_TAPS: usize = 15;

/// Computes a normalised 1D Gaussian kernel for `sigma`: `(tap_count, weights)`.
///
/// `sigma` is clamped to `[0.5, 20.0]`; the radius is `min(ceil(2σ), 7)` so the tap
/// count stays odd and `<= 15` (large sigma is truncated, matching the node's
/// fixed-size kernel). `weights` is zero-padded to 16 slots (the used taps come
/// first) so the GPU uniform can carry it directly.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]
fn gaussian_kernel(sigma: f32) -> (u32, [f32; 16]) {
    let sigma = sigma.clamp(0.5, 20.0);
    let radius = ((2.0 * sigma).ceil() as i32).clamp(1, (MAX_TAPS as i32 - 1) / 2);
    let tap_count = (2 * radius + 1) as usize;

    let mut weights = [0.0f32; 16];
    let mut sum = 0.0f32;
    for (i, slot) in weights.iter_mut().enumerate().take(tap_count) {
        let x = i as f32 - radius as f32;
        let w = (-(x * x) / (2.0 * sigma * sigma)).exp();
        *slot = w;
        sum += w;
    }
    for w in weights.iter_mut().take(tap_count) {
        *w /= sum;
    }
    (tap_count as u32, weights)
}

/// One directional (horizontal or vertical) pass of a separable blur over an f32
/// RGBA buffer, clamping sample coordinates to the edge. `radius = (tap_count-1)/2`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn blur_pass_cpu(
    src: &[f32],
    dst: &mut [f32],
    w: usize,
    h: usize,
    horizontal: bool,
    radius: i32,
    weights: &[f32; 16],
) {
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for i in 0..=(2 * radius) {
                let off = i - radius;
                let (sx, sy) = if horizontal {
                    ((x as i32 + off).clamp(0, w as i32 - 1), y as i32)
                } else {
                    (x as i32, (y as i32 + off).clamp(0, h as i32 - 1))
                };
                let p = (sy as usize * w + sx as usize) * 4;
                let weight = weights[i as usize];
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += src[p + c] * weight;
                }
            }
            let d = (y * w + x) * 4;
            dst[d..d + 4].copy_from_slice(&acc);
        }
    }
}

/// Blurs an 8-bit RGBA buffer in place with a separable Gaussian, returning the
/// blurred result as f32 (0..1) so a caller (sharpen, glow) can reuse it. `None`
/// when the buffer size does not match `w × h × 4`.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
pub(crate) fn separable_blur_f32(rgba: &[u8], w: u32, h: u32, sigma: f32) -> Option<Vec<f32>> {
    let (wu, hu) = (w as usize, h as usize);
    if wu == 0 || hu == 0 || rgba.len() != wu * hu * 4 {
        return None;
    }
    let (tap_count, weights) = gaussian_kernel(sigma);
    let radius = (tap_count / 2) as i32;
    let src: Vec<f32> = rgba.iter().map(|&b| f32::from(b) / 255.0).collect();
    let mut temp = vec![0.0f32; src.len()];
    blur_pass_cpu(&src, &mut temp, wu, hu, true, radius, &weights);
    let mut out = vec![0.0f32; src.len()];
    blur_pass_cpu(&temp, &mut out, wu, hu, false, radius, &weights);
    Some(out)
}

// GaussianBlurNode

/// Two-pass separable Gaussian blur.
pub struct GaussianBlurNode {
    /// Standard deviation in pixels. Effective range `[0.5, 20.0]` (values outside
    /// are clamped); a larger sigma is truncated to a 15-tap kernel.
    pub sigma: f32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<BlurPipeline>,
}

impl GaussianBlurNode {
    /// Creates a Gaussian blur node with the given standard deviation.
    #[must_use]
    pub fn new(sigma: f32) -> Self {
        Self {
            sigma,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for GaussianBlurNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        let Some(out) = separable_blur_f32(rgba, w, h, self.sigma) else {
            return;
        };
        for (b, &f) in rgba.iter_mut().zip(out.iter()) {
            *b = (f.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
}

#[cfg(feature = "wgpu")]
impl GaussianBlurNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &BlurPipeline {
        self.pipeline
            .get_or_init(|| create_blur_pipeline(ctx, self.sigma))
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for GaussianBlurNode {
    fn pass_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("GaussianBlurNode::process called with no inputs");
            return;
        };
        if outputs.len() < 2 {
            log::warn!("GaussianBlurNode::process needs 2 output targets");
            return;
        }
        let pd = self.get_or_create_pipeline(ctx);
        // Pass 0 (horizontal): source -> outputs[0].
        encode_blur_pass(ctx, pd, &pd.h_uniform_buf, input, outputs[0]);
        // Pass 1 (vertical): outputs[0] -> outputs[1] (final).
        encode_blur_pass(ctx, pd, &pd.v_uniform_buf, outputs[0], outputs[1]);
    }
}

// SharpenNode

/// Unsharp-mask sharpen: `orig + (orig - blurred) * strength`.
pub struct SharpenNode {
    /// Blur radius (as the Gaussian sigma) for the unsharp mask. Range `[0.5, 5.0]`.
    pub radius: f32,
    /// Sharpening strength (`0.0` = no-op). Typical range `[0.0, 3.0]`.
    pub strength: f32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<SharpenPipeline>,
}

impl SharpenNode {
    /// Creates an unsharp-mask sharpen node.
    #[must_use]
    pub fn new(radius: f32, strength: f32) -> Self {
        Self {
            radius,
            strength,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for SharpenNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        let Some(blur) = separable_blur_f32(rgba, w, h, self.radius) else {
            return;
        };
        // Sharpen the RGB channels; leave alpha unchanged.
        for (px, blurred) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(blur.as_chunks::<4>().0)
        {
            for c in 0..3 {
                let orig = f32::from(px[c]) / 255.0;
                let detail = orig - blurred[c];
                let sharpened = (orig + detail * self.strength).clamp(0.0, 1.0);
                px[c] = (sharpened * 255.0 + 0.5) as u8;
            }
        }
    }
}

#[cfg(feature = "wgpu")]
impl SharpenNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &SharpenPipeline {
        self.pipeline.get_or_init(|| SharpenPipeline {
            blur: create_blur_pipeline(ctx, self.radius),
            combine: create_combine_pipeline(ctx, self.strength),
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for SharpenNode {
    fn pass_count(&self) -> usize {
        3
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("SharpenNode::process called with no inputs");
            return;
        };
        if outputs.len() < 3 {
            log::warn!("SharpenNode::process needs 3 output targets");
            return;
        }
        let pd = self.get_or_create_pipeline(ctx);
        // Passes 0-1: separable Gaussian blur of the source into outputs[1].
        encode_blur_pass(ctx, &pd.blur, &pd.blur.h_uniform_buf, input, outputs[0]);
        encode_blur_pass(
            ctx,
            &pd.blur,
            &pd.blur.v_uniform_buf,
            outputs[0],
            outputs[1],
        );
        // Pass 2: combine original (input) with the blur (outputs[1]) into outputs[2].
        encode_combine_pass(ctx, &pd.combine, input, outputs[1], outputs[2]);
    }
}

// GPU pipeline construction

#[cfg(feature = "wgpu")]
struct BlurPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    h_uniform_buf: wgpu::Buffer,
    v_uniform_buf: wgpu::Buffer,
}

#[cfg(feature = "wgpu")]
struct CombinePipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
}

#[cfg(feature = "wgpu")]
struct SharpenPipeline {
    blur: BlurPipeline,
    combine: CombinePipeline,
}

#[cfg(feature = "wgpu")]
fn create_blur_pipeline(ctx: &crate::context::RenderContext, sigma: f32) -> BlurPipeline {
    let device = &ctx.device;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("GaussianBlur shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/gaussian_blur.wgsl").into()),
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("GaussianBlur BGL"),
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

    let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "GaussianBlur");

    let (tap_count, weights) = gaussian_kernel(sigma);
    let h_uniform_buf = create_uniform(device, "GaussianBlur H uniforms", 80);
    let v_uniform_buf = create_uniform(device, "GaussianBlur V uniforms", 80);
    ctx.queue.write_buffer(
        &h_uniform_buf,
        0,
        &pack_blur_uniforms([1.0, 0.0], tap_count, &weights),
    );
    ctx.queue.write_buffer(
        &v_uniform_buf,
        0,
        &pack_blur_uniforms([0.0, 1.0], tap_count, &weights),
    );

    BlurPipeline {
        render_pipeline,
        bind_group_layout: bgl,
        h_uniform_buf,
        v_uniform_buf,
    }
}

#[cfg(feature = "wgpu")]
fn create_combine_pipeline(ctx: &crate::context::RenderContext, strength: f32) -> CombinePipeline {
    let device = &ctx.device;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Sharpen combine shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sharpen.wgsl").into()),
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Sharpen combine BGL"),
        entries: &[
            texture_entry(0),
            texture_entry(1),
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

    let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "Sharpen combine");

    let uniform_buf = create_uniform(device, "Sharpen uniforms", 16);
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&strength.to_le_bytes());
    ctx.queue.write_buffer(&uniform_buf, 0, &bytes);

    CombinePipeline {
        render_pipeline,
        bind_group_layout: bgl,
        uniform_buf,
    }
}

#[cfg(feature = "wgpu")]
pub(crate) fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

#[cfg(feature = "wgpu")]
pub(crate) fn create_uniform(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(feature = "wgpu")]
pub(crate) fn fullscreen_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    bgl: &wgpu::BindGroupLayout,
    label: &str,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
    })
}

/// Encodes one separable-blur pass reading `input` and writing `output`, using the
/// direction baked into `uniform_buf`.
#[cfg(feature = "wgpu")]
fn encode_blur_pass(
    ctx: &crate::context::RenderContext,
    pd: &BlurPipeline,
    uniform_buf: &wgpu::Buffer,
    input: &wgpu::Texture,
    output: &wgpu::Texture,
) {
    let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
    let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("GaussianBlur BG"),
        layout: &pd.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&input_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform_buf.as_entire_binding(),
            },
        ],
    });
    run_fullscreen(
        ctx,
        &pd.render_pipeline,
        &bind_group,
        &output_view,
        "GaussianBlur pass",
    );
}

/// Encodes the sharpen combine pass reading `orig` + `blur` and writing `output`.
#[cfg(feature = "wgpu")]
fn encode_combine_pass(
    ctx: &crate::context::RenderContext,
    pd: &CombinePipeline,
    orig: &wgpu::Texture,
    blur: &wgpu::Texture,
    output: &wgpu::Texture,
) {
    let orig_view = orig.create_view(&wgpu::TextureViewDescriptor::default());
    let blur_view = blur.create_view(&wgpu::TextureViewDescriptor::default());
    let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Sharpen combine BG"),
        layout: &pd.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&orig_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&blur_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pd.uniform_buf.as_entire_binding(),
            },
        ],
    });
    run_fullscreen(
        ctx,
        &pd.render_pipeline,
        &bind_group,
        &output_view,
        "Sharpen combine pass",
    );
}

#[cfg(feature = "wgpu")]
pub(crate) fn run_fullscreen(
    ctx: &crate::context::RenderContext,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    output_view: &wgpu::TextureView,
    label: &str,
) {
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
    ctx.queue.submit(std::iter::once(encoder.finish()));
}

/// Packs `BlurUniforms` (direction, `tap_count`, 16 weights) into the 80-byte `std140`
/// layout the shader declares: `vec2` + `u32` + pad + `array<vec4<f32>, 4>`.
#[cfg(feature = "wgpu")]
fn pack_blur_uniforms(direction: [f32; 2], tap_count: u32, weights: &[f32; 16]) -> [u8; 80] {
    let mut b = [0u8; 80];
    b[0..4].copy_from_slice(&direction[0].to_le_bytes());
    b[4..8].copy_from_slice(&direction[1].to_le_bytes());
    b[8..12].copy_from_slice(&tap_count.to_le_bytes());
    // b[12..16] is padding (0).
    for (i, w) in weights.iter().enumerate() {
        let off = 16 + i * 4;
        b[off..off + 4].copy_from_slice(&w.to_le_bytes());
    }
    b
}

// MotionBlurNode

/// GPU-native motion blur via exponential-decay accumulation.
///
/// Each frame the node blends the current frame with a persistent accumulation of
/// the previous output: `out = mix(current, prev, prev_weight)`, then keeps `out`
/// as the next frame's `prev`. `prev_weight` grows with `shutter_angle`
/// (`0` = no blur, `180` = standard film blur) and `sub_frames` (2–8; more = more
/// persistence, i.e. a smoother/longer trail). The node is **stateful**: the trail
/// builds up only across successive `process` / `process_cpu` calls on the *same*
/// node instance (a fresh node per frame never accumulates).
pub struct MotionBlurNode {
    /// Shutter angle in degrees `[0, 360]`. 0 = no blur, 180 = standard film blur.
    ///
    /// A `Cell` so an animated shutter can be applied to the live node
    /// ([`NodeParam::MotionBlurShutter`](crate::NodeParam::MotionBlurShutter))
    /// instead of rebuilding it, which would
    /// discard the trail. `Cell` keeps the node `Send`, which `RenderNodeCpu`
    /// requires; a `Sync` container would be a stronger bound than anything here
    /// needs.
    shutter_angle: Cell<f32>,
    /// Accumulated sub-frame count (clamped to `2..=8`); higher = smoother trail.
    pub sub_frames: u8,
    /// Previous output with its `(width, height)`, retained across frames for the
    /// CPU path. The dimensions reset the accumulation on a size change, matching
    /// the GPU path (a same-byte-length reshape would otherwise blend garbage).
    cpu_prev: RefCell<Option<(Vec<u8>, u32, u32)>>,
    #[cfg(feature = "wgpu")]
    gpu: RefCell<Option<MotionBlurGpu>>,
}

impl MotionBlurNode {
    /// Creates a motion-blur node.
    #[must_use]
    pub fn new(shutter_angle: f32, sub_frames: u8) -> Self {
        Self {
            shutter_angle: Cell::new(shutter_angle),
            sub_frames,
            cpu_prev: RefCell::new(None),
            #[cfg(feature = "wgpu")]
            gpu: RefCell::new(None),
        }
    }

    /// The shutter angle currently in effect, in degrees.
    #[must_use]
    pub fn shutter_angle(&self) -> f32 {
        self.shutter_angle.get()
    }

    /// The weight applied to the accumulated `prev` frame. `shutter = 0` yields
    /// `0` (no blur) for any `sub_frames`; `sub_frames` (clamped `2..=8`) scales the
    /// retention from `0.5x` (2) to `1.0x` (8) of the shutter fraction.
    fn prev_weight(&self) -> f32 {
        let alpha = (self.shutter_angle.get() / 360.0).clamp(0.0, 1.0);
        let sub = self.sub_frames.clamp(2, 8);
        let g = 0.5 + 0.5 * (f32::from(sub - 2) / 6.0);
        (alpha * g).clamp(0.0, 1.0)
    }
}

impl RenderNodeCpu for MotionBlurNode {
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        let mut prev = self.cpu_prev.borrow_mut();
        match prev.as_mut() {
            Some((p, pw, ph)) if *pw == w && *ph == h && p.len() == rgba.len() => {
                let weight = self.prev_weight();
                for (cur, prv) in rgba.iter_mut().zip(p.iter()) {
                    *cur = lerp_u8(f32::from(*cur), f32::from(*prv), weight);
                }
                // Reuse the retained buffer's allocation for the new output.
                p.copy_from_slice(rgba);
            }
            // First frame (or a size change): no blur; seed the accumulation.
            _ => *prev = Some((rgba.to_vec(), w, h)),
        }
    }
}

/// Linear interpolation `a + (b - a) * t`, rounded to the nearest byte.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lerp_u8(a: f32, b: f32, t: f32) -> u8 {
    (a + (b - a) * t + 0.5).clamp(0.0, 255.0) as u8
}

#[cfg(feature = "wgpu")]
struct MotionBlurGpu {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    prev: wgpu::Texture,
    dims: (u32, u32),
    initialized: bool,
}

#[cfg(feature = "wgpu")]
fn build_motion_blur_gpu(device: &wgpu::Device, w: u32, h: u32) -> MotionBlurGpu {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("MotionBlur shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/motion_blur.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("MotionBlur BGL"),
        entries: &[
            texture_entry(0),
            texture_entry(1),
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
    let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "MotionBlur");
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("MotionBlur uniforms"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let prev = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("MotionBlur prev"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    MotionBlurGpu {
        render_pipeline,
        bind_group_layout: bgl,
        uniform_buf,
        prev,
        dims: (w, h),
        initialized: false,
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for MotionBlurNode {
    /// Takes [`NodeParam::MotionBlurShutter`](super::NodeParam::MotionBlurShutter),
    /// so an animated shutter reaches the live node and the accumulated trail
    /// survives the change.
    fn set_param(&self, param: super::NodeParam) -> bool {
        match param {
            super::NodeParam::MotionBlurShutter(deg) => {
                self.shutter_angle.set(deg);
                true
            }
        }
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(current) = inputs.first() else {
            log::warn!("MotionBlurNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("MotionBlurNode::process called with no outputs");
            return;
        };
        let (w, h) = (current.width(), current.height());

        let mut state = self.gpu.borrow_mut();
        if state.as_ref().is_none_or(|s| s.dims != (w, h)) {
            *state = Some(build_motion_blur_gpu(&ctx.device, w, h));
        }
        let Some(st) = state.as_mut() else {
            return; // unreachable: set to `Some` just above
        };

        // The first frame has no accumulated history, so render the current frame
        // unblended (weight 0) and seed `prev` from the output below.
        let weight = if st.initialized {
            self.prev_weight()
        } else {
            0.0
        };
        let mut uniform = [0u8; 16];
        uniform[0..4].copy_from_slice(&weight.to_le_bytes());
        ctx.queue.write_buffer(&st.uniform_buf, 0, &uniform);

        let cur_view = current.create_view(&wgpu::TextureViewDescriptor::default());
        let prev_view = st.prev.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("MotionBlur BG"),
            layout: &st.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cur_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&prev_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: st.uniform_buf.as_entire_binding(),
                },
            ],
        });
        run_fullscreen(
            ctx,
            &st.render_pipeline,
            &bind_group,
            &out_view,
            "MotionBlur pass",
        );

        // Copy this frame's output into `prev` for the next call.
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("MotionBlur accumulate"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &st.prev,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit(std::iter::once(encoder.finish()));
        st.initialized = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w × h` RGBA frame with a single white opaque pixel at `(cx, cy)` on an
    /// opaque black background (the impulse used to observe the blur kernel).
    fn impulse(w: usize, h: usize, cx: usize, cy: usize) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for px in v.as_chunks_mut::<4>().0 {
            px[3] = 255; // opaque
        }
        let p = (cy * w + cx) * 4;
        v[p] = 255;
        v[p + 1] = 255;
        v[p + 2] = 255;
        v[3 + p] = 255;
        v
    }

    #[test]
    fn gaussian_kernel_should_be_normalised_and_symmetric() {
        let (tap, weights) = gaussian_kernel(2.0);
        assert!(tap % 2 == 1, "tap count must be odd; got {tap}");
        assert!(
            tap as usize <= MAX_TAPS,
            "tap count must be <= 15; got {tap}"
        );
        let sum: f32 = weights.iter().take(tap as usize).sum();
        assert!((sum - 1.0).abs() < 1e-5, "weights must sum to 1; got {sum}");
        let r = (tap / 2) as usize;
        for i in 0..r {
            assert!(
                (weights[r - 1 - i] - weights[r + 1 + i]).abs() < 1e-6,
                "kernel must be symmetric around the centre tap"
            );
        }
    }

    #[test]
    fn gaussian_blur_cpu_impulse_should_spread_and_preserve_energy() {
        let (w, h) = (9usize, 9usize);
        let frame = impulse(w, h, 4, 4);
        let mut blurred = frame.clone();
        GaussianBlurNode::new(1.5).process_cpu(&mut blurred, w as u32, h as u32);

        let centre = (4 * w + 4) * 4;
        assert!(
            blurred[centre] < 255,
            "the impulse centre must lose energy to its neighbours; got {}",
            blurred[centre]
        );
        let neighbour = (4 * w + 5) * 4;
        assert!(
            blurred[neighbour] > 0,
            "an adjacent pixel must gain energy from the impulse; got {}",
            blurred[neighbour]
        );
        // Energy (sum of the R channel) is preserved by a normalised kernel with
        // clamp-to-edge, since the impulse sits well inside the frame.
        let sum_before: u32 = frame.iter().step_by(4).map(|&b| u32::from(b)).sum();
        let sum_after: u32 = blurred.iter().step_by(4).map(|&b| u32::from(b)).sum();
        assert!(
            (i64::from(sum_after) - i64::from(sum_before)).abs() <= 8,
            "a normalised blur must roughly preserve total energy; before={sum_before} after={sum_after}"
        );
    }

    #[test]
    fn gaussian_blur_sigma_zero_should_clamp_and_not_panic() {
        let (w, h) = (4u32, 4u32);
        let mut frame = impulse(4, 4, 1, 1);
        // sigma 0.0 is clamped to 0.5 inside the kernel; must run without panicking.
        GaussianBlurNode::new(0.0).process_cpu(&mut frame, w, h);
    }

    #[test]
    fn sharpen_strength_zero_should_be_a_noop() {
        let (w, h) = (8u32, 8u32);
        // A horizontal gradient.
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for (i, px) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = (i as u32 % w) as u8;
            *px = [x * 30, x * 30, x * 30, 255];
        }
        let original = frame.clone();
        SharpenNode::new(1.0, 0.0).process_cpu(&mut frame, w, h);
        for (a, b) in frame.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "strength 0 must be a no-op (within rounding); got {a} vs {b}"
            );
        }
    }

    #[test]
    fn sharpen_cpu_should_increase_edge_contrast() {
        // Left half dark (100), right half light (150): a vertical edge at x=4.
        let (w, h) = (8usize, 4usize);
        let mut frame = vec![0u8; w * h * 4];
        for (i, px) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = i % w;
            let v = if x < 4 { 100u8 } else { 150u8 };
            *px = [v, v, v, 255];
        }
        let original = frame.clone();
        SharpenNode::new(1.0, 1.5).process_cpu(&mut frame, w as u32, h as u32);

        // The pixels straddling the edge must move further apart (overshoot):
        // the dark side just left of the edge gets darker, the light side lighter.
        let dark = (0 * w + 3) * 4; // x=3, left of the edge
        let light = (0 * w + 4) * 4; // x=4, right of the edge
        let before = i32::from(original[light]) - i32::from(original[dark]);
        let after = i32::from(frame[light]) - i32::from(frame[dark]);
        assert!(
            after > before,
            "sharpen must widen the edge step; before={before} after={after}"
        );
    }

    #[test]
    fn motion_blur_node_should_be_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MotionBlurNode>();
    }

    #[test]
    fn motion_blur_first_frame_should_be_unchanged() {
        let node = MotionBlurNode::new(180.0, 4);
        let original = vec![200u8, 150, 100, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "the first frame has no history, so no blur");
    }

    #[test]
    fn motion_blur_shutter_zero_should_be_no_blur() {
        let node = MotionBlurNode::new(0.0, 4);
        let mut white = vec![255u8, 255, 255, 255];
        node.process_cpu(&mut white, 1, 1); // seed prev = white
        let mut black = vec![0u8, 0, 0, 255];
        node.process_cpu(&mut black, 1, 1);
        assert_eq!(
            &black[0..3],
            &[0, 0, 0],
            "shutter=0 keeps only the current frame (no blur)"
        );
    }

    #[test]
    fn motion_blur_should_leave_a_trail() {
        let node = MotionBlurNode::new(180.0, 4);
        let mut white = vec![255u8, 255, 255, 255];
        node.process_cpu(&mut white, 1, 1); // seed prev = white
        let mut black = vec![0u8, 0, 0, 255];
        node.process_cpu(&mut black, 1, 1);
        assert!(
            black[0] > 0,
            "the white frame must leave a fading trail on the black frame; got {}",
            black[0]
        );
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn set_param_should_change_the_shutter_without_resetting_the_trail() {
        // The whole reason the parameter travels to the live node: rebuilding it to
        // change the shutter would drop `cpu_prev`, and the trail with it.
        use crate::nodes::{NodeParam, RenderNode};
        let node = MotionBlurNode::new(180.0, 4);
        let mut white = vec![255u8, 255, 255, 255];
        node.process_cpu(&mut white, 1, 1); // seed prev = white

        assert!(node.set_param(NodeParam::MotionBlurShutter(360.0)));
        assert!((node.shutter_angle() - 360.0).abs() < 1e-6);

        let mut black = vec![0u8, 0, 0, 255];
        node.process_cpu(&mut black, 1, 1);
        assert!(
            black[0] > 0,
            "the seeded trail must survive the parameter change; got {}",
            black[0]
        );
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn set_param_should_be_declined_by_a_node_that_does_not_take_it() {
        // The default is `false`, which is how a caller tells that nothing was
        // applied and the graph has to be rebuilt instead.
        use crate::nodes::{NodeParam, RenderNode};
        let node = GaussianBlurNode::new(2.0);
        assert!(!node.set_param(NodeParam::MotionBlurShutter(90.0)));
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn a_changed_shutter_should_change_the_blend_weight() {
        // Non-vacuity for the test above: the parameter has to reach the maths, not
        // just the field.
        use crate::nodes::{NodeParam, RenderNode};
        let node = MotionBlurNode::new(360.0, 8);
        let mut white = vec![255u8, 255, 255, 255];
        node.process_cpu(&mut white, 1, 1);
        let mut black_full = vec![0u8, 0, 0, 255];
        node.process_cpu(&mut black_full, 1, 1);

        let node = MotionBlurNode::new(360.0, 8);
        let mut white = vec![255u8, 255, 255, 255];
        node.process_cpu(&mut white, 1, 1);
        assert!(node.set_param(NodeParam::MotionBlurShutter(0.0)));
        let mut black_none = vec![0u8, 0, 0, 255];
        node.process_cpu(&mut black_none, 1, 1);

        assert!(
            black_full[0] > black_none[0],
            "a shutter of 0 must retain less than one of 360: {} vs {}",
            black_full[0],
            black_none[0]
        );
        assert_eq!(black_none[0], 0, "a zero shutter is no blur at all");
    }

    #[test]
    fn motion_blur_sub_frames_out_of_range_should_clamp() {
        // sub_frames below 2 clamps to 2, above 8 clamps to 8, so their weights
        // match the boundary values.
        let below = MotionBlurNode::new(180.0, 1).prev_weight();
        let at_two = MotionBlurNode::new(180.0, 2).prev_weight();
        let above = MotionBlurNode::new(180.0, 20).prev_weight();
        let at_eight = MotionBlurNode::new(180.0, 8).prev_weight();
        assert!((below - at_two).abs() < 1e-6, "sub_frames<2 clamps to 2");
        assert!((above - at_eight).abs() < 1e-6, "sub_frames>8 clamps to 8");
        assert!(at_two < at_eight, "more sub_frames retains more of prev");
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

    fn impulse(w: usize, h: usize, cx: usize, cy: usize) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for px in v.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        let p = (cy * w + cx) * 4;
        v[p] = 255;
        v[p + 1] = 255;
        v[p + 2] = 255;
        v[3 + p] = 255;
        v
    }

    /// RMSE over the RGB channels of two 8-bit RGBA buffers, normalised to `[0, 1]`.
    fn rmse_rgb(a: &[u8], b: &[u8]) -> f64 {
        let mut sum = 0.0f64;
        let mut n = 0u64;
        for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            for c in 0..3 {
                let d = (f64::from(pa[c]) - f64::from(pb[c])) / 255.0;
                sum += d * d;
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { (sum / n as f64).sqrt() }
    }

    #[test]
    fn gaussian_blur_gpu_should_match_cpu_reference_within_rmse() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (9u32, 9u32);
        let frame = impulse(9, 9, 4, 4);

        let node = GaussianBlurNode::new(3.0);
        let mut cpu_ref = frame.clone();
        node.process_cpu(&mut cpu_ref, w, h);

        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(GaussianBlurNode::new(3.0))
            .process_gpu(&frame, w, h)
            .expect("gpu blur");

        assert_eq!(gpu.len(), cpu_ref.len());
        let rmse = rmse_rgb(&gpu, &cpu_ref);
        assert!(
            rmse < 0.005,
            "GPU blur must match the CPU reference within RMSE 0.005; got {rmse}"
        );
    }

    #[test]
    fn sharpen_gpu_should_increase_edge_contrast() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (8u32, 4u32);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for (i, px) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = i as u32 % w;
            let v = if x < 4 { 100u8 } else { 150u8 };
            *px = [v, v, v, 255];
        }

        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(SharpenNode::new(1.0, 1.5))
            .process_gpu(&frame, w, h)
            .expect("gpu sharpen");

        let dark = 3 * 4; // x=3, y=0
        let light = 4 * 4; // x=4, y=0
        let before = i32::from(frame[light]) - i32::from(frame[dark]);
        let after = i32::from(gpu[light]) - i32::from(gpu[dark]);
        assert!(
            after > before,
            "GPU sharpen must widen the edge step; before={before} after={after}"
        );
    }

    #[test]
    fn motion_blur_gpu_should_leave_a_trail() {
        let Some(ctx) = ctx() else {
            return;
        };
        // Accumulate across two process_gpu calls on the SAME graph instance.
        let graph = RenderGraph::new(Arc::clone(&ctx)).push(MotionBlurNode::new(180.0, 4));
        let white = vec![255u8, 255, 255, 255];
        let black = vec![0u8, 0, 0, 255];
        graph
            .process_gpu(&white, 1, 1)
            .expect("gpu motion blur frame 1");
        let out = graph
            .process_gpu(&black, 1, 1)
            .expect("gpu motion blur frame 2");
        assert!(
            out[0] > 0,
            "the white frame must leave a trail on the black frame; got {}",
            out[0]
        );
    }

    #[test]
    fn motion_blur_gpu_first_frame_should_be_unchanged() {
        let Some(ctx) = ctx() else {
            return;
        };
        // The first GPU call has no history, so weight is forced to 0: output == input.
        let frame = vec![200u8, 150, 100, 255];
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(MotionBlurNode::new(180.0, 4))
            .process_gpu(&frame, 1, 1)
            .expect("gpu motion blur frame 1");
        for i in 0..4 {
            assert!(
                (i32::from(out[i]) - i32::from(frame[i])).abs() <= 1,
                "the first GPU frame must be unblended at {i}"
            );
        }
    }

    #[test]
    fn motion_blur_gpu_shutter_zero_should_be_no_blur() {
        let Some(ctx) = ctx() else {
            return;
        };
        let graph = RenderGraph::new(Arc::clone(&ctx)).push(MotionBlurNode::new(0.0, 4));
        let white = vec![255u8, 255, 255, 255];
        let black = vec![0u8, 0, 0, 255];
        graph
            .process_gpu(&white, 1, 1)
            .expect("gpu motion blur frame 1");
        let out = graph
            .process_gpu(&black, 1, 1)
            .expect("gpu motion blur frame 2");
        for i in 0..3 {
            assert!(out[i] <= 2, "shutter=0 must keep the current frame at {i}");
        }
    }
}
