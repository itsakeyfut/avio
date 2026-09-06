//! Mask nodes: `ShapeMaskNode`, `LumaMaskNode`, `AlphaMatteNode` + shared pipeline.

use super::chroma_key::bt709_luma;
#[cfg(feature = "wgpu")]
use super::helpers::{
    fullscreen_pipeline, linear_sampler, submit_render_pass, two_tex_sampler_uniform_bgl,
    upload_rgba_texture,
};
use crate::nodes::RenderNodeCpu;

// Shared mask pipeline

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
        // `mode` + `invert` + the source size, then the rectangle as a `vec4<f32>`
        // (16-byte aligned, hence the 32).
        size: 32,
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

/// The `MaskUniforms` block `mask.wgsl` reads, in its own layout order.
#[cfg(feature = "wgpu")]
#[derive(Default, Clone, Copy)]
struct MaskUniforms {
    /// 0 = `ShapeMask`, 1 = `LumaMask`, 2 = `AlphaMatte`.
    mode: u32,
    invert: u32,
    /// The source frame's size, so `rect` can be given in its pixels.
    src: (f32, f32),
    /// `x, y, x_end, y_end` in source pixels. Unused outside `ShapeMask`.
    rect: [f32; 4],
}

#[cfg(feature = "wgpu")]
impl MaskUniforms {
    /// The 32 bytes the shader expects. Written by hand rather than through
    /// `bytemuck` because the struct is private and this is its only use.
    fn to_bytes(self) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[0..4].copy_from_slice(&self.mode.to_le_bytes());
        out[4..8].copy_from_slice(&self.invert.to_le_bytes());
        out[8..12].copy_from_slice(&self.src.0.to_le_bytes());
        out[12..16].copy_from_slice(&self.src.1.to_le_bytes());
        for (i, v) in self.rect.iter().enumerate() {
            let at = 16 + i * 4;
            out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
}

#[cfg(feature = "wgpu")]
fn submit_mask_pass(
    ctx: &crate::context::RenderContext,
    pd: &MaskPipeline,
    base_tex: &wgpu::Texture,
    mask_tex: &wgpu::Texture,
    output_tex: &wgpu::Texture,
    uniforms: MaskUniforms,
    label: &str,
) {
    ctx.queue
        .write_buffer(&pd.uniform_buf, 0, &uniforms.to_bytes());

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

// ShapeMaskNode

/// Clear the alpha of `inputs[0]` outside a rectangle of the source frame.
///
/// Pixels inside the rectangle keep their alpha; all others are made fully
/// transparent. The rectangle reaches the shader as a uniform, so the node holds no
/// mask buffer and uploads nothing per frame (#1710).
pub struct ShapeMaskNode {
    /// `x, y, width, height` in source-frame pixels, and `invert`.
    ///
    /// A `Cell` so an animated rectangle can be applied to the live node
    /// ([`NodeParam::ShapeMaskRect`](crate::NodeParam::ShapeMaskRect)) instead of
    /// rebuilding the graph around it, which would recreate the render pipeline every
    /// frame. `Cell` keeps the node `Send`, which is all `RenderNodeCpu` requires.
    rect: std::cell::Cell<(u32, u32, u32, u32)>,
    invert: std::cell::Cell<bool>,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<MaskPipeline>,
}

impl ShapeMaskNode {
    /// Keeps the pixels inside `[x, x + width) x [y, y + height)` of the **source**
    /// frame, or those outside it when `invert`.
    #[must_use]
    pub fn new(x: u32, y: u32, width: u32, height: u32, invert: bool) -> Self {
        Self {
            rect: std::cell::Cell::new((x, y, width, height)),
            invert: std::cell::Cell::new(invert),
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }

    /// Whether pixel `(px, py)` of the source frame is kept.
    fn keeps(&self, px: u32, py: u32) -> bool {
        let (x, y, width, height) = self.rect.get();
        let inside =
            px >= x && px < x.saturating_add(width) && py >= y && py < y.saturating_add(height);
        inside != self.invert.get()
    }
}

impl RenderNodeCpu for ShapeMaskNode {
    /// Tests the rectangle against the coordinates of the buffer it is given.
    ///
    /// Those are the previous node's output pixels, where the GPU path evaluates the
    /// rectangle in the *original* source frame's pixels. The two agree until a node
    /// that resizes (a `ScaleNode`) runs in front of this one.
    fn process_cpu(&self, rgba: &mut [u8], w: u32, _h: u32) {
        if w == 0 {
            return;
        }
        // Walked rather than derived from the index, so no `usize -> u32` cast is
        // needed for a coordinate that is a `u32` by construction.
        let (mut px, mut py) = (0u32, 0u32);
        for base in rgba.as_chunks_mut::<4>().0 {
            if !self.keeps(px, py) {
                base[3] = 0;
            }
            px += 1;
            if px == w {
                px = 0;
                py += 1;
            }
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

    /// Takes [`NodeParam::ShapeMaskRect`](crate::NodeParam::ShapeMaskRect), so an
    /// animated rectangle moves without the graph being rebuilt around it.
    fn set_param(&self, param: crate::nodes::NodeParam) -> bool {
        match param {
            crate::nodes::NodeParam::ShapeMaskRect {
                x,
                y,
                width,
                height,
                invert,
            } => {
                self.rect.set((x, y, width, height));
                self.invert.set(invert);
                true
            }
            crate::nodes::NodeParam::MotionBlurShutter(_) => false,
        }
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
        // `inputs[1]` is the original source frame, which is the space the rectangle
        // is expressed in. It is bound only to satisfy the shared bind-group layout:
        // mode 0 evaluates the rectangle and never samples it.
        //
        // `input_count()` is 2, so the executor always supplies it. Falling back to the
        // chained texture would put the rectangle in a different pixel space whenever a
        // node resized in front of this one, so say it rather than absorb it silently.
        let source = inputs.get(1).copied().unwrap_or_else(|| {
            log::warn!("ShapeMaskNode::process called without the source frame");
            base_tex
        });
        let pd = self.get_or_create_pipeline(ctx);
        let (x, y, width, height) = self.rect.get();
        #[allow(clippy::cast_precision_loss)]
        let uniforms = MaskUniforms {
            mode: 0,
            invert: u32::from(self.invert.get()),
            src: (source.width() as f32, source.height() as f32),
            rect: [
                x as f32,
                y as f32,
                x.saturating_add(width) as f32,
                y.saturating_add(height) as f32,
            ],
        };
        submit_mask_pass(ctx, pd, base_tex, source, output, uniforms, "ShapeMask BG");
    }
}

// LumaMaskNode

/// Mask `inputs[0]` using the BT.709 luma of the source frame (`inputs[1]`).
///
/// The luma (0.0–1.0) is multiplied into the base alpha channel. The source frame is
/// sampled per frame rather than baked into the node (#1710), which is what lets an
/// effect graph containing this node be cached across frames.
pub struct LumaMaskNode {
    /// Use `1 - luma` instead of `luma`.
    invert: bool,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<MaskPipeline>,
}

impl LumaMaskNode {
    /// Masks by the source frame's own BT.709 luma, or its complement when `invert`.
    ///
    /// The node holds no mask: the GPU path samples the source frame directly, so
    /// there is nothing to build or upload per frame.
    #[must_use]
    pub fn new(invert: bool) -> Self {
        Self {
            invert,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for LumaMaskNode {
    /// Masks by the luma of the buffer it is given.
    ///
    /// That buffer is the previous node's output, where the GPU path uses the
    /// *original* source frame. The two agree when this node is first in the graph;
    /// an effect in front of it makes them differ. The compositor drives only the GPU
    /// path, and its own divergence from the CPU `geq` in that position is a known v1
    /// limitation (see `avio::gpu_compositor`'s `LumaMask` arm).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        for base in rgba.as_chunks_mut::<4>().0 {
            let mr = f32::from(base[0]) / 255.0;
            let mg = f32::from(base[1]) / 255.0;
            let mb = f32::from(base[2]) / 255.0;
            let luma = bt709_luma(mr, mg, mb);
            let opacity = if self.invert { 1.0 - luma } else { luma };
            let ba = f32::from(base[3]) / 255.0;
            base[3] = ((ba * opacity).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
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
        // `inputs[1]` is the original source frame, which *is* the mask: the shader
        // takes its BT.709 luma. Nothing is built or uploaded per frame.
        //
        // `input_count()` is 2, so the executor always supplies it; the fallback would
        // mask by the chained frame instead of the source.
        let source = inputs.get(1).copied().unwrap_or_else(|| {
            log::warn!("LumaMaskNode::process called without the source frame");
            base_tex
        });
        let pd = self.get_or_create_pipeline(ctx);
        let uniforms = MaskUniforms {
            mode: 1,
            invert: u32::from(self.invert),
            ..MaskUniforms::default()
        };
        submit_mask_pass(ctx, pd, base_tex, source, output, uniforms, "LumaMask BG");
    }
}

// AlphaMatteNode

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
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.background_rgba.as_chunks::<4>().0.iter())
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
        let uniforms = MaskUniforms {
            mode: 2,
            ..MaskUniforms::default()
        };
        submit_mask_pass(ctx, pd, fg_tex, &bg_tex, output, uniforms, "AlphaMatte BG");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::RenderNodeCpu;

    #[test]
    fn shape_mask_node_should_keep_a_pixel_inside_the_rectangle() {
        // The rectangle covers the only pixel, so its alpha survives.
        let node = ShapeMaskNode::new(0, 0, 1, 1, false);
        let mut rgba = vec![128u8, 128, 128, 200];
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            i32::from(rgba[3]).abs_diff(200) <= 1,
            "a pixel inside the rectangle keeps its alpha"
        );
    }

    #[test]
    fn shape_mask_node_should_drop_a_pixel_outside_the_rectangle() {
        // A rectangle of zero width covers nothing.
        let node = ShapeMaskNode::new(0, 0, 0, 0, false);
        let mut rgba = vec![128u8, 128, 128, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba[3], 0, "a pixel outside the rectangle is dropped");
    }

    #[test]
    fn shape_mask_node_invert_should_swap_inside_for_outside() {
        // The other half of the gate above, so the rectangle test is not read as a
        // blanket keep-everything.
        let node = ShapeMaskNode::new(0, 0, 1, 1, true);
        let mut rgba = vec![128u8, 128, 128, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(
            rgba[3], 0,
            "inverted, a pixel inside the rectangle is dropped"
        );
    }

    /// A 4x2 opaque base. The rectangle below covers its left half, so a per-region
    /// assertion pins spatial selection (a 1x1 test cannot).
    fn tagged_base() -> (Vec<u8>, u32, u32) {
        let (w, h) = (4u32, 2u32);
        let mut base = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for _ in 0..w {
                base.extend_from_slice(&[100, 100, 100, 255]); // opaque grey
            }
        }
        (base, w, h)
    }

    fn alpha_at(rgba: &[u8], w: u32, x: u32, y: u32) -> u8 {
        rgba[((y * w + x) * 4 + 3) as usize]
    }

    #[test]
    fn shape_mask_node_should_keep_masked_region_only() {
        let (mut base, w, h) = tagged_base();
        // x in [0, 2), all rows.
        ShapeMaskNode::new(0, 0, 2, h, false).process_cpu(&mut base, w, h);
        assert!(alpha_at(&base, w, 0, 0) > 200, "kept (0,0) preserves alpha");
        assert!(alpha_at(&base, w, 1, 1) > 200, "kept (1,1) preserves alpha");
        assert!(alpha_at(&base, w, 2, 0) < 30, "dropped (2,0) zeroes alpha");
        assert!(alpha_at(&base, w, 3, 1) < 30, "dropped (3,1) zeroes alpha");
    }

    // LumaMaskNode

    #[test]
    fn luma_mask_node_white_should_preserve_alpha() {
        // The mask is the frame's own luma, so a white pixel is fully opaque.
        let node = LumaMaskNode::new(false);
        let mut rgba = vec![255u8, 255, 255, 200];
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            i32::from(rgba[3]).abs_diff(200) <= 2,
            "white preserves alpha, got {}",
            rgba[3]
        );
    }

    #[test]
    fn luma_mask_node_black_should_zero_alpha() {
        let node = LumaMaskNode::new(false);
        let mut rgba = vec![0u8, 0, 0, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba[3], 0, "black must zero out alpha");
    }

    #[test]
    fn luma_mask_node_invert_should_swap_light_for_dark() {
        // The other half of the two above: inverted, white is what disappears.
        let node = LumaMaskNode::new(true);
        let mut white = vec![255u8, 255, 255, 255];
        node.process_cpu(&mut white, 1, 1);
        assert_eq!(white[3], 0, "inverted, white zeroes alpha");
        let node = LumaMaskNode::new(true);
        let mut black = vec![0u8, 0, 0, 200];
        node.process_cpu(&mut black, 1, 1);
        assert!(
            i32::from(black[3]).abs_diff(200) <= 2,
            "inverted, black preserves alpha, got {}",
            black[3]
        );
    }

    #[test]
    fn luma_mask_node_should_mask_by_region_luma() {
        // White left half, black right half, all opaque.
        let (w, h) = (4u32, 2u32);
        let mut base = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                let v = if x < 2 { 255 } else { 0 };
                base.extend_from_slice(&[v, v, v, 255]);
            }
        }
        LumaMaskNode::new(false).process_cpu(&mut base, w, h);
        assert!(
            alpha_at(&base, w, 0, 0) > 200,
            "white (0,0) preserves alpha"
        );
        assert!(
            alpha_at(&base, w, 1, 1) > 200,
            "white (1,1) preserves alpha"
        );
        assert!(alpha_at(&base, w, 2, 0) < 30, "black (2,0) zeroes alpha");
        assert!(alpha_at(&base, w, 3, 1) < 30, "black (3,1) zeroes alpha");
    }

    // AlphaMatteNode

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

    /// An opaque base whose left half is white and right half black.
    ///
    /// The frame *is* the mask now: the shader reads the source frame the executor
    /// binds as the second input rather than a buffer the node carries.
    fn luma_tagged_frame() -> (Vec<u8>, u32, u32) {
        let (w, h) = (4u32, 2u32);
        let mut base = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                if x < 2 {
                    base.extend_from_slice(&[255, 255, 255, 255]);
                } else {
                    base.extend_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
        (base, w, h)
    }

    fn alpha_at(rgba: &[u8], w: u32, x: u32, y: u32) -> u8 {
        rgba[((y * w + x) * 4 + 3) as usize]
    }

    #[test]
    fn luma_mask_gpu_should_mask_by_region_luma_on_tagged_fixture() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (base, w, h) = luma_tagged_frame();
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(LumaMaskNode::new(false))
            .process_gpu(&base, w, h)
            .expect("gpu luma mask");

        // Bright region -> alpha preserved; dark region -> zeroed (validates the
        // mask.wgsl luma branch).
        assert!(
            alpha_at(&out, w, 0, 0) > 200,
            "bright (0,0) preserves alpha on GPU"
        );
        assert!(
            alpha_at(&out, w, 1, 1) > 200,
            "bright (1,1) preserves alpha on GPU"
        );
        assert!(
            alpha_at(&out, w, 2, 0) < 30,
            "dark (2,0) zeroes alpha on GPU"
        );
        assert!(
            alpha_at(&out, w, 3, 1) < 30,
            "dark (3,1) zeroes alpha on GPU"
        );
    }

    #[test]
    fn luma_mask_gpu_inverted_should_keep_the_dark_region() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (base, w, h) = luma_tagged_frame();
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(LumaMaskNode::new(true))
            .process_gpu(&base, w, h)
            .expect("gpu luma mask");

        assert!(
            alpha_at(&out, w, 0, 0) < 30,
            "inverted: bright (0,0) is dropped on GPU"
        );
        assert!(
            alpha_at(&out, w, 3, 1) > 200,
            "inverted: dark (3,1) is kept on GPU"
        );
    }

    /// The mask must come from the **source** frame, not from whatever the chain has
    /// made of it by the time this node runs.
    ///
    /// That is the behaviour the baked mask had (it was built from the pre-graph
    /// frame), so keeping it is what makes the shader a drop-in for it. A node bound to
    /// `inputs[0]` instead of `inputs[1]` looks correct whenever the mask is the only
    /// effect -- every parity test here is that shape -- so this one puts an effect in
    /// front of it that destroys the luma.
    #[test]
    fn luma_mask_gpu_should_mask_by_the_source_frame_not_the_chained_one() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (base, w, h) = luma_tagged_frame();
        let out = RenderGraph::new(Arc::clone(&ctx))
            // Brightness -1.0 drives every pixel to black, so a mask taken from the
            // chained texture would hide the whole frame.
            .push(crate::nodes::ColorGradeNode::new(-1.0, 1.0, 1.0, 0.0, 0.0))
            .push(LumaMaskNode::new(false))
            .process_gpu(&base, w, h)
            .expect("gpu luma mask");

        assert!(
            alpha_at(&out, w, 0, 0) > 200,
            "the source frame's bright half must still be kept, got {}",
            alpha_at(&out, w, 0, 0)
        );
        assert!(
            alpha_at(&out, w, 3, 1) < 30,
            "the source frame's dark half must still be dropped, got {}",
            alpha_at(&out, w, 3, 1)
        );
    }

    #[test]
    fn shape_mask_gpu_should_keep_the_rectangle_on_tagged_fixture() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (4u32, 2u32);
        let base: Vec<u8> = std::iter::repeat_n([100u8, 100, 100, 255], (w * h) as usize)
            .flatten()
            .collect();
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(ShapeMaskNode::new(0, 0, 2, h, false))
            .process_gpu(&base, w, h)
            .expect("gpu shape mask");

        // Inside the rectangle -> alpha preserved; outside -> zeroed (validates the
        // mask.wgsl shape branch).
        assert!(
            alpha_at(&out, w, 0, 0) > 200,
            "kept (0,0) preserves alpha on GPU"
        );
        assert!(
            alpha_at(&out, w, 1, 1) > 200,
            "kept (1,1) preserves alpha on GPU"
        );
        assert!(
            alpha_at(&out, w, 2, 0) < 30,
            "dropped (2,0) zeroes alpha on GPU"
        );
        assert!(
            alpha_at(&out, w, 3, 1) < 30,
            "dropped (3,1) zeroes alpha on GPU"
        );
    }

    #[test]
    fn shape_mask_gpu_inverted_should_keep_the_outside() {
        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (4u32, 2u32);
        let base: Vec<u8> = std::iter::repeat_n([100u8, 100, 100, 255], (w * h) as usize)
            .flatten()
            .collect();
        let out = RenderGraph::new(Arc::clone(&ctx))
            .push(ShapeMaskNode::new(0, 0, 2, h, true))
            .process_gpu(&base, w, h)
            .expect("gpu shape mask");

        assert!(
            alpha_at(&out, w, 0, 0) < 30,
            "inverted: inside (0,0) is dropped on GPU"
        );
        assert!(
            alpha_at(&out, w, 3, 1) > 200,
            "inverted: outside (3,1) is kept on GPU"
        );
    }
}
