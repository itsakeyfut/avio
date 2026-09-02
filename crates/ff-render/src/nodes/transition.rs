//! Two-clip transition nodes: a directional wipe, a linear dissolve, and a two-phase
//! dip to a solid colour. Like [`CrossfadeNode`](super::crossfade::CrossfadeNode), the second clip
//! (B) is carried as RGBA bytes in the node and uploaded to a GPU texture at render
//! time: a single render graph has one source, so B cannot arrive as a second GPU
//! input. Clip A is the node's input (`inputs[0]` / the `process_cpu` argument).

use super::RenderNodeCpu;

/// Linear interpolation `a + (b - a) * t`, rounded to the nearest byte.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lerp_u8(a: f32, b: f32, t: f32) -> u8 {
    (a + (b - a) * t + 0.5).clamp(0.0, 255.0) as u8
}

/// `smoothstep(e0, e1, x)`; callers guarantee `e1 > e0`.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// WipeTransitionNode

/// Directional wipe that reveals clip B behind a moving edge.
///
/// `progress = 0` outputs clip A, `progress = 1` outputs clip B, and the edge sweeps
/// across the frame between them along `angle` (radians: `0` = left→right,
/// `π/2` = top→bottom). `softness` feathers the edge (normalised units).
pub struct WipeTransitionNode {
    /// Transition progress `[0, 1]`: 0 = clip A, 1 = clip B.
    pub progress: f32,
    /// Edge feather half-width in normalised units (0 = hard edge).
    pub softness: f32,
    /// Wipe direction in radians (0 = left→right, `π/2` = top→bottom).
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

    /// The B-weight (`mask`) at a normalised pixel centre. Shared by the CPU and GPU
    /// paths (`wipe.wgsl` uses the identical formula) so they agree.
    fn mask_at(&self, uv_x: f32, uv_y: f32) -> f32 {
        let (ax, ay) = (self.angle.cos(), self.angle.sin());
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
        let (wf, hf) = (w as f32, h as f32);
        for y in 0..h {
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                let mask = self.mask_at((x as f32 + 0.5) / wf, (y as f32 + 0.5) / hf);
                for c in 0..4 {
                    let a = f32::from(rgba[idx + c]);
                    let b = f32::from(self.to_rgba[idx + c]);
                    rgba[idx + c] = lerp_u8(a, b, mask);
                }
            }
        }
    }
}

// DissolveTransitionNode

/// Linear dissolve: clip A mixed into clip B by `progress`.
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
pub struct DissolveTransitionNode {
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

impl DissolveTransitionNode {
    /// Creates a dissolve from clip A (the node input) to clip B (`to_rgba`).
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

impl RenderNodeCpu for DissolveTransitionNode {
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.to_rgba.len() != rgba.len() {
            log::warn!(
                "DissolveTransitionNode::process_cpu skipped: size mismatch a={} b={}",
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

// DipToColorNode

/// Two-phase transition: clip A fades to a solid `color`, then the colour fades to
/// clip B. `progress = 0.5` is the fully solid dip (a fade-to-black/white/brand dip).
pub struct DipToColorNode {
    /// Transition progress `[0, 1]`: 0 = clip A, 0.5 = solid `color`, 1 = clip B.
    pub progress: f32,
    /// Dip colour in RGB `[0, 1]` (e.g. `[0, 0, 0]` for fade-to-black).
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
        let dip = [
            self.color[0] * 255.0,
            self.color[1] * 255.0,
            self.color[2] * 255.0,
            255.0,
        ];
        if self.progress < 0.5 {
            // Phase 1: clip A -> dip colour. Clip B is not needed yet.
            let t = self.progress * 2.0;
            for px in rgba.as_chunks_mut::<4>().0 {
                for c in 0..4 {
                    px[c] = lerp_u8(f32::from(px[c]), dip[c], t);
                }
            }
            return;
        }
        // Phase 2: dip colour -> clip B.
        if self.to_rgba.len() != rgba.len() {
            log::warn!(
                "DipToColorNode::process_cpu skipped: size mismatch a={} b={}",
                rgba.len(),
                self.to_rgba.len()
            );
            return;
        }
        let t = (self.progress - 0.5) * 2.0;
        for (px, b) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.to_rgba.as_chunks::<4>().0)
        {
            for c in 0..4 {
                px[c] = lerp_u8(dip[c], f32::from(b[c]), t);
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
/// uniform, and a uniform buffer of `uniform_size` bytes.
#[cfg(feature = "wgpu")]
fn build_pipeline(
    device: &wgpu::Device,
    shader_src: &str,
    label: &str,
    uniform_size: u64,
) -> TransitionPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(shader_src.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
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
        ],
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
    output: &wgpu::Texture,
    label: &str,
) {
    let a_view = tex_a.create_view(&wgpu::TextureViewDescriptor::default());
    let b_view = tex_b.create_view(&wgpu::TextureViewDescriptor::default());
    let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pd.bind_group_layout,
        entries: &[
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
        ],
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
            )
        });
        ctx.queue.write_buffer(
            &pd.uniform_buf,
            0,
            &pack_f32(&[self.progress, self.softness, self.angle, 0.0]),
        );
        let to_tex = upload_frame(ctx, &self.to_rgba, self.to_width, self.to_height);
        run_pass(ctx, pd, tex_a, &to_tex, output, "Wipe pass");
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
        // `crossfade.wgsl` is this node's shader: same binding layout as the other
        // transitions, and its one `f32` uniform is `progress` (see the type docs).
        let pd = self.pipeline.get_or_init(|| {
            build_pipeline(
                &ctx.device,
                include_str!("../shaders/crossfade.wgsl"),
                "Dissolve",
                16,
            )
        });
        ctx.queue.write_buffer(
            &pd.uniform_buf,
            0,
            &pack_f32(&[self.progress, 0.0, 0.0, 0.0]),
        );
        let to_tex = upload_frame(ctx, &self.to_rgba, self.to_width, self.to_height);
        run_pass(ctx, pd, tex_a, &to_tex, output, "Dissolve pass");
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
            build_pipeline(&ctx.device, include_str!("../shaders/dip.wgsl"), "Dip", 32)
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
        run_pass(ctx, pd, tex_a, &to_tex, output, "Dip pass");
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
    fn wipe_progress_one_should_be_clip_b() {
        let b = vec![200u8, 210, 220, 255];
        let node = WipeTransitionNode::new(1.0, 0.0, 0.0, b.clone(), 1, 1);
        let mut rgba = vec![10u8, 20, 30, 255];
        node.process_cpu(&mut rgba, 1, 1);
        for (got, want) in rgba.iter().zip(b.iter()) {
            assert!(
                (i32::from(*got) - i32::from(*want)).abs() <= 1,
                "progress=1 must output clip B; got {got} want {want}"
            );
        }
    }

    #[test]
    fn wipe_half_hard_should_split_left_a_right_b() {
        // A 2×1 frame, angle=0, softness=0: left pixel = A, right pixel = B.
        let a = vec![10u8, 20, 30, 255, 10, 20, 30, 255];
        let b = vec![200u8, 210, 220, 255, 200, 210, 220, 255];
        let node = WipeTransitionNode::new(0.5, 0.0, 0.0, b, 2, 1);
        let mut rgba = a.clone();
        node.process_cpu(&mut rgba, 2, 1);
        assert_eq!(&rgba[0..4], &[10, 20, 30, 255], "left half must be clip A");
        for (got, want) in rgba[4..8].iter().zip([200u8, 210, 220, 255].iter()) {
            assert!(
                (i32::from(*got) - i32::from(*want)).abs() <= 1,
                "right half must be clip B"
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
    const DISSOLVE_A: [u8; 4] = [10, 200, 30, 255];
    const DISSOLVE_B: [u8; 4] = [210, 40, 130, 55];

    #[test]
    fn dissolve_progress_zero_should_be_clip_a() {
        let node = DissolveTransitionNode::new(0.0, DISSOLVE_B.to_vec(), 1, 1);
        let mut rgba = DISSOLVE_A.to_vec();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, DISSOLVE_A, "progress=0 must output clip A");
    }

    #[test]
    fn dissolve_progress_one_should_be_clip_b() {
        let node = DissolveTransitionNode::new(1.0, DISSOLVE_B.to_vec(), 1, 1);
        let mut rgba = DISSOLVE_A.to_vec();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, DISSOLVE_B, "progress=1 must output clip B");
    }

    #[test]
    fn dissolve_half_should_average_the_pair() {
        // The acceptance criterion. On its own it cannot tell A from B (the mix is
        // symmetric at 0.5); the endpoint tests above are what pin the direction.
        let node = DissolveTransitionNode::new(0.5, DISSOLVE_B.to_vec(), 1, 1);
        let mut rgba = DISSOLVE_A.to_vec();
        node.process_cpu(&mut rgba, 1, 1);
        for (c, got) in rgba.iter().enumerate() {
            let want = f32::midpoint(f32::from(DISSOLVE_A[c]), f32::from(DISSOLVE_B[c]));
            assert!(
                (f32::from(*got) - want).abs() <= 1.0,
                "channel {c}: got {got} want {want}"
            );
        }
    }

    #[test]
    fn dissolve_size_mismatch_should_leave_rgba_unchanged() {
        let node = DissolveTransitionNode::new(0.5, vec![200u8; 8], 2, 1); // 2 px of B
        let original = DISSOLVE_A.to_vec(); // 1 px of A
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "size mismatch must be a no-op");
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
    fn dip_half_black_should_be_black() {
        let b = vec![200u8, 200, 200, 255];
        let node = DipToColorNode::new(0.5, [0.0, 0.0, 0.0], b, 1, 1);
        let mut rgba = vec![120u8, 130, 140, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(
            &rgba[0..3],
            &[0, 0, 0],
            "progress=0.5 with black dip must be a black frame"
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
    fn wipe_gpu_progress_one_should_be_clip_b() {
        let Some(ctx) = ctx() else {
            return;
        };
        let a = vec![10u8, 20, 30, 255];
        let b = vec![200u8, 210, 220, 255];
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(WipeTransitionNode::new(1.0, 0.0, 0.0, b.clone(), 1, 1))
            .process_gpu(&a, 1, 1)
            .expect("gpu wipe");
        for i in 0..3 {
            assert!(
                (i32::from(out[i]) - i32::from(b[i])).abs() <= 2,
                "GPU wipe progress=1 must output clip B at {i}"
            );
        }
    }

    #[test]
    fn dip_gpu_half_black_should_be_black() {
        let Some(ctx) = ctx() else {
            return;
        };
        let a = vec![120u8, 130, 140, 255];
        let b = vec![200u8, 200, 200, 255];
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(DipToColorNode::new(0.5, [0.0, 0.0, 0.0], b, 1, 1))
            .process_gpu(&a, 1, 1)
            .expect("gpu dip");
        for i in 0..3 {
            assert!(
                out[i] <= 2,
                "GPU dip progress=0.5 (black) must be ~0 at {i}"
            );
        }
    }
}
