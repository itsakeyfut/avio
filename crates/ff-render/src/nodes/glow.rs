//! Three-pass GPU bloom / glow render node.
//!
//! Pass 0 extracts pixels at or above a luma `threshold` (others become black),
//! pass 1 Gaussian-blurs those highlights (reusing [`GaussianBlurNode`]), and
//! pass 2 adds the blurred highlights back onto the original frame weighted by
//! `intensity`. A CPU fallback ([`RenderNodeCpu`]) mirrors the same three steps.

use super::RenderNodeCpu;
use super::blur::separable_blur_f32;

#[cfg(feature = "wgpu")]
use super::GaussianBlurNode;
#[cfg(feature = "wgpu")]
use super::blur::{create_uniform, fullscreen_pipeline, run_fullscreen, texture_entry};

// GlowNode

/// Three-pass GPU bloom / glow effect.
///
/// Pass 0: extract pixels whose luma is `>= threshold` (others become black).
/// Pass 1: Gaussian-blur the extracted highlights (sigma = `radius`).
/// Pass 2: additive blend of original + blurred highlights weighted by `intensity`.
pub struct GlowNode {
    /// Luminance threshold, clamped to `[0.0, 1.0]`; pixels below it are
    /// suppressed before the glow blur.
    pub threshold: f32,
    /// Gaussian blur sigma in pixels for the glow spread (shares the blur node's
    /// `[0.5, 20.0]` effective range).
    pub radius: f32,
    /// Additive blend weight of the glow layer (`0.0` = no glow / identity).
    pub intensity: f32,
    /// The blur node reused for pass 1 (its pipeline caches independently).
    #[cfg(feature = "wgpu")]
    blur: GaussianBlurNode,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<GlowPipeline>,
}

impl GlowNode {
    /// Creates a glow node with the given threshold, blur radius, and intensity.
    #[must_use]
    pub fn new(threshold: f32, radius: f32, intensity: f32) -> Self {
        Self {
            threshold,
            radius,
            intensity,
            #[cfg(feature = "wgpu")]
            blur: GaussianBlurNode::new(radius),
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for GlowNode {
    /// Identity node (no glow).
    fn default() -> Self {
        Self::new(0.8, 10.0, 0.0)
    }
}

// CPU path

impl RenderNodeCpu for GlowNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32) {
        let threshold = self.threshold.clamp(0.0, 1.0);

        // Pass 0: extract highlights into an opaque black scratch buffer.
        let mut highlights = vec![0u8; rgba.len()];
        for (dst, px) in highlights
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(rgba.as_chunks::<4>().0)
        {
            let luma =
                (0.299 * f32::from(px[0]) + 0.587 * f32::from(px[1]) + 0.114 * f32::from(px[2]))
                    / 255.0;
            if luma >= threshold {
                dst[0..3].copy_from_slice(&px[0..3]);
            }
            dst[3] = 255;
        }

        // Pass 1: Gaussian-blur the highlights (shared kernel with the blur node).
        let Some(blurred) = separable_blur_f32(&highlights, w, h, self.radius) else {
            return;
        };

        // Pass 2: additive blend of the original with the blurred highlights.
        for (px, glow) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(blurred.as_chunks::<4>().0)
        {
            for c in 0..3 {
                let base = f32::from(px[c]) / 255.0;
                let out = (base + glow[c] * self.intensity).clamp(0.0, 1.0);
                px[c] = (out * 255.0 + 0.5) as u8;
            }
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
struct GlowPipeline {
    extract: EffectPipeline,
    blend: EffectPipeline,
}

#[cfg(feature = "wgpu")]
struct EffectPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
}

#[cfg(feature = "wgpu")]
impl GlowNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &GlowPipeline {
        self.pipeline.get_or_init(|| GlowPipeline {
            extract: create_extract_pipeline(ctx, self.threshold.clamp(0.0, 1.0)),
            blend: create_blend_pipeline(ctx, self.intensity),
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for GlowNode {
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
            log::warn!("GlowNode::process called with no inputs");
            return;
        };
        if outputs.len() < 3 {
            log::warn!("GlowNode::process needs 3 output targets");
            return;
        }
        let pd = self.get_or_create_pipeline(ctx);

        // Pass 0: extract highlights (input -> outputs[0]).
        encode_uniform_texture_pass(ctx, &pd.extract, &[input], outputs[0], "Glow extract");

        // Pass 1: blur the highlights. Delegating to GaussianBlurNode runs the two
        // separable passes as outputs[0] -> outputs[1] -> outputs[0], leaving the
        // blurred highlights in outputs[0]. The extract in outputs[0] is consumed by
        // the horizontal pass before the vertical pass overwrites it.
        self.blur
            .process(&[outputs[0]], &[outputs[1], outputs[0]], ctx);

        // Pass 2: additive blend of the original with the blurred highlights
        // (input + outputs[0] -> outputs[2], the final target).
        encode_uniform_texture_pass(
            ctx,
            &pd.blend,
            &[input, outputs[0]],
            outputs[2],
            "Glow blend",
        );
    }
}

/// Builds the single-pass highlight-extraction pipeline (texture + `threshold`).
#[cfg(feature = "wgpu")]
fn create_extract_pipeline(ctx: &crate::context::RenderContext, threshold: f32) -> EffectPipeline {
    let device = &ctx.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Glow extract shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/glow_extract.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Glow extract BGL"),
        entries: &[texture_entry(0), uniform_entry(1)],
    });
    let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "Glow extract");
    let uniform_buf = create_uniform(device, "Glow extract uniforms", 16);
    ctx.queue
        .write_buffer(&uniform_buf, 0, &pack_scalar(threshold));
    EffectPipeline {
        render_pipeline,
        bind_group_layout: bgl,
        uniform_buf,
    }
}

/// Builds the two-input additive-blend pipeline (original + blurred + `intensity`).
#[cfg(feature = "wgpu")]
fn create_blend_pipeline(ctx: &crate::context::RenderContext, intensity: f32) -> EffectPipeline {
    let device = &ctx.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Glow blend shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/glow_blend.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Glow blend BGL"),
        entries: &[texture_entry(0), texture_entry(1), uniform_entry(2)],
    });
    let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "Glow blend");
    let uniform_buf = create_uniform(device, "Glow blend uniforms", 16);
    ctx.queue
        .write_buffer(&uniform_buf, 0, &pack_scalar(intensity));
    EffectPipeline {
        render_pipeline,
        bind_group_layout: bgl,
        uniform_buf,
    }
}

#[cfg(feature = "wgpu")]
fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Packs one f32 into a 16-byte uniform (the shader pads it to a vec4).
#[cfg(feature = "wgpu")]
fn pack_scalar(v: f32) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&v.to_le_bytes());
    b
}

/// Encodes one fullscreen pass binding `textures` at bindings `0..n` followed by
/// the pipeline's uniform at binding `n`, writing into `output`.
// `textures.len()` is 1 or 2 here, so the binding-index casts cannot truncate.
#[cfg(feature = "wgpu")]
#[allow(clippy::cast_possible_truncation)]
fn encode_uniform_texture_pass(
    ctx: &crate::context::RenderContext,
    pd: &EffectPipeline,
    textures: &[&wgpu::Texture],
    output: &wgpu::Texture,
    label: &str,
) {
    let views: Vec<wgpu::TextureView> = textures
        .iter()
        .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
        .collect();
    let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

    let mut entries: Vec<wgpu::BindGroupEntry> = views
        .iter()
        .enumerate()
        .map(|(i, view)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(view),
        })
        .collect();
    entries.push(wgpu::BindGroupEntry {
        binding: views.len() as u32,
        resource: pd.uniform_buf.as_entire_binding(),
    });

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pd.bind_group_layout,
        entries: &entries,
    });
    run_fullscreen(ctx, &pd.render_pipeline, &bind_group, &output_view, label);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w × h` opaque black frame with a filled `[x0, x1) × [y0, y1)` white
    /// rectangle (used as the glow source).
    fn white_rect(
        w: usize,
        h: usize,
        x0: usize,
        x1: usize,
        y0: usize,
        y1: usize,
        v: u8,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; w * h * 4];
        for (i, px) in buf.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = i % w;
            let y = i / w;
            px[3] = 255;
            if x >= x0 && x < x1 && y >= y0 && y < y1 {
                px[0] = v;
                px[1] = v;
                px[2] = v;
            }
        }
        buf
    }

    #[test]
    fn glow_should_produce_halo_beyond_edge() {
        // 16x16 white rectangle centred in a 48x48 black frame; right edge at x=32.
        let (w, h) = (48usize, 48usize);
        let mut frame = white_rect(w, h, 16, 32, 16, 32, 255);
        GlowNode::new(0.8, 10.0, 1.0).process_cpu(&mut frame, w as u32, h as u32);
        // A pixel 5 px beyond the right edge (x=37, y=24) must have gained glow.
        let p = (24 * w + 37) * 4;
        assert!(
            frame[p] > 0,
            "glow halo must extend >= 5 px beyond the edge; got {}",
            frame[p]
        );
    }

    #[test]
    fn glow_intensity_zero_should_be_noop() {
        let (w, h) = (32usize, 32usize);
        let original = white_rect(w, h, 8, 24, 8, 24, 255);
        let mut frame = original.clone();
        GlowNode::new(0.8, 10.0, 0.0).process_cpu(&mut frame, w as u32, h as u32);
        for (a, b) in frame.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "intensity 0 must be a no-op (within rounding); got {a} vs {b}"
            );
        }
    }

    #[test]
    fn glow_high_threshold_should_suppress() {
        // A bright but sub-white rectangle (luma ~0.78): with threshold 1.1 clamped
        // to 1.0, no pixel qualifies, so the frame is unchanged.
        let (w, h) = (32usize, 32usize);
        let original = white_rect(w, h, 8, 24, 8, 24, 200);
        let mut frame = original.clone();
        GlowNode::new(1.1, 10.0, 1.0).process_cpu(&mut frame, w as u32, h as u32);
        for (a, b) in frame.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "threshold above the brightest luma must suppress all glow; got {a} vs {b}"
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

    fn white_rect(
        w: usize,
        h: usize,
        x0: usize,
        x1: usize,
        y0: usize,
        y1: usize,
        v: u8,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; w * h * 4];
        for (i, px) in buf.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = i % w;
            let y = i / w;
            px[3] = 255;
            if x >= x0 && x < x1 && y >= y0 && y < y1 {
                px[0] = v;
                px[1] = v;
                px[2] = v;
            }
        }
        buf
    }

    #[test]
    fn glow_gpu_should_produce_halo_beyond_edge() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (48u32, 48u32);
        let frame = white_rect(48, 48, 16, 32, 16, 32, 255);
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(GlowNode::new(0.8, 10.0, 1.0))
            .process_gpu(&frame, w, h)
            .expect("gpu glow");
        let p = (24 * 48 + 37) * 4;
        assert!(
            gpu[p] > 0,
            "GPU glow halo must extend >= 5 px beyond the edge; got {}",
            gpu[p]
        );
    }

    #[test]
    fn glow_gpu_intensity_zero_should_preserve_input() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (32u32, 32u32);
        let frame = white_rect(32, 32, 8, 24, 8, 24, 255);
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(GlowNode::new(0.8, 10.0, 0.0))
            .process_gpu(&frame, w, h)
            .expect("gpu glow");
        // Sample the rectangle centre and a background pixel: both must be unchanged.
        for &idx in &[(16 * 32 + 16) * 4, (2 * 32 + 2) * 4] {
            assert!(
                (i32::from(gpu[idx]) - i32::from(frame[idx])).abs() <= 1,
                "intensity 0 must preserve the input on the GPU path"
            );
        }
    }
}
