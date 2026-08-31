//! Hue / saturation / lightness adjustment node.

use super::RenderNodeCpu;

#[cfg(feature = "wgpu")]
use super::blur::{create_uniform, fullscreen_pipeline, run_fullscreen, texture_entry};

// HslNode

/// Hue, saturation, and lightness adjustment.
pub struct HslNode {
    /// Hue rotation in degrees (−180 to +180; 0 = no change).
    pub hue_shift: f32,
    /// Saturation multiplier (0.0 = greyscale, 1.0 = unchanged, 2.0 = double).
    pub saturation: f32,
    /// Lightness offset (−1.0 to +1.0; 0 = no change).
    pub lightness: f32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<HslPipeline>,
}

impl HslNode {
    /// Creates an HSL adjustment node.
    #[must_use]
    pub fn new(hue_shift: f32, saturation: f32, lightness: f32) -> Self {
        Self {
            hue_shift,
            saturation,
            lightness,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for HslNode {
    /// Identity node (no HSL change).
    fn default() -> Self {
        Self::new(0.0, 1.0, 0.0)
    }
}

// Shared RGB <-> HSL (kept in sync with hsl.wgsl for GPU/CPU agreement).

/// Converts RGB (each in `[0, 1]`) to HSL with hue in `[0, 1)`.
// Exact `==` on the max channel is intentional: it picks which channel defines
// the hue sector, exactly as the WGSL shader does.
#[allow(clippy::float_cmp, clippy::many_single_char_names)]
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let l = f32::midpoint(mx, mn);
    let d = mx - mn;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let mut h = if mx == r {
        let hh = (g - b) / d;
        hh - 6.0 * (hh / 6.0).floor()
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h /= 6.0;
    (h, s, l)
}

fn hue_to_rgb(p: f32, q: f32, t_in: f32) -> f32 {
    let t = t_in - t_in.floor();
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// Converts HSL (hue in `[0, 1)`) back to RGB (each in `[0, 1]`).
#[allow(clippy::many_single_char_names)]
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s < 1e-6 {
        return (l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    (
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    )
}

// CPU path

impl RenderNodeCpu for HslNode {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::many_single_char_names
    )]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        for px in rgba.as_chunks_mut::<4>().0 {
            let (h, s, l) = rgb_to_hsl(
                f32::from(px[0]) / 255.0,
                f32::from(px[1]) / 255.0,
                f32::from(px[2]) / 255.0,
            );
            let h = h + self.hue_shift / 360.0;
            let s = (s * self.saturation).clamp(0.0, 1.0);
            let l = (l + self.lightness).clamp(0.0, 1.0);
            let (r, g, b) = hsl_to_rgb(h - h.floor(), s, l);
            px[0] = (r.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            px[1] = (g.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            px[2] = (b.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
struct HslPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
}

#[cfg(feature = "wgpu")]
impl HslNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &HslPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Hsl shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/hsl.wgsl").into()),
            });
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Hsl BGL"),
                entries: &[texture_entry(0), uniform_entry(1)],
            });
            let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "Hsl");
            let uniform_buf = create_uniform(device, "Hsl uniforms", 16);
            let mut bytes = [0u8; 16];
            bytes[0..4].copy_from_slice(&self.hue_shift.to_le_bytes());
            bytes[4..8].copy_from_slice(&self.saturation.to_le_bytes());
            bytes[8..12].copy_from_slice(&self.lightness.to_le_bytes());
            ctx.queue.write_buffer(&uniform_buf, 0, &bytes);
            HslPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for HslNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("HslNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("HslNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);
        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Hsl BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pd.uniform_buf.as_entire_binding(),
                },
            ],
        });
        run_fullscreen(
            ctx,
            &pd.render_pipeline,
            &bind_group,
            &output_view,
            "Hsl pass",
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(v: [u8; 3]) -> Vec<u8> {
        vec![v[0], v[1], v[2], 255]
    }

    #[test]
    fn hsl_hue_shift_180_should_invert_red_to_cyan() {
        let node = HslNode::new(180.0, 1.0, 0.0);
        let mut rgba = solid([255, 0, 0]); // pure red
        node.process_cpu(&mut rgba, 1, 1);
        // Red (H=0) rotated 180 deg is cyan (H=0.5): low R, high G and B.
        assert!(rgba[0] < 40, "R must drop for cyan; got {}", rgba[0]);
        assert!(rgba[1] > 215, "G must rise for cyan; got {}", rgba[1]);
        assert!(rgba[2] > 215, "B must rise for cyan; got {}", rgba[2]);
    }

    #[test]
    fn hsl_saturation_zero_should_greyscale() {
        let node = HslNode::new(0.0, 0.0, 0.0);
        let mut rgba = solid([200, 100, 50]);
        node.process_cpu(&mut rgba, 1, 1);
        let d_rg = (i32::from(rgba[0]) - i32::from(rgba[1])).abs();
        let d_rb = (i32::from(rgba[0]) - i32::from(rgba[2])).abs();
        assert!(
            d_rg <= 1 && d_rb <= 1,
            "saturation 0 must equalise channels"
        );
    }

    #[test]
    fn hsl_default_should_be_identity() {
        let node = HslNode::default();
        let original = solid([200, 100, 50]);
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        for (a, b) in rgba.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "default HSL must preserve the pixel; got {a} vs {b}"
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

    #[test]
    fn hsl_gpu_hue_shift_180_should_invert_red_to_cyan() {
        let Some(ctx) = ctx() else {
            return;
        };
        let frame = vec![255u8, 0, 0, 255];
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(HslNode::new(180.0, 1.0, 0.0))
            .process_gpu(&frame, 1, 1)
            .expect("gpu hsl");
        assert!(gpu[0] < 40, "R must drop for cyan; got {}", gpu[0]);
        assert!(gpu[1] > 215, "G must rise for cyan; got {}", gpu[1]);
        assert!(gpu[2] > 215, "B must rise for cyan; got {}", gpu[2]);
    }
}
