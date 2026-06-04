//! Mask nodes: `ShapeMaskNode`, `LumaMaskNode`, `AlphaMatteNode` + shared pipeline.

use super::chroma_key::bt709_luma;
#[cfg(feature = "wgpu")]
use super::helpers::{
    fullscreen_pipeline, linear_sampler, submit_render_pass, two_tex_sampler_uniform_bgl,
    upload_rgba_texture,
};
use crate::nodes::RenderNodeCpu;

// ── Shared mask pipeline ──────────────────────────────────────────────────────

#[cfg(feature = "wgpu")]
struct MaskPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

#[cfg(feature = "wgpu")]
fn create_mask_pipeline(ctx: &crate::context::RenderContext) -> MaskPipeline {
    let device = &ctx.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Mask shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/mask.wgsl").into()),
    });
    let bgl = two_tex_sampler_uniform_bgl(device, "Mask");
    let render_pipeline = fullscreen_pipeline(device, &shader, "Mask", &bgl);
    let sampler = linear_sampler(device, "Mask");
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Mask uniforms"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    MaskPipeline {
        render_pipeline,
        bind_group_layout: bgl,
        sampler,
        uniform_buf,
    }
}

#[cfg(feature = "wgpu")]
fn submit_mask_pass(
    ctx: &crate::context::RenderContext,
    pd: &MaskPipeline,
    base_tex: &wgpu::Texture,
    mask_tex: &wgpu::Texture,
    output_tex: &wgpu::Texture,
    mode: u32,
    label: &str,
) {
    let mode_bytes = mode.to_le_bytes();
    let uniforms: [u8; 16] = [
        mode_bytes[0],
        mode_bytes[1],
        mode_bytes[2],
        mode_bytes[3],
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniforms);

    let base_view = base_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mask_view = mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let out_view = output_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pd.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&base_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&mask_view),
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
    submit_render_pass(ctx, &pd.render_pipeline, &bind_group, &out_view, label);
}

// ── ShapeMaskNode ─────────────────────────────────────────────────────────────

/// Mask `inputs[0]` using the alpha channel of `inputs[1]` (or `mask_rgba`).
///
/// Pixels where the mask alpha is > 0 are kept opaque; all others are made
/// fully transparent (hard threshold at ~1/255).
pub struct ShapeMaskNode {
    /// Mask frame RGBA bytes (required for the CPU path).
    pub mask_rgba: Vec<u8>,
    /// Width of `mask_rgba`.
    pub mask_width: u32,
    /// Height of `mask_rgba`.
    pub mask_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<MaskPipeline>,
}

impl ShapeMaskNode {
    #[must_use]
    pub fn new(mask_rgba: Vec<u8>, mask_width: u32, mask_height: u32) -> Self {
        Self {
            mask_rgba,
            mask_width,
            mask_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for ShapeMaskNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.mask_rgba.len() != rgba.len() {
            return;
        }
        for (base, mask) in rgba.chunks_exact_mut(4).zip(self.mask_rgba.chunks_exact(4)) {
            let keep = if mask[3] > 1 { 1.0_f32 } else { 0.0_f32 };
            let a = f32::from(base[3]) / 255.0;
            base[3] = ((a * keep).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
}

#[cfg(feature = "wgpu")]
impl ShapeMaskNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &MaskPipeline {
        self.pipeline.get_or_init(|| create_mask_pipeline(ctx))
    }
}

#[cfg(feature = "wgpu")]
impl crate::nodes::RenderNode for ShapeMaskNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(base_tex) = inputs.first() else {
            log::warn!("ShapeMaskNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("ShapeMaskNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);
        let mask_tex = upload_rgba_texture(
            ctx,
            &self.mask_rgba,
            self.mask_width,
            self.mask_height,
            "ShapeMask mask",
        );
        submit_mask_pass(ctx, pd, base_tex, &mask_tex, output, 0, "ShapeMask BG");
    }
}

// ── LumaMaskNode ──────────────────────────────────────────────────────────────

/// Mask `inputs[0]` using the BT.709 luma of `inputs[1]` (or `mask_rgba`).
///
/// The mask luma (0.0–1.0) is multiplied into the base alpha channel.
pub struct LumaMaskNode {
    /// Mask frame RGBA bytes (required for the CPU path).
    pub mask_rgba: Vec<u8>,
    /// Width of `mask_rgba`.
    pub mask_width: u32,
    /// Height of `mask_rgba`.
    pub mask_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<MaskPipeline>,
}

impl LumaMaskNode {
    #[must_use]
    pub fn new(mask_rgba: Vec<u8>, mask_width: u32, mask_height: u32) -> Self {
        Self {
            mask_rgba,
            mask_width,
            mask_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for LumaMaskNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.mask_rgba.len() != rgba.len() {
            return;
        }
        for (base, mask) in rgba.chunks_exact_mut(4).zip(self.mask_rgba.chunks_exact(4)) {
            let mr = f32::from(mask[0]) / 255.0;
            let mg = f32::from(mask[1]) / 255.0;
            let mb = f32::from(mask[2]) / 255.0;
            let luma = bt709_luma(mr, mg, mb);
            let ba = f32::from(base[3]) / 255.0;
            base[3] = ((ba * luma).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
}

#[cfg(feature = "wgpu")]
impl LumaMaskNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &MaskPipeline {
        self.pipeline.get_or_init(|| create_mask_pipeline(ctx))
    }
}

#[cfg(feature = "wgpu")]
impl crate::nodes::RenderNode for LumaMaskNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(base_tex) = inputs.first() else {
            log::warn!("LumaMaskNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("LumaMaskNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);
        let mask_tex = upload_rgba_texture(
            ctx,
            &self.mask_rgba,
            self.mask_width,
            self.mask_height,
            "LumaMask mask",
        );
        submit_mask_pass(ctx, pd, base_tex, &mask_tex, output, 1, "LumaMask BG");
    }
}

// ── AlphaMatteNode ────────────────────────────────────────────────────────────

/// Porter-Duff src-over: composite `inputs[0]` (foreground) over `inputs[1]`
/// (background) using the foreground's own alpha channel.
///
/// For the CPU path the background data must be stored in `background_rgba`.
pub struct AlphaMatteNode {
    /// Background frame RGBA bytes (required for the CPU path).
    pub background_rgba: Vec<u8>,
    /// Width of `background_rgba`.
    pub background_width: u32,
    /// Height of `background_rgba`.
    pub background_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<MaskPipeline>,
}

impl AlphaMatteNode {
    #[must_use]
    pub fn new(background_rgba: Vec<u8>, background_width: u32, background_height: u32) -> Self {
        Self {
            background_rgba,
            background_width,
            background_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for AlphaMatteNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.background_rgba.len() != rgba.len() {
            return;
        }
        for (fg, bg) in rgba
            .chunks_exact_mut(4)
            .zip(self.background_rgba.chunks_exact(4))
        {
            let fa = f32::from(fg[3]) / 255.0;
            let ba = f32::from(bg[3]) / 255.0;
            for ch in 0..3 {
                let fc = f32::from(fg[ch]) / 255.0;
                let bc = f32::from(bg[ch]) / 255.0;
                fg[ch] = ((fc * fa + bc * (1.0 - fa)).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            fg[3] = ((fa + ba * (1.0 - fa)).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
}

#[cfg(feature = "wgpu")]
impl AlphaMatteNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &MaskPipeline {
        self.pipeline.get_or_init(|| create_mask_pipeline(ctx))
    }
}

#[cfg(feature = "wgpu")]
impl crate::nodes::RenderNode for AlphaMatteNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(fg_tex) = inputs.first() else {
            log::warn!("AlphaMatteNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("AlphaMatteNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);
        let bg_tex = upload_rgba_texture(
            ctx,
            &self.background_rgba,
            self.background_width,
            self.background_height,
            "AlphaMatte bg",
        );
        submit_mask_pass(ctx, pd, fg_tex, &bg_tex, output, 2, "AlphaMatte BG");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::RenderNodeCpu;

    #[test]
    fn shape_mask_node_opaque_mask_should_keep_base_alpha() {
        let mask = vec![0u8, 0, 0, 255]; // fully opaque mask
        let node = ShapeMaskNode::new(mask, 1, 1);
        let mut rgba = vec![128u8, 128, 128, 200];
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            (rgba[3] as i32 - 200).abs() <= 1,
            "opaque mask must preserve base alpha"
        );
    }

    #[test]
    fn shape_mask_node_transparent_mask_should_zero_alpha() {
        let mask = vec![255u8, 255, 255, 0]; // fully transparent mask
        let node = ShapeMaskNode::new(mask, 1, 1);
        let mut rgba = vec![128u8, 128, 128, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba[3], 0, "transparent mask must produce zero alpha");
    }

    // ── LumaMaskNode ─────────────────────────────────────────────────────────

    #[test]
    fn luma_mask_node_white_mask_should_preserve_alpha() {
        let mask = vec![255u8, 255, 255, 255]; // white → luma = 1.0
        let node = LumaMaskNode::new(mask, 1, 1);
        let mut rgba = vec![100u8, 100, 100, 200];
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            (rgba[3] as i32 - 200).abs() <= 2,
            "white mask preserves alpha"
        );
    }

    #[test]
    fn luma_mask_node_black_mask_should_zero_alpha() {
        let mask = vec![0u8, 0, 0, 255]; // black → luma = 0.0
        let node = LumaMaskNode::new(mask, 1, 1);
        let mut rgba = vec![100u8, 100, 100, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba[3], 0, "black mask must zero out alpha");
    }

    // ── AlphaMatteNode ───────────────────────────────────────────────────────

    #[test]
    fn alpha_matte_node_opaque_fg_should_replace_background() {
        let bg = vec![50u8, 50, 50, 255];
        let node = AlphaMatteNode::new(bg, 1, 1);
        let mut fg = vec![200u8, 100, 50, 255]; // fully opaque fg
        node.process_cpu(&mut fg, 1, 1);
        assert!(
            (fg[0] as i32 - 200).abs() <= 1,
            "opaque fg must dominate; got {}",
            fg[0]
        );
    }

    #[test]
    fn alpha_matte_node_transparent_fg_should_show_background() {
        let bg = vec![50u8, 80, 120, 255];
        let node = AlphaMatteNode::new(bg, 1, 1);
        let mut fg = vec![200u8, 200, 200, 0]; // fully transparent fg
        node.process_cpu(&mut fg, 1, 1);
        assert!(
            (fg[0] as i32 - 50).abs() <= 1,
            "transparent fg must show bg; got {}",
            fg[0]
        );
    }
}
