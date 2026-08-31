//! Per-channel tone-curve node using a precomputed Monotone Cubic (Steffen) LUT.

use super::RenderNodeCpu;

#[cfg(feature = "wgpu")]
use super::blur::{fullscreen_pipeline, run_fullscreen, texture_entry};

/// Number of LUT samples per curve (matches the 256-wide LUT texture).
const LUT_SIZE: usize = 256;

// CurvesNode

/// Per-channel tone curve adjustment via a precomputed 256-sample LUT.
///
/// Each curve is defined by control points `[input, output]` in `[0, 1]` and
/// interpolated with the Steffen (1990) monotone cubic method (no overshoot).
/// The master curve is applied to each channel first, then the per-channel curve.
pub struct CurvesNode {
    /// Control points for the master curve (applied to every channel).
    pub master: Vec<[f32; 2]>,
    /// Control points for the red channel curve.
    pub red: Vec<[f32; 2]>,
    /// Control points for the green channel curve.
    pub green: Vec<[f32; 2]>,
    /// Control points for the blue channel curve.
    pub blue: Vec<[f32; 2]>,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<CurvesPipeline>,
}

impl CurvesNode {
    /// Creates a curves node from the four channels' control points.
    #[must_use]
    pub fn new(
        master: Vec<[f32; 2]>,
        red: Vec<[f32; 2]>,
        green: Vec<[f32; 2]>,
        blue: Vec<[f32; 2]>,
    ) -> Self {
        Self {
            master,
            red,
            green,
            blue,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for CurvesNode {
    /// Identity node (linear `[(0,0),(1,1)]` on every channel).
    fn default() -> Self {
        let identity = || vec![[0.0, 0.0], [1.0, 1.0]];
        Self::new(identity(), identity(), identity(), identity())
    }
}

fn sgn(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Builds a `LUT_SIZE`-entry LUT (evenly spaced outputs over `[0, 1]`) from the
/// control points using Steffen monotone cubic interpolation.
///
/// Points are sanitised first (clamped to `[0, 1]`, sorted by input, duplicate
/// inputs dropped); fewer than two valid points falls back to identity, so any
/// input, including non-monotone points, produces a LUT without panicking.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn build_lut(control_points: &[[f32; 2]], samples: usize) -> Vec<f32> {
    // Sanitise: clamp into the unit square, sort by input, drop duplicate inputs.
    let mut pts: Vec<[f32; 2]> = control_points
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
        .collect();
    pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);

    let identity = |i: usize| i as f32 / (samples - 1) as f32;
    if pts.len() < 2 {
        return (0..samples).map(identity).collect();
    }

    let n = pts.len();
    let x: Vec<f32> = pts.iter().map(|p| p[0]).collect();
    let y: Vec<f32> = pts.iter().map(|p| p[1]).collect();
    let h: Vec<f32> = (0..n - 1).map(|i| x[i + 1] - x[i]).collect();
    let s: Vec<f32> = (0..n - 1).map(|i| (y[i + 1] - y[i]) / h[i]).collect();

    // Steffen slopes at each control point.
    let mut yp = vec![0.0f32; n];
    if n == 2 {
        yp[0] = s[0];
        yp[1] = s[0];
    } else {
        for i in 1..n - 1 {
            let p = (s[i - 1] * h[i] + s[i] * h[i - 1]) / (h[i - 1] + h[i]);
            yp[i] = (sgn(s[i - 1]) + sgn(s[i])) * s[i - 1].abs().min(s[i].abs()).min(0.5 * p.abs());
        }
        // Steffen one-sided endpoint derivatives.
        let p0 = s[0] * (1.0 + h[0] / (h[0] + h[1])) - s[1] * (h[0] / (h[0] + h[1]));
        yp[0] = if p0 * s[0] <= 0.0 {
            0.0
        } else if p0.abs() > 2.0 * s[0].abs() {
            2.0 * s[0]
        } else {
            p0
        };
        let (a, b) = (h[n - 2], h[n - 3]);
        let pn = s[n - 2] * (1.0 + a / (a + b)) - s[n - 3] * (a / (a + b));
        yp[n - 1] = if pn * s[n - 2] <= 0.0 {
            0.0
        } else if pn.abs() > 2.0 * s[n - 2].abs() {
            2.0 * s[n - 2]
        } else {
            pn
        };
    }

    (0..samples)
        .map(|i| {
            let xi = identity(i);
            if xi <= x[0] {
                return y[0];
            }
            if xi >= x[n - 1] {
                return y[n - 1];
            }
            // Locate the interval containing xi.
            let mut k = 0;
            while k < n - 1 && xi > x[k + 1] {
                k += 1;
            }
            let t = (xi - x[k]) / h[k];
            let t2 = t * t;
            let t3 = t2 * t;
            let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
            let h10 = t3 - 2.0 * t2 + t;
            let h01 = -2.0 * t3 + 3.0 * t2;
            let h11 = t3 - t2;
            (h00 * y[k] + h10 * h[k] * yp[k] + h01 * y[k + 1] + h11 * h[k] * yp[k + 1])
                .clamp(0.0, 1.0)
        })
        .collect()
}

/// Nearest LUT index for a normalised value (matches `lut_idx` in curves.wgsl).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lut_idx(v: f32) -> usize {
    ((v * 255.0 + 0.5) as i32).clamp(0, 255) as usize
}

// CPU path

impl RenderNodeCpu for CurvesNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        let master = build_lut(&self.master, LUT_SIZE);
        let red = build_lut(&self.red, LUT_SIZE);
        let green = build_lut(&self.green, LUT_SIZE);
        let blue = build_lut(&self.blue, LUT_SIZE);

        for px in rgba.as_chunks_mut::<4>().0 {
            // Master applied to each channel first, then the per-channel curve.
            let mr = master[lut_idx(f32::from(px[0]) / 255.0)];
            let mg = master[lut_idx(f32::from(px[1]) / 255.0)];
            let mb = master[lut_idx(f32::from(px[2]) / 255.0)];
            px[0] = (red[lut_idx(mr)] * 255.0 + 0.5) as u8;
            px[1] = (green[lut_idx(mg)] * 255.0 + 0.5) as u8;
            px[2] = (blue[lut_idx(mb)] * 255.0 + 0.5) as u8;
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
struct CurvesPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    lut_texture: wgpu::Texture,
}

#[cfg(feature = "wgpu")]
impl CurvesNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &CurvesPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let red = build_lut(&self.red, LUT_SIZE);
            let green = build_lut(&self.green, LUT_SIZE);
            let blue = build_lut(&self.blue, LUT_SIZE);
            let master = build_lut(&self.master, LUT_SIZE);
            // Pack as RGBA texels: R=red, G=green, B=blue, A=master.
            let mut texels = vec![0u8; LUT_SIZE * 4];
            for i in 0..LUT_SIZE {
                texels[i * 4] = (red[i] * 255.0 + 0.5) as u8;
                texels[i * 4 + 1] = (green[i] * 255.0 + 0.5) as u8;
                texels[i * 4 + 2] = (blue[i] * 255.0 + 0.5) as u8;
                texels[i * 4 + 3] = (master[i] * 255.0 + 0.5) as u8;
            }

            let lut_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Curves LUT"),
                size: wgpu::Extent3d {
                    width: LUT_SIZE as u32,
                    height: 1,
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
                    texture: &lut_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LUT_SIZE as u32 * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: LUT_SIZE as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Curves shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/curves.wgsl").into()),
            });
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Curves BGL"),
                entries: &[texture_entry(0), texture_entry(1)],
            });
            let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "Curves");

            CurvesPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                lut_texture,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for CurvesNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("CurvesNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("CurvesNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);
        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let lut_view = pd
            .lut_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Curves BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lut_view),
                },
            ],
        });
        run_fullscreen(
            ctx,
            &pd.render_pipeline,
            &bind_group,
            &output_view,
            "Curves pass",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [1.0, 1.0]]
    }

    #[test]
    fn build_lut_identity_should_be_linear() {
        let lut = build_lut(&identity(), LUT_SIZE);
        for (i, &v) in lut.iter().enumerate() {
            let expected = i as f32 / 255.0;
            assert!(
                (v - expected).abs() < 1e-3,
                "identity LUT[{i}] must be {expected}; got {v}"
            );
        }
    }

    #[test]
    fn build_lut_should_stay_within_unit_range() {
        // Non-monotone control points must not overshoot [0, 1] or panic.
        let lut = build_lut(&[[0.0, 0.0], [0.3, 0.9], [0.6, 0.1], [1.0, 1.0]], LUT_SIZE);
        assert_eq!(lut.len(), LUT_SIZE);
        for &v in &lut {
            assert!((0.0..=1.0).contains(&v), "LUT value out of range: {v}");
        }
    }

    #[test]
    fn curves_identity_should_be_noop() {
        let node = CurvesNode::default();
        let original = vec![10u8, 128, 220, 255, 60, 90, 200, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 2, 1);
        for (a, b) in rgba.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "identity curves must preserve the pixel; got {a} vs {b}"
            );
        }
    }

    #[test]
    fn curves_lifted_midtones_should_brighten_grey() {
        let node = CurvesNode::new(
            vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            identity(),
            identity(),
            identity(),
        );
        let mut rgba = vec![128u8, 128, 128, 255]; // 50% grey
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[0] > 150,
            "lifted midtones must brighten 50% grey; got {}",
            rgba[0]
        );
    }

    #[test]
    fn curves_non_monotone_input_should_not_panic() {
        // Unsorted, non-monotone points must be handled without panicking.
        let node = CurvesNode::new(
            vec![[1.0, 0.2], [0.0, 0.8], [0.5, 0.5]],
            identity(),
            identity(),
            identity(),
        );
        let mut rgba = vec![100u8, 150, 200, 255];
        node.process_cpu(&mut rgba, 1, 1);
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

    fn identity() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [1.0, 1.0]]
    }

    #[test]
    fn curves_gpu_lifted_midtones_should_brighten_grey() {
        let Some(ctx) = ctx() else {
            return;
        };
        let frame = vec![128u8, 128, 128, 255];
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(CurvesNode::new(
                vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
                identity(),
                identity(),
                identity(),
            ))
            .process_gpu(&frame, 1, 1)
            .expect("gpu curves");
        assert!(
            gpu[0] > 150,
            "GPU lifted midtones must brighten 50% grey; got {}",
            gpu[0]
        );
    }

    #[test]
    fn curves_gpu_identity_should_preserve_input() {
        let Some(ctx) = ctx() else {
            return;
        };
        let frame = vec![10u8, 128, 220, 255];
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(CurvesNode::default())
            .process_gpu(&frame, 1, 1)
            .expect("gpu curves");
        for i in 0..3 {
            assert!(
                (i32::from(gpu[i]) - i32::from(frame[i])).abs() <= 2,
                "identity curves must preserve the input on the GPU path"
            );
        }
    }
}
