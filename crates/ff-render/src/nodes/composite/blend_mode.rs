//! `BlendMode` enum and `BlendModeNode` (CPU + GPU Photoshop-style blending).

use super::blend_math::blend_rgb;
#[cfg(feature = "wgpu")]
use super::helpers::{
    fullscreen_pipeline, linear_sampler, submit_render_pass, two_tex_sampler_uniform_bgl,
    upload_rgba_texture,
};
use crate::nodes::RenderNodeCpu;

// ── BlendMode ─────────────────────────────────────────────────────────────────

/// Photoshop-compatible blend modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum BlendMode {
    /// Overlay replaces base.
    #[default]
    Normal = 0,
    /// base × overlay.
    Multiply = 1,
    /// 1 − (1−base)(1−overlay).
    Screen = 2,
    /// Multiply below 50% grey, Screen above.
    Overlay = 3,
    /// Soft light — W3C formula.
    SoftLight = 4,
    /// Hard light — Overlay with base/overlay swapped.
    HardLight = 5,
    /// base / (1 − overlay).
    ColorDodge = 6,
    /// 1 − (1−base) / overlay.
    ColorBurn = 7,
    /// |base − overlay|.
    Difference = 8,
    /// base + overlay − 2·base·overlay.
    Exclusion = 9,
    /// clamp(base + overlay, 0, 1).
    Add = 10,
    /// clamp(base − overlay, 0, 1).
    Subtract = 11,
    /// min(base, overlay).
    Darken = 12,
    /// max(base, overlay).
    Lighten = 13,
    /// Overlay hue + base saturation + base lightness.
    Hue = 14,
    /// Base hue + overlay saturation + base lightness.
    Saturation = 15,
    /// Overlay hue + overlay saturation + base lightness.
    Color = 16,
    /// Base hue + base saturation + overlay lightness.
    Luminosity = 17,
}

// ── BlendModeNode ─────────────────────────────────────────────────────────────

#[cfg(feature = "wgpu")]
struct BlendPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

/// Apply a Photoshop-compatible blend mode to two input textures.
///
/// `input_count() = 2` — `inputs[0]` is the base layer, `inputs[1]` is the
/// overlay.  The `opacity` field attenuates the overlay's contribution.
///
/// For the CPU path the overlay data must be stored in `overlay_rgba`.
pub struct BlendModeNode {
    /// Blend algorithm.
    pub mode: BlendMode,
    /// Overlay opacity (0.0 = invisible, 1.0 = fully applied).
    pub opacity: f32,
    /// Overlay frame as RGBA bytes (required for CPU path).
    pub overlay_rgba: Vec<u8>,
    /// Width of `overlay_rgba`.
    pub overlay_width: u32,
    /// Height of `overlay_rgba`.
    pub overlay_height: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<BlendPipeline>,
}

impl BlendModeNode {
    #[must_use]
    pub fn new(
        mode: BlendMode,
        opacity: f32,
        overlay_rgba: Vec<u8>,
        overlay_width: u32,
        overlay_height: u32,
    ) -> Self {
        Self {
            mode,
            opacity,
            overlay_rgba,
            overlay_width,
            overlay_height,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl RenderNodeCpu for BlendModeNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        if self.overlay_rgba.len() != rgba.len() {
            log::warn!(
                "BlendModeNode::process_cpu skipped: size mismatch base={} overlay={}",
                rgba.len(),
                self.overlay_rgba.len()
            );
            return;
        }
        for (base, ov) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.overlay_rgba.as_chunks::<4>().0.iter())
        {
            let br = f32::from(base[0]) / 255.0;
            let bg = f32::from(base[1]) / 255.0;
            let bb = f32::from(base[2]) / 255.0;
            let or = f32::from(ov[0]) / 255.0;
            let og = f32::from(ov[1]) / 255.0;
            let ob = f32::from(ov[2]) / 255.0;
            let oa = f32::from(ov[3]) / 255.0;

            let [rr, rg, rb] = blend_rgb(self.mode, [br, bg, bb], [or, og, ob]);
            let eff_alpha = oa * self.opacity;
            let out_r = (br + (rr - br) * eff_alpha).clamp(0.0, 1.0);
            let out_g = (bg + (rg - bg) * eff_alpha).clamp(0.0, 1.0);
            let out_b = (bb + (rb - bb) * eff_alpha).clamp(0.0, 1.0);
            base[0] = (out_r * 255.0 + 0.5) as u8;
            base[1] = (out_g * 255.0 + 0.5) as u8;
            base[2] = (out_b * 255.0 + 0.5) as u8;
        }
    }
}

// ── GPU: BlendModeNode ────────────────────────────────────────────────────────

#[cfg(feature = "wgpu")]
impl BlendModeNode {
    #[allow(clippy::too_many_lines)]
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &BlendPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Blend shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/blend.wgsl").into()),
            });
            let bgl = two_tex_sampler_uniform_bgl(device, "Blend");
            let render_pipeline = fullscreen_pipeline(device, &shader, "Blend", &bgl);
            let sampler = linear_sampler(device, "Blend");
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blend uniforms"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            BlendPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                sampler,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl crate::nodes::RenderNode for BlendModeNode {
    fn input_count(&self) -> usize {
        2
    }

    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(tex_base) = inputs.first() else {
            log::warn!("BlendModeNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("BlendModeNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);

        // Upload overlay frame.
        let ov_tex = upload_rgba_texture(
            ctx,
            &self.overlay_rgba,
            self.overlay_width,
            self.overlay_height,
            "Blend overlay",
        );

        // Write uniforms: [mode_u32, opacity_f32, pad, pad] = 16 bytes.
        let mode_bytes = (self.mode as u32).to_le_bytes();
        let opac_bytes = self.opacity.to_le_bytes();
        let uniforms: [u8; 16] = [
            mode_bytes[0],
            mode_bytes[1],
            mode_bytes[2],
            mode_bytes[3],
            opac_bytes[0],
            opac_bytes[1],
            opac_bytes[2],
            opac_bytes[3],
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

        let base_view = tex_base.create_view(&wgpu::TextureViewDescriptor::default());
        let ov_view = ov_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blend BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ov_view),
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

        submit_render_pass(ctx, &pd.render_pipeline, &bind_group, &out_view, "Blend");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::RenderNodeCpu;

    #[test]
    fn blend_mode_multiply_should_produce_product_of_base_and_overlay() {
        // 50% grey × 50% grey = 25% grey (pixel-exact per acceptance criteria).
        let grey50 = vec![128u8, 128, 128, 255];
        let node = BlendModeNode::new(BlendMode::Multiply, 1.0, grey50.clone(), 1, 1);
        let mut rgba = grey50;
        node.process_cpu(&mut rgba, 1, 1);
        // 128/255 * 128/255 * 255 ≈ 64.25 → 64 or 65.
        let expected = (128.0_f32 / 255.0 * 128.0 / 255.0 * 255.0 + 0.5) as u8;
        let diff = (rgba[0] as i32 - expected as i32).abs();
        assert!(
            diff <= 1,
            "Multiply 50%×50% grey: expected ~{expected}, got {}",
            rgba[0]
        );
    }

    #[test]
    fn blend_mode_screen_should_be_brighter_than_either_input() {
        let base = vec![100u8, 100, 100, 255];
        let overlay = vec![150u8, 150, 150, 255];
        let node = BlendModeNode::new(BlendMode::Screen, 1.0, overlay, 1, 1);
        let mut rgba = base;
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[0] > 150,
            "Screen must be brighter than max input; got {}",
            rgba[0]
        );
    }

    #[test]
    fn blend_mode_normal_at_full_opacity_should_replace_base_with_overlay() {
        let base = vec![50u8, 50, 50, 255];
        let overlay = vec![200u8, 100, 50, 255];
        let node = BlendModeNode::new(BlendMode::Normal, 1.0, overlay, 1, 1);
        let mut rgba = base;
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            (rgba[0] as i32 - 200).abs() <= 1,
            "R should match overlay; got {}",
            rgba[0]
        );
        assert!(
            (rgba[1] as i32 - 100).abs() <= 1,
            "G should match overlay; got {}",
            rgba[1]
        );
    }

    #[test]
    fn blend_mode_normal_at_zero_opacity_should_leave_base_unchanged() {
        let base = vec![50u8, 80, 120, 255];
        let overlay = vec![200u8, 200, 200, 255];
        let node = BlendModeNode::new(BlendMode::Normal, 0.0, overlay, 1, 1);
        let mut rgba = base.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            (rgba[0] as i32 - 50).abs() <= 1,
            "R should match base; got {}",
            rgba[0]
        );
    }

    #[test]
    fn blend_mode_difference_of_equal_pixels_should_be_black() {
        let grey = vec![128u8, 128, 128, 255];
        let node = BlendModeNode::new(BlendMode::Difference, 1.0, grey.clone(), 1, 1);
        let mut rgba = grey;
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[0] <= 1,
            "Difference of same pixel must be ~black; got {}",
            rgba[0]
        );
    }

    #[test]
    fn blend_mode_add_should_clamp_at_white() {
        let bright = vec![200u8, 200, 200, 255];
        let node = BlendModeNode::new(BlendMode::Add, 1.0, bright.clone(), 1, 1);
        let mut rgba = bright;
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba[0], 255, "Add of two bright values must clamp to 255");
    }

    #[test]
    fn blend_mode_darken_should_return_minimum_channel() {
        let base = vec![100u8, 200, 50, 255];
        let overlay = vec![150u8, 50, 100, 255];
        let node = BlendModeNode::new(BlendMode::Darken, 1.0, overlay, 1, 1);
        let mut rgba = base;
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            (rgba[0] as i32 - 100).abs() <= 1,
            "Darken R: min(100,150)=100; got {}",
            rgba[0]
        );
        assert!(
            (rgba[1] as i32 - 50).abs() <= 1,
            "Darken G: min(200,50)=50; got {}",
            rgba[1]
        );
        assert!(
            (rgba[2] as i32 - 50).abs() <= 1,
            "Darken B: min(50,100)=50; got {}",
            rgba[2]
        );
    }

    #[test]
    fn blend_mode_size_mismatch_should_be_noop() {
        let overlay = vec![200u8; 8];
        let node = BlendModeNode::new(BlendMode::Normal, 1.0, overlay, 2, 1);
        let original = vec![50u8, 80, 120, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "size mismatch must leave base unchanged");
    }

    #[test]
    fn all_blend_mode_variants_should_compile() {
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::SoftLight,
            BlendMode::HardLight,
            BlendMode::ColorDodge,
            BlendMode::ColorBurn,
            BlendMode::Difference,
            BlendMode::Exclusion,
            BlendMode::Add,
            BlendMode::Subtract,
            BlendMode::Darken,
            BlendMode::Lighten,
            BlendMode::Hue,
            BlendMode::Saturation,
            BlendMode::Color,
            BlendMode::Luminosity,
        ];
        assert_eq!(modes.len(), 18);
    }
}
