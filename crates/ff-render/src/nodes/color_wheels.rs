//! Three-way colour corrector (shadows/midtones/highlights lift, gamma, gain).

use super::RenderNodeCpu;

#[cfg(feature = "wgpu")]
use super::blur::{create_uniform, fullscreen_pipeline, run_fullscreen, texture_entry};

// ColorWheelsNode

/// Three-way colour corrector: shadows / midtones / highlights lift, gamma, gain.
///
/// Each adjustment is weighted by a luminance region so it acts on its tonal
/// range: `shadows_lift` (additive) on dark pixels, `midtones_gamma` on mid
/// pixels, `highlights_gain` (multiplicative) on bright pixels.
pub struct ColorWheelsNode {
    /// Shadows lift: additive offset per RGB channel (typ. `[-1, 1]`).
    pub shadows_lift: [f32; 3],
    /// Midtones gamma: exponent per RGB channel (typ. `[0.1, 10.0]`; `1.0` = no-op).
    pub midtones_gamma: [f32; 3],
    /// Highlights gain: multiplier per RGB channel (typ. `[0.0, 4.0]`; `1.0` = no-op).
    pub highlights_gain: [f32; 3],
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<ColorWheelsPipeline>,
}

impl ColorWheelsNode {
    /// Creates a three-way colour corrector.
    #[must_use]
    pub fn new(
        shadows_lift: [f32; 3],
        midtones_gamma: [f32; 3],
        highlights_gain: [f32; 3],
    ) -> Self {
        Self {
            shadows_lift,
            midtones_gamma,
            highlights_gain,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for ColorWheelsNode {
    /// Identity node (no lift, unit gamma, unit gain).
    fn default() -> Self {
        Self::new([0.0; 3], [1.0; 3], [1.0; 3])
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// CPU path

impl RenderNodeCpu for ColorWheelsNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        for px in rgba.as_chunks_mut::<4>().0 {
            let rgb = [
                f32::from(px[0]) / 255.0,
                f32::from(px[1]) / 255.0,
                f32::from(px[2]) / 255.0,
            ];
            let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            let shadow_w = 1.0 - smoothstep(0.0, 0.5, luma);
            let highlight_w = smoothstep(0.5, 1.0, luma);
            let mid_w = (1.0 - shadow_w - highlight_w).clamp(0.0, 1.0);

            for c in 0..3 {
                let mut v = rgb[c] + self.shadows_lift[c] * shadow_w;
                let gval = v.clamp(0.0, 1.0).powf(1.0 / self.midtones_gamma[c]);
                v += (gval - v) * mid_w;
                v *= 1.0 + (self.highlights_gain[c] - 1.0) * highlight_w;
                px[c] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
struct ColorWheelsPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
}

#[cfg(feature = "wgpu")]
impl ColorWheelsNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &ColorWheelsPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ColorWheels shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../shaders/color_wheels.wgsl").into(),
                ),
            });
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ColorWheels BGL"),
                entries: &[texture_entry(0), uniform_entry(1)],
            });
            let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "ColorWheels");
            // Three vec3 padded to 16 bytes each (std140).
            let uniform_buf = create_uniform(device, "ColorWheels uniforms", 48);
            let mut bytes = [0u8; 48];
            for (i, v) in self.shadows_lift.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            for (i, v) in self.midtones_gamma.iter().enumerate() {
                bytes[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            for (i, v) in self.highlights_gain.iter().enumerate() {
                bytes[32 + i * 4..32 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            ctx.queue.write_buffer(&uniform_buf, 0, &bytes);
            ColorWheelsPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for ColorWheelsNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("ColorWheelsNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("ColorWheelsNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);
        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ColorWheels BG"),
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
            "ColorWheels pass",
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

    #[test]
    fn color_wheels_default_should_be_noop() {
        let node = ColorWheelsNode::default();
        let original = vec![20u8, 20, 20, 255, 128, 128, 128, 255, 230, 230, 230, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 3, 1);
        for (a, b) in rgba.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "default colour wheels must preserve the pixel; got {a} vs {b}"
            );
        }
    }

    #[test]
    fn color_wheels_shadow_lift_should_tint_shadows() {
        // Magenta lift on shadows: R and B rise, G stays, on a dark pixel.
        let node = ColorWheelsNode::new([0.1, 0.0, 0.1], [1.0; 3], [1.0; 3]);
        let mut rgba = vec![20u8, 20, 20, 255]; // dark grey (shadow)
        node.process_cpu(&mut rgba, 1, 1);
        assert!(rgba[0] > 20, "shadow lift must raise R; got {}", rgba[0]);
        assert!(rgba[2] > 20, "shadow lift must raise B; got {}", rgba[2]);
        assert!(
            i32::from(rgba[1]) - 20 < i32::from(rgba[0]) - 20,
            "G must rise less than R (magenta tint)"
        );
    }

    #[test]
    fn color_wheels_shadow_lift_should_spare_highlights() {
        // The same lift must barely touch a bright pixel (highlight region).
        let node = ColorWheelsNode::new([0.1, 0.0, 0.1], [1.0; 3], [1.0; 3]);
        let mut rgba = vec![240u8, 240, 240, 255];
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            (i32::from(rgba[0]) - 240).abs() <= 3,
            "shadow lift must not tint highlights; got {}",
            rgba[0]
        );
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
    fn color_wheels_gpu_shadow_lift_should_tint_shadows() {
        let Some(ctx) = ctx() else {
            return;
        };
        let frame = vec![20u8, 20, 20, 255];
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(ColorWheelsNode::new([0.1, 0.0, 0.1], [1.0; 3], [1.0; 3]))
            .process_gpu(&frame, 1, 1)
            .expect("gpu color wheels");
        assert!(gpu[0] > 20, "shadow lift must raise R; got {}", gpu[0]);
        assert!(gpu[2] > 20, "shadow lift must raise B; got {}", gpu[2]);
    }
}
