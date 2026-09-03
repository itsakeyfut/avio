//! Two-clip transition nodes: a directional wipe, a linear fade, a per-pixel dissolve,
//! and a two-phase dip to a solid colour. Like [`CrossfadeNode`](super::crossfade::CrossfadeNode), the second clip
//! (B) is carried as RGBA bytes in the node and uploaded to a GPU texture at render
//! time: a single render graph has one source, so B cannot arrive as a second GPU
//! input. Clip A is the node's input (`inputs[0]` / the `process_cpu` argument).

use super::RenderNodeCpu;

/// Linear interpolation `a + (b - a) * t`, rounded to the nearest byte.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lerp_u8(a: f32, b: f32, t: f32) -> u8 {
    (a + (b - a) * t + 0.5).clamp(0.0, 255.0) as u8
}

/// `FFmpeg`'s fixed dip phase (`vf_xfade.c`, `FADEBLACK_TRANSITION`): the fraction of
/// the transition spent reaching the solid colour at each end.
///
/// It is what makes the dip *not* a linear ramp -- the solid colour is reached about a
/// fifth of the way in and held through the middle, where a linear dip would only touch
/// it at the midpoint. The linear version this replaced diverged from a real export by a
/// mean of 78 (#1732).
const DIP_PHASE: f32 = 0.2;

/// `smoothstep(e0, e1, x)`; callers guarantee `e1 > e0`.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// WipeTransitionNode

/// Directional wipe that reveals clip B behind a moving edge.
///
/// `progress = 0` outputs clip A, `progress = 1` outputs clip B, and an edge sweeps
/// across the frame between them. `angle` (radians) points along the axis clip B
/// **grows from**, because the mask fills where the projection
/// exceeds the sweeping threshold: at `angle = 0` clip B enters from the **right** and
/// the edge travels right-to-left; at `angle = π/2` it enters from the **bottom**.
///
/// That is the opposite of how `FFmpeg` names its `xfade` wipes — `wiperight` sweeps its
/// edge rightward, so clip B enters from the left and maps to `angle = π` (RK-020).
///
/// `softness` feathers the edge (normalised units).
pub struct WipeTransitionNode {
    /// Transition progress `[0, 1]`: 0 = clip A, 1 = clip B.
    pub progress: f32,
    /// Edge feather half-width in normalised units (0 = hard edge).
    pub softness: f32,
    /// Axis clip B grows from, in radians: `0` = from the right, `π/2` = from the
    /// bottom. See the type docs — this is the opposite of the `FFmpeg` wipe names.
    pub angle: f32,
    /// Clip B as RGBA bytes (`to_width × to_height × 4`).
    pub to_rgba: Vec<u8>,
    /// Width of `to_rgba`.
    pub to_width: u32,
    /// Height of `to_rgba`.
    pub to_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<TransitionPipeline>,
}

impl WipeTransitionNode {
    /// Creates a wipe from clip A (the node input) to clip B (`to_rgba`).
    #[must_use]
    pub fn new(
        progress: f32,
        softness: f32,
        angle: f32,
        to_rgba: Vec<u8>,
        to_width: u32,
        to_height: u32,
    ) -> Self {
        Self {
            progress,
            softness,
            angle,
            to_rgba,
            to_width,
            to_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }

    /// The B-weight (`mask`) for the pixel at `(x, y)` on a `w` x `h` grid. Shared by
    /// the CPU and GPU paths (`wipe.wgsl` uses the identical formula) so they agree.
    ///
    /// Clip B occupies the side where `proj` exceeds `center`, and `center` sweeps down
    /// as `progress` rises -- so B fills in from the **high** end of the projected axis.
    ///
    /// A hard edge (`softness == 0`) along one of the four axes takes an exact integer
    /// rule instead, because those are the four `FFmpeg` wipes and the export has to be
    /// able to reproduce them. `FFmpeg` compares the pixel index against an integer edge
    /// `z` (`vf_xfade.c`, `WIPE*_TRANSITION`), which puts the seam half a pixel away from
    /// where a normalised threshold puts it -- one column, but a column that a per-pixel
    /// comparison sees (#1732). The rule is deliberately asymmetric at the endpoints,
    /// matching `FFmpeg`: the `-x` axis already shows one column of B at progress 0.
    ///
    /// A feathered or off-axis wipe has no `FFmpeg` counterpart to match and keeps the
    /// smoothstep.
    fn mask_at(&self, x: u32, y: u32, w: u32, h: u32) -> f32 {
        let (ax, ay) = (self.angle.cos(), self.angle.sin());
        if self.softness <= 0.0 {
            const AXIS: f32 = 0.999;
            #[allow(clippy::cast_precision_loss)]
            let (wf, hf) = (w as f32, h as f32);
            // `z` truncates exactly as C's `const int z = width * progress` does.
            #[allow(clippy::cast_possible_truncation)]
            let edge = |extent: f32, at: f32| (extent * at) as i64;
            if ax > AXIS {
                return f32::from(i64::from(x) > edge(wf, 1.0 - self.progress));
            }
            if ax < -AXIS {
                return f32::from(i64::from(x) <= edge(wf, self.progress));
            }
            if ay > AXIS {
                return f32::from(i64::from(y) > edge(hf, 1.0 - self.progress));
            }
            if ay < -AXIS {
                return f32::from(i64::from(y) <= edge(hf, self.progress));
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let (uv_x, uv_y) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        let reach = f32::midpoint(ax.abs(), ay.abs());
        // Floor the half-width so a zero softness is a near-hard, division-safe edge.
        let hw = self.softness.max(1e-3);
        // Sweep the threshold from beyond the far corner (all A at progress 0) to
        // beyond the near corner (all B at progress 1), so the endpoints are exact.
        let center = (0.5 + reach + hw) + ((0.5 - reach - hw) - (0.5 + reach + hw)) * self.progress;
        let proj = (uv_x - 0.5) * ax + (uv_y - 0.5) * ay + 0.5;
        smoothstep(center - hw, center + hw, proj)
    }
}

impl RenderNodeCpu for WipeTransitionNode {
    #[allow(clippy::cast_precision_loss)]
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        if self.to_rgba.len() != rgba.len() {
            log::warn!(
                "WipeTransitionNode::process_cpu skipped: size mismatch a={} b={}",
                rgba.len(),
                self.to_rgba.len()
            );
            return;
        }
        for y in 0..h {
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                let mask = self.mask_at(x, y, w, h);
                for c in 0..4 {
                    let a = f32::from(rgba[idx + c]);
                    let b = f32::from(self.to_rgba[idx + c]);
                    rgba[idx + c] = lerp_u8(a, b, mask);
                }
            }
        }
    }
}

// FadeTransitionNode

/// Linear cross-blend: clip A mixed into clip B by `progress`.
///
/// This is `FFmpeg`'s `xfade=transition=fade`, not its `dissolve` — `dissolve` reveals
/// clip B one pixel at a time and never produces a mixed value (see
/// [`DissolveTransitionNode`]).
///
/// `progress = 0` outputs clip A, `progress = 1` outputs clip B, and every value
/// between is the per-channel mix of the two (alpha included, as the sibling
/// transitions do).
///
/// The GPU path reuses `crossfade.wgsl` rather than carrying a second copy of the same
/// `mix`: that shader's bindings already match the layout this module's shared
/// `build_pipeline` sets up, and its single-`f32` uniform is this node's `progress`. It
/// is therefore the same operation as [`CrossfadeNode`](super::crossfade::CrossfadeNode),
/// exposed with the `progress` / `to_rgba` shape the rest of this module uses so a
/// transition set can be mapped uniformly.
///
/// Like its siblings this node renders to an `Rgba8Unorm` target (the shared
/// `build_pipeline` hard-codes that format), so it does not run in an `Rgba16Float`
/// graph.
pub struct FadeTransitionNode {
    /// Transition progress `[0, 1]`: 0 = clip A, 1 = clip B.
    pub progress: f32,
    /// Clip B as RGBA bytes (`to_width × to_height × 4`).
    pub to_rgba: Vec<u8>,
    /// Width of `to_rgba`.
    pub to_width: u32,
    /// Height of `to_rgba`.
    pub to_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<TransitionPipeline>,
}

impl FadeTransitionNode {
    /// Creates a fade from clip A (the node input) to clip B (`to_rgba`).
    #[must_use]
    pub fn new(progress: f32, to_rgba: Vec<u8>, to_width: u32, to_height: u32) -> Self {
        Self {
            progress,
            to_rgba,
            to_width,
            to_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for FadeTransitionNode {
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.to_rgba.len() != rgba.len() {
            log::warn!(
                "FadeTransitionNode::process_cpu skipped: size mismatch a={} b={}",
                rgba.len(),
                self.to_rgba.len()
            );
            return;
        }
        for (a, b) in rgba.iter_mut().zip(self.to_rgba.iter()) {
            *a = lerp_u8(f32::from(*a), f32::from(*b), self.progress);
        }
    }
}

// DissolveTransitionNode

/// Per-pixel dissolve: clip B shows through wherever the supplied `mask` is set.
///
/// This is `FFmpeg`'s `xfade=transition=dissolve`. Unlike [`FadeTransitionNode`] every
/// output pixel is *fully* clip A or *fully* clip B, never a mixture of them — the node
/// selects, it does not blend.
///
/// It takes no progress of its own: which pixels have turned over is entirely the mask's
/// decision, and `ff_filter::dissolve_mask` is what turns a progress into one.
///
/// Like its siblings this node renders to an `Rgba8Unorm` target (the shared
/// `build_pipeline` hard-codes that format), so it does not run in an `Rgba16Float`
/// graph.
pub struct DissolveTransitionNode {
    /// Per-pixel selection as an RGBA mask: `255` shows clip B, `0` shows clip A.
    ///
    /// Supplied rather than computed. `FFmpeg`'s dissolve keys off
    /// `fract(sinf(x*12.9898 + y*78.233) * 43758.545)`, whose argument reaches ~110 000
    /// at 1080p -- past where `f32` holds it steadily, so the value is not reproducible
    /// across implementations and a `WGSL` copy would reveal a different set of pixels
    /// than the CPU reference. `ff_filter::dissolve_mask` builds this once and both
    /// paths read it (#1732).
    pub mask: Vec<u8>,
    /// Clip B as RGBA bytes (`to_width × to_height × 4`).
    pub to_rgba: Vec<u8>,
    /// Width of `to_rgba`.
    pub to_width: u32,
    /// Height of `to_rgba`.
    pub to_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<TransitionPipeline>,
}

impl DissolveTransitionNode {
    /// Creates a dissolve from clip A (the node input) to clip B (`to_rgba`), revealing B
    /// wherever `mask` is set. Build `mask` with `ff_filter::dissolve_mask`.
    #[must_use]
    pub fn new(mask: Vec<u8>, to_rgba: Vec<u8>, to_width: u32, to_height: u32) -> Self {
        Self {
            mask,
            to_rgba,
            to_width,
            to_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for DissolveTransitionNode {
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.to_rgba.len() != rgba.len() || self.mask.len() != rgba.len() {
            log::warn!(
                "DissolveTransitionNode::process_cpu skipped: size mismatch a={} b={} mask={}",
                rgba.len(),
                self.to_rgba.len(),
                self.mask.len()
            );
            return;
        }
        for ((px, b), m) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.to_rgba.as_chunks::<4>().0)
            .zip(self.mask.as_chunks::<4>().0)
        {
            if m[0] >= 128 {
                *px = *b;
            }
        }
    }
}

// DipToColorNode

/// Two-phase transition: clip A fades to a solid `color`, then the colour fades to
/// clip B. `progress = 0.5` is the fully solid dip (a fade-to-black/white/brand dip).
pub struct DipToColorNode {
    /// Transition progress `[0, 1]`: 0 = clip A, 1 = clip B, with `color` solid across
    /// the middle (see this module's `DIP_PHASE`).
    pub progress: f32,
    /// Dip colour in RGB, normally `[0, 1]`.
    ///
    /// Values **outside** that range are meaningful and are not clamped until the final
    /// write: reproducing `FFmpeg`'s `fadeblack` / `fadewhite` needs the dip endpoint to
    /// be the luma level 0 / 255 expanded out of limited range, which lands just outside
    /// `[0, 1]`. `avio`'s `map_transition` is what supplies those values.
    pub color: [f32; 3],
    /// Clip B as RGBA bytes (`to_width × to_height × 4`).
    pub to_rgba: Vec<u8>,
    /// Width of `to_rgba`.
    pub to_width: u32,
    /// Height of `to_rgba`.
    pub to_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<TransitionPipeline>,
}

impl DipToColorNode {
    /// Creates a dip-to-colour transition from clip A to clip B (`to_rgba`).
    #[must_use]
    pub fn new(
        progress: f32,
        color: [f32; 3],
        to_rgba: Vec<u8>,
        to_width: u32,
        to_height: u32,
    ) -> Self {
        Self {
            progress,
            color,
            to_rgba,
            to_width,
            to_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for DipToColorNode {
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.to_rgba.len() != rgba.len() {
            log::warn!(
                "DipToColorNode::process_cpu skipped: size mismatch a={} b={}",
                rgba.len(),
                self.to_rgba.len()
            );
            return;
        }
        let bg = [
            self.color[0] * 255.0,
            self.color[1] * 255.0,
            self.color[2] * 255.0,
            255.0,
        ];
        // `FFmpeg`'s progress is the complement of ours, and both curves are constant
        // over the frame, so they are evaluated once rather than per pixel.
        let g = 1.0 - self.progress;
        let s1 = smoothstep(1.0 - DIP_PHASE, 1.0, g);
        let s2 = smoothstep(DIP_PHASE, 1.0, g);
        for (px, b) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.to_rgba.as_chunks::<4>().0)
        {
            for c in 0..4 {
                let leaving = f32::from(px[c]) * s1 + bg[c] * (1.0 - s1);
                let arriving = bg[c] * s2 + f32::from(b[c]) * (1.0 - s2);
                // `lerp_u8(a, b, t)` is `a + (b - a) * t`, so this is
                // `leaving * g + arriving * (1 - g)` -- FFmpeg's outer `mix`.
                px[c] = lerp_u8(arriving, leaving, g);
            }
        }
    }
}

// GPU path (shared 2-input plumbing)

#[cfg(feature = "wgpu")]
struct TransitionPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

#[cfg(feature = "wgpu")]
fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

/// Builds the shared transition pipeline: bind group `tex_a` / `tex_b` / sampler /
/// uniform, a uniform buffer of `uniform_size` bytes, and -- when `mask` is set -- a
/// third texture at binding 4 carrying a per-pixel selection mask.
///
/// Only `DissolveTransitionNode` asks for the mask. The other three shaders declare
/// exactly the four bindings above, so handing them a layout entry they never read would
/// be dead surface.
#[cfg(feature = "wgpu")]
fn build_pipeline(
    device: &wgpu::Device,
    shader_src: &str,
    label: &str,
    uniform_size: u64,
    mask: bool,
) -> TransitionPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(shader_src.into()),
    });
    let mut entries = vec![
        tex_entry(0),
        tex_entry(1),
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
    ];
    if mask {
        entries.push(tex_entry(4));
    }
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
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
        label: Some(label),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: uniform_size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    TransitionPipeline {
        render_pipeline,
        bind_group_layout: bgl,
        sampler,
        uniform_buf,
    }
}

/// Uploads clip B to a temporary `Rgba8Unorm` texture.
#[cfg(feature = "wgpu")]
fn upload_frame(
    ctx: &crate::context::RenderContext,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> wgpu::Texture {
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Transition to_tex"),
        size: wgpu::Extent3d {
            width,
            height,
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
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    tex
}

/// Binds `tex_a` / `tex_b` / sampler / uniform and runs the full-screen pass.
#[cfg(feature = "wgpu")]
fn run_pass(
    ctx: &crate::context::RenderContext,
    pd: &TransitionPipeline,
    tex_a: &wgpu::Texture,
    tex_b: &wgpu::Texture,
    mask: Option<&wgpu::Texture>,
    output: &wgpu::Texture,
    label: &str,
) {
    let a_view = tex_a.create_view(&wgpu::TextureViewDescriptor::default());
    let b_view = tex_b.create_view(&wgpu::TextureViewDescriptor::default());
    let mask_view = mask.map(|m| m.create_view(&wgpu::TextureViewDescriptor::default()));
    let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
    let mut bind_entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&a_view),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(&b_view),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: wgpu::BindingResource::Sampler(&pd.sampler),
        },
        wgpu::BindGroupEntry {
            binding: 3,
            resource: pd.uniform_buf.as_entire_binding(),
        },
    ];
    if let Some(view) = mask_view.as_ref() {
        bind_entries.push(wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::TextureView(view),
        });
    }
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pd.bind_group_layout,
        entries: &bind_entries,
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
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

#[cfg(feature = "wgpu")]
fn pack_f32(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for WipeTransitionNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(tex_a) = inputs.first() else {
            log::warn!("WipeTransitionNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("WipeTransitionNode::process called with no outputs");
            return;
        };
        let pd = self.pipeline.get_or_init(|| {
            build_pipeline(
                &ctx.device,
                include_str!("../shaders/wipe.wgsl"),
                "Wipe",
                16,
                false,
            )
        });
        ctx.queue.write_buffer(
            &pd.uniform_buf,
            0,
            &pack_f32(&[self.progress, self.softness, self.angle, 0.0]),
        );
        let to_tex = upload_frame(ctx, &self.to_rgba, self.to_width, self.to_height);
        run_pass(ctx, pd, tex_a, &to_tex, None, output, "Wipe pass");
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for FadeTransitionNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(tex_a) = inputs.first() else {
            log::warn!("FadeTransitionNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("FadeTransitionNode::process called with no outputs");
            return;
        };
        // `crossfade.wgsl` is this node's shader: same binding layout as the other
        // transitions, and its one `f32` uniform is `progress` (see the type docs).
        let pd = self.pipeline.get_or_init(|| {
            build_pipeline(
                &ctx.device,
                include_str!("../shaders/crossfade.wgsl"),
                "Fade",
                16,
                false,
            )
        });
        ctx.queue.write_buffer(
            &pd.uniform_buf,
            0,
            &pack_f32(&[self.progress, 0.0, 0.0, 0.0]),
        );
        let to_tex = upload_frame(ctx, &self.to_rgba, self.to_width, self.to_height);
        run_pass(ctx, pd, tex_a, &to_tex, None, output, "Fade pass");
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for DissolveTransitionNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(tex_a) = inputs.first() else {
            log::warn!("DissolveTransitionNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("DissolveTransitionNode::process called with no outputs");
            return;
        };
        // The only transition that binds a third texture, so the only one that asks
        // `build_pipeline` for the mask entry.
        let pd = self.pipeline.get_or_init(|| {
            build_pipeline(
                &ctx.device,
                include_str!("../shaders/dissolve.wgsl"),
                "Dissolve",
                16,
                true,
            )
        });
        let to_tex = upload_frame(ctx, &self.to_rgba, self.to_width, self.to_height);
        let mask_tex = upload_frame(ctx, &self.mask, self.to_width, self.to_height);
        run_pass(
            ctx,
            pd,
            tex_a,
            &to_tex,
            Some(&mask_tex),
            output,
            "Dissolve pass",
        );
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for DipToColorNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(tex_a) = inputs.first() else {
            log::warn!("DipToColorNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("DipToColorNode::process called with no outputs");
            return;
        };
        let pd = self.pipeline.get_or_init(|| {
            build_pipeline(
                &ctx.device,
                include_str!("../shaders/dip.wgsl"),
                "Dip",
                32,
                false,
            )
        });
        ctx.queue.write_buffer(
            &pd.uniform_buf,
            0,
            &pack_f32(&[
                self.progress,
                0.0,
                0.0,
                0.0,
                self.color[0],
                self.color[1],
                self.color[2],
                1.0,
            ]),
        );
        let to_tex = upload_frame(ctx, &self.to_rgba, self.to_width, self.to_height);
        run_pass(ctx, pd, tex_a, &to_tex, None, output, "Dip pass");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_progress_zero_should_be_clip_a() {
        let b = vec![200u8, 200, 200, 255];
        let node = WipeTransitionNode::new(0.0, 0.0, 0.0, b, 1, 1);
        let a = vec![10u8, 20, 30, 255];
        let mut rgba = a.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, a, "progress=0 must output clip A");
    }

    #[test]
    fn wipe_at_progress_one_should_keep_ffmpegs_final_column() {
        // `FFmpeg`'s edge is an integer and its comparison is strict, so the last column
        // never flips: at progress 1 the axis-`+x` rule is `x > floor(w * 0)` = `x > 0`,
        // leaving column 0 on clip A. Reproducing that asymmetry is the point -- the
        // export has to land on FFmpeg's pixels, not on a tidier convention (#1732).
        let a = vec![
            10u8, 20, 30, 255, 10, 20, 30, 255, 10, 20, 30, 255, 10, 20, 30, 255,
        ];
        let b = vec![
            200u8, 210, 220, 255, 200, 210, 220, 255, 200, 210, 220, 255, 200, 210, 220, 255,
        ];
        let node = WipeTransitionNode::new(1.0, 0.0, 0.0, b, 4, 1);
        let mut rgba = a.clone();
        node.process_cpu(&mut rgba, 4, 1);
        assert_eq!(
            &rgba[0..4],
            &a[0..4],
            "column 0 stays on clip A at progress 1"
        );
        for x in 1..4 {
            assert_eq!(
                &rgba[x * 4..x * 4 + 3],
                &[200, 210, 220],
                "column {x} must be clip B at progress 1"
            );
        }
    }

    #[test]
    fn wipe_hard_edge_should_land_on_ffmpegs_integer_column() {
        // 8x1, angle 0 (axis +x), softness 0, progress 0.5. `FFmpeg`'s WIPELEFT computes
        // `z = width * (1 - progress) = 4` and takes clip B where `x > z`, so columns
        // 0..=4 are A and 5..=7 are B -- an asymmetric split, not four and four. A
        // normalised threshold puts the seam a column earlier, which is the entire
        // divergence this rule fixes (#1732).
        let a: Vec<u8> = (0..8).flat_map(|_| [10u8, 20, 30, 255]).collect();
        let b: Vec<u8> = (0..8).flat_map(|_| [200u8, 210, 220, 255]).collect();
        let node = WipeTransitionNode::new(0.5, 0.0, 0.0, b, 8, 1);
        let mut rgba = a.clone();
        node.process_cpu(&mut rgba, 8, 1);
        for x in 0..8 {
            let want: [u8; 3] = if x > 4 { [200, 210, 220] } else { [10, 20, 30] };
            assert_eq!(
                &rgba[x * 4..x * 4 + 3],
                &want,
                "column {x} at progress 0.5 (FFmpeg edge z=4)"
            );
        }
    }

    #[test]
    fn wipe_size_mismatch_should_leave_rgba_unchanged() {
        let b = vec![200u8; 8]; // 2 px
        let node = WipeTransitionNode::new(0.5, 0.0, 0.0, b, 2, 1);
        let original = vec![10u8, 20, 30, 255]; // 1 px
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "size mismatch must be a no-op");
    }

    /// Clip A and clip B of the dissolve tests: every channel differs between the two
    /// and no channel repeats within a frame, so a swapped pair, a dropped channel or a
    /// transposed one all show up in the assertions below.
    const FADE_A: [u8; 4] = [10, 200, 30, 255];
    const FADE_B: [u8; 4] = [210, 40, 130, 55];

    #[test]
    fn fade_transition_progress_zero_should_be_clip_a() {
        let node = FadeTransitionNode::new(0.0, FADE_B.to_vec(), 1, 1);
        let mut rgba = FADE_A.to_vec();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, FADE_A, "progress=0 must output clip A");
    }

    #[test]
    fn fade_transition_progress_one_should_be_clip_b() {
        let node = FadeTransitionNode::new(1.0, FADE_B.to_vec(), 1, 1);
        let mut rgba = FADE_A.to_vec();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, FADE_B, "progress=1 must output clip B");
    }

    #[test]
    fn fade_transition_half_should_average_the_pair() {
        // The acceptance criterion. On its own it cannot tell A from B (the mix is
        // symmetric at 0.5); the endpoint tests above are what pin the direction.
        let node = FadeTransitionNode::new(0.5, FADE_B.to_vec(), 1, 1);
        let mut rgba = FADE_A.to_vec();
        node.process_cpu(&mut rgba, 1, 1);
        for (c, got) in rgba.iter().enumerate() {
            let want = f32::midpoint(f32::from(FADE_A[c]), f32::from(FADE_B[c]));
            assert!(
                (f32::from(*got) - want).abs() <= 1.0,
                "channel {c}: got {got} want {want}"
            );
        }
    }

    #[test]
    fn fade_transition_size_mismatch_should_leave_rgba_unchanged() {
        let node = FadeTransitionNode::new(0.5, vec![200u8; 8], 2, 1); // 2 px of B
        let original = FADE_A.to_vec(); // 1 px of A
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "size mismatch must be a no-op");
    }

    #[test]
    fn dissolve_with_an_empty_mask_should_be_clip_a() {
        let node = DissolveTransitionNode::new(vec![0u8; 4], vec![210u8, 40, 130, 55], 1, 1);
        let a = vec![10u8, 200, 30, 255];
        let mut rgba = a.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, a, "an unset mask must leave clip A");
    }

    #[test]
    fn dissolve_with_a_full_mask_should_be_clip_b() {
        let b = vec![210u8, 40, 130, 55];
        let node = DissolveTransitionNode::new(vec![255u8; 4], b.clone(), 1, 1);
        let mut rgba = vec![10u8, 200, 30, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, b, "a set mask must reveal clip B");
    }

    #[test]
    fn dissolve_should_follow_the_mask_pixel_for_pixel() {
        // The property that separates this node from `FadeTransitionNode`: it *selects*,
        // so every pixel stays fully one clip or the other, and which one is the mask's
        // decision rather than the node's. Pinning the selection exactly (not "about
        // half") is what makes the node reusable for `FFmpeg`'s own dissolve, whose mask
        // is computed elsewhere precisely because it cannot be recomputed here.
        let (w, h) = (8u32, 4u32);
        let n = (w * h) as usize;
        let a: Vec<u8> = [0u8, 0, 0, 255].repeat(n);
        let b: Vec<u8> = [255u8, 255, 255, 255].repeat(n);
        // An irregular pattern, so a node that ignored the mask and thresholded on its
        // own could not coincidentally agree.
        let mut mask = vec![0u8; n * 4];
        for i in 0..n {
            if i % 3 == 0 {
                mask[i * 4..i * 4 + 4].fill(255);
            }
        }
        let node = DissolveTransitionNode::new(mask, b, w, h);
        let mut rgba = a.clone();
        node.process_cpu(&mut rgba, w, h);
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let want = if i % 3 == 0 { 255 } else { 0 };
            assert_eq!(px[0], want, "pixel {i} must follow the mask");
        }
    }

    #[test]
    fn dissolve_size_mismatch_should_leave_rgba_unchanged() {
        let node = DissolveTransitionNode::new(vec![255u8; 8], vec![200u8; 8], 2, 1);
        let original = vec![10u8, 200, 30, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "size mismatch must be a no-op");
    }

    #[test]
    fn dissolve_mask_size_mismatch_should_leave_rgba_unchanged() {
        // Clip B is the right size but the mask is not: still a no-op rather than a
        // partially-applied frame.
        let node = DissolveTransitionNode::new(vec![255u8; 8], vec![200u8; 4], 1, 1);
        let original = vec![10u8, 200, 30, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "a mask size mismatch must be a no-op");
    }

    #[test]
    fn dip_progress_zero_should_be_clip_a() {
        let b = vec![200u8, 200, 200, 255];
        let node = DipToColorNode::new(0.0, [0.0, 0.0, 0.0], b, 1, 1);
        let a = vec![10u8, 20, 30, 255];
        let mut rgba = a.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, a, "progress=0 must output clip A");
    }

    #[test]
    fn dip_at_half_should_follow_ffmpegs_phased_curve() {
        // The midpoint is *not* the solid frame -- that is the linear dip this replaced.
        // With `FFmpeg`'s curve at progress 0.5 (so its own progress is 0.5 too):
        //   s1 = smoothstep(0.8, 1, 0.5) = 0            -> leaving  = bg = 0
        //   s2 = smoothstep(0.2, 1, 0.5) = 0.31640625   -> arriving = 200 * 0.68359 = 136.7
        //   out = 0 * 0.5 + 136.7 * 0.5                 = 68
        // Pinning the arithmetic rather than a vague "dark" keeps the phase honest: a
        // linear dip would read 0 here.
        let b = vec![200u8, 200, 200, 255];
        let node = DipToColorNode::new(0.5, [0.0, 0.0, 0.0], b, 1, 1);
        let mut rgba = vec![120u8, 130, 140, 255];
        node.process_cpu(&mut rgba, 1, 1);
        for (i, got) in rgba[0..3].iter().enumerate() {
            assert!(
                (i32::from(*got) - 68).abs() <= 1,
                "progress=0.5 must follow FFmpeg's phased curve (~68) at {i}, got {got}"
            );
        }
    }

    #[test]
    fn dip_should_be_darkest_before_the_midpoint() {
        // `FFmpeg`'s `phase` of 0.2 puts the solid stretch in the first part of the
        // transition, not at the centre. Sampling across progress, the darkest frame must
        // land nearer 0.2 than 0.5 -- the property the old linear dip got backwards.
        let b = vec![200u8, 200, 200, 255];
        let darkest = (1..=9)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let p = i as f32 / 10.0;
                let node = DipToColorNode::new(p, [0.0, 0.0, 0.0], b.clone(), 1, 1);
                let mut rgba = vec![120u8, 130, 140, 255];
                node.process_cpu(&mut rgba, 1, 1);
                (rgba[0], i)
            })
            .min()
            .map(|(_, i)| i)
            .expect("the sweep is non-empty");
        assert!(
            darkest <= 3,
            "the dip must bottom out in its first phase (<= 0.3), got progress 0.{darkest}"
        );
    }

    #[test]
    fn dip_progress_one_should_be_clip_b() {
        let b = vec![200u8, 210, 220, 255];
        let node = DipToColorNode::new(1.0, [0.0, 0.0, 0.0], b.clone(), 1, 1);
        let mut rgba = vec![10u8, 20, 30, 255];
        node.process_cpu(&mut rgba, 1, 1);
        for (got, want) in rgba.iter().zip(b.iter()) {
            assert!(
                (i32::from(*got) - i32::from(*want)).abs() <= 1,
                "progress=1 must output clip B"
            );
        }
    }

    #[test]
    fn dip_phase_two_size_mismatch_should_leave_rgba_unchanged() {
        let b = vec![200u8; 8]; // 2 px
        let node = DipToColorNode::new(0.75, [0.0, 0.0, 0.0], b, 2, 1);
        let original = vec![10u8, 20, 30, 255]; // 1 px
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "phase-2 size mismatch must be a no-op");
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

    #[test]
    fn dissolve_gpu_should_follow_the_mask() {
        let Some(ctx) = ctx() else {
            return;
        };
        // The dissolve is the only transition that binds a third texture, so this is the
        // only test that exercises `build_pipeline`'s mask entry and `run_pass` binding
        // it. An irregular pattern, so a shader that ignored the mask could not agree by
        // coincidence; and `textureLoad`, not `textureSample`, so the linear sampler
        // cannot blur a per-pixel decision.
        let (w, h) = (8u32, 4u32);
        let n = (w * h) as usize;
        let a: Vec<u8> = [0u8, 0, 0, 255].repeat(n);
        let b: Vec<u8> = [255u8, 255, 255, 255].repeat(n);
        let mut mask = vec![0u8; n * 4];
        for i in 0..n {
            if i % 3 == 0 {
                mask[i * 4..i * 4 + 4].fill(255);
            }
        }
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(DissolveTransitionNode::new(mask, b, w, h))
            .process_gpu(&a, w, h)
            .expect("gpu dissolve");
        for (i, px) in out.as_chunks::<4>().0.iter().enumerate() {
            let want: u8 = if i % 3 == 0 { 255 } else { 0 };
            assert!(
                (i32::from(px[0]) - i32::from(want)).abs() <= 2,
                "GPU pixel {i} must follow the mask: got {} want {want}",
                px[0]
            );
        }
    }

    #[test]
    fn wipe_gpu_should_land_on_ffmpegs_integer_column() {
        let Some(ctx) = ctx() else {
            return;
        };
        // The CPU mirror of this is `wipe_hard_edge_should_land_on_ffmpegs_integer_column`;
        // the shader has to agree column for column or the export and the preview drift.
        let a: Vec<u8> = (0..8).flat_map(|_| [10u8, 20, 30, 255]).collect();
        let b: Vec<u8> = (0..8).flat_map(|_| [200u8, 210, 220, 255]).collect();
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(WipeTransitionNode::new(0.5, 0.0, 0.0, b, 8, 1))
            .process_gpu(&a, 8, 1)
            .expect("gpu wipe");
        for x in 0..8 {
            let want: [u8; 3] = if x > 4 { [200, 210, 220] } else { [10, 20, 30] };
            for i in 0..3 {
                assert!(
                    (i32::from(out[x * 4 + i]) - i32::from(want[i])).abs() <= 2,
                    "GPU column {x} channel {i} at progress 0.5 (FFmpeg edge z=4)"
                );
            }
        }
    }

    #[test]
    fn dip_gpu_at_half_should_match_the_cpu_curve() {
        let Some(ctx) = ctx() else {
            return;
        };
        let a = vec![120u8, 130, 140, 255];
        let b = vec![200u8, 200, 200, 255];
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(DipToColorNode::new(0.5, [0.0, 0.0, 0.0], b, 1, 1))
            .process_gpu(&a, 1, 1)
            .expect("gpu dip");
        // Same arithmetic as `dip_at_half_should_follow_ffmpegs_phased_curve`.
        for i in 0..3 {
            assert!(
                (i32::from(out[i]) - 68).abs() <= 2,
                "GPU dip at progress 0.5 must follow FFmpeg's curve (~68) at {i}, got {}",
                out[i]
            );
        }
    }
}
