//! GPU preview compositor: the executor half of the bridge (Br3, #1626).
//!
//! Implements `ff_preview::PreviewCompositor` over `ff-render`, so the preview
//! runner can composite on the GPU by default and fall back to its built-in CPU
//! compositor automatically. Per frame it maps the runner's layers with
//! [`map_scene`](crate::gpu::map_scene), executes each layer's `ColorGrade`/`Scale`
//! effects with an `ff_render::RenderGraph`, composites the stack with
//! `ff_render::Compositor`, and reads the result back to rgba. Any unsupported layer
//! (`map_scene` fallback) or GPU error returns `None`, so the runner uses the CPU
//! path for that frame (never a panic, never a partial result).
//!
//! v1 scope: the GPU path renders only layers that need no geometric placement (an
//! identity transform and a frame whose aspect matches the canvas). Non-identity
//! transforms and letterbox cases fall back to CPU, because the compositor's
//! UV-space transform + stretch-to-canvas model does not yet match the CPU
//! compositor's native-overlay + pad model; closing that with parity tests is Br5.
//! Known v1 inefficiencies (deferred with the zero-copy work): a decoded frame is
//! deep-copied per no-effect layer, and the readback allocates a staging buffer per
//! frame.

use std::sync::Arc;
use std::time::Duration;

use ff_filter::RealtimeLayer;
use ff_format::VideoFrame;
use ff_preview::PreviewCompositor;
use ff_render::{
    ColorGradeNode, Compositor, FrameLayer, LayerTransform, RenderContext, RenderGraph, ScaleNode,
};

use crate::gpu::{GpuEffect, GpuLayerPlan, GpuMapping, map_scene};

/// Composites preview frames on the GPU, falling back to `None` (the runner's CPU
/// path) on unsupported content or any GPU error.
pub struct GpuPreviewCompositor {
    ctx: Arc<RenderContext>,
    /// Compositor cached for its target canvas; rebuilt when the canvas changes.
    compositor: Option<(Compositor, (u32, u32))>,
}

impl GpuPreviewCompositor {
    /// Initialises a GPU context (best available adapter). Returns `None` when no
    /// adapter is available, so the caller keeps the CPU compositor. Logs the
    /// selected path once (lifecycle, not per frame).
    #[must_use]
    pub fn new() -> Option<Self> {
        match RenderContext::init_blocking() {
            Ok(ctx) => {
                log::info!("preview compositor path=gpu");
                Some(Self {
                    ctx: Arc::new(ctx),
                    compositor: None,
                })
            }
            Err(e) => {
                log::info!("preview compositor path=cpu reason={e}");
                None
            }
        }
    }

    /// Applies a layer's mappable effects to its rgba frame via a `RenderGraph`, or
    /// returns the frame unchanged when it has none. `None` on a GPU error.
    fn apply_effects(&self, plan: &GpuLayerPlan, frame: &VideoFrame) -> Option<VideoFrame> {
        if plan.effects.is_empty() {
            return Some(frame.clone());
        }
        let (in_w, in_h) = (frame.width(), frame.height());
        let rgba = frame.to_rgba()?;
        let mut graph = RenderGraph::new(self.ctx.clone());
        // A `Scale` node resizes the frame; track the output dimensions so the
        // read-back buffer is wrapped at the right size.
        let (mut out_w, mut out_h) = (in_w, in_h);
        for effect in &plan.effects {
            graph = match effect {
                GpuEffect::ColorGrade {
                    brightness,
                    contrast,
                    saturation,
                    temperature,
                    tint,
                } => graph.push(ColorGradeNode::new(
                    *brightness,
                    *contrast,
                    *saturation,
                    *temperature,
                    *tint,
                )),
                GpuEffect::Scale {
                    width,
                    height,
                    algorithm,
                } => {
                    out_w = *width;
                    out_h = *height;
                    graph.push(ScaleNode::new(*width, *height, *algorithm))
                }
            };
        }
        let out = graph.process_gpu(&rgba, in_w, in_h).ok()?;
        VideoFrame::from_rgba(out_w, out_h, out).ok()
    }
}

impl PreviewCompositor for GpuPreviewCompositor {
    fn composite(
        &mut self,
        layers: &[(&RealtimeLayer, &VideoFrame)],
        canvas: (u32, u32),
        t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)> {
        // Map the layers to a GPU plan; an unsupported layer falls back to CPU.
        let refs: Vec<&RealtimeLayer> = layers.iter().map(|(l, _)| *l).collect();
        let plan = match map_scene(&refs, canvas, t) {
            GpuMapping::Gpu(plan) => plan,
            GpuMapping::Fallback(_) => return None,
        };

        // Execute each layer's effects, then wrap it as a compositor FrameLayer.
        let mut frame_layers = Vec::with_capacity(plan.layers.len());
        for (lp, (_, frame)) in plan.layers.iter().zip(layers.iter()) {
            // v1 renders only layers that need no geometric placement. The model's
            // transform is in canvas pixels / clockwise degrees, while the
            // compositor's `LayerTransform` is UV-space / counter-clockwise radians,
            // and the compositor stretches each layer to the canvas (no letterbox),
            // whereas the CPU compositor overlays at native size and pads. Matching
            // those exactly is parity work (Br5), so a non-identity transform falls
            // back to CPU here rather than render wrong output.
            if !is_identity_transform(lp) {
                return None;
            }
            let processed = self.apply_effects(lp, frame)?;
            // Same reason: the compositor would stretch a differently-shaped frame to
            // fill the canvas, which the CPU path letterboxes instead. Only a frame
            // whose aspect matches the canvas composites without distortion.
            if u64::from(processed.width()) * u64::from(canvas.1)
                != u64::from(processed.height()) * u64::from(canvas.0)
            {
                return None;
            }
            frame_layers.push(FrameLayer {
                frame: processed,
                transform: LayerTransform::default(),
                blend_mode: lp.blend_mode,
                opacity: lp.opacity,
                z_order: lp.z_order,
            });
        }

        // Composite (rebuilding the cached compositor if the canvas changed) and
        // read back to rgba. A GPU error becomes `None` (CPU fallback).
        let rebuild = match &self.compositor {
            Some((_, cached)) => *cached != canvas,
            None => true,
        };
        if rebuild {
            self.compositor = Some((
                Compositor::new(self.ctx.clone(), canvas.0, canvas.1),
                canvas,
            ));
        }
        let (compositor, _) = self.compositor.as_mut()?;
        compositor.composite_to_rgba(&mut frame_layers).ok()
    }
}

/// Whether a plan layer's transform is the identity (no translate / scale / rotate),
/// within a small tolerance. Non-identity transforms fall back to CPU in v1.
fn is_identity_transform(lp: &GpuLayerPlan) -> bool {
    lp.x.abs() < 1e-6
        && lp.y.abs() < 1e-6
        && (lp.scale_x - 1.0).abs() < 1e-6
        && (lp.scale_y - 1.0).abs() < 1e-6
        && lp.rotation.abs() < 1e-6
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ff_filter::{
        AnimatedValue, BlendMode, CompositeOp, RealtimeLayer, RealtimeLayerDescriptor,
    };
    use ff_format::{Color, PixelFormat, VideoFrame};

    use super::*;
    use crate::{Clip, Timeline, TimelinePlayer};

    fn identity_layer(w: u32, h: u32) -> RealtimeLayer {
        let desc = RealtimeLayerDescriptor {
            effects: Vec::new(),
            opacity: AnimatedValue::Static(1.0),
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            blend_mode: BlendMode::Normal,
            composite_op: CompositeOp::Over,
        };
        RealtimeLayer::with_dimensions(desc, w, h, PixelFormat::Rgba)
    }

    #[test]
    fn open_forcing_cpu_should_not_attach_a_gpu_compositor() {
        // Forcing CPU must never inject the GPU compositor, regardless of adapter.
        let timeline = Timeline::builder()
            .canvas(16, 16)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::solid(Color::rgb(10, 20, 30)).trim(Duration::ZERO, Duration::from_secs(1)),
            ])
            .build()
            .unwrap();
        match TimelinePlayer::open_forcing_cpu(&timeline) {
            Ok((runner, _handle)) => assert!(
                !runner.has_gpu_compositor(),
                "force-cpu must not attach a gpu compositor"
            ),
            // Skip when the preview cannot open here (e.g. the color filter is
            // unavailable on a minimal FFmpeg): the force-cpu path is unreachable.
            Err(_) => {}
        }
    }

    #[test]
    fn open_should_attach_a_gpu_compositor_when_available() {
        // Default open attaches the GPU compositor when an adapter is present
        // (the GPU-by-default path). Probe-gated: skip without an adapter.
        if GpuPreviewCompositor::new().is_none() {
            return;
        }
        let timeline = Timeline::builder()
            .canvas(16, 16)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::solid(Color::rgb(10, 20, 30)).trim(Duration::ZERO, Duration::from_secs(1)),
            ])
            .build()
            .unwrap();
        match TimelinePlayer::open(&timeline) {
            Ok((runner, _handle)) => assert!(
                runner.has_gpu_compositor(),
                "a gpu adapter is present, so open must attach the gpu compositor"
            ),
            // Skip when the preview cannot open here (color filter unavailable).
            Err(_) => {}
        }
    }

    #[test]
    fn gpu_preview_compositor_should_composite_a_single_layer() {
        // Probe-gated (RK-002): skip when no GPU adapter is available.
        let Some(mut gpu) = GpuPreviewCompositor::new() else {
            return;
        };
        let layer = identity_layer(4, 4);
        let frame = VideoFrame::from_rgba(4, 4, vec![50u8; 4 * 4 * 4]).unwrap();
        let out = gpu.composite(&[(&layer, &frame)], (4, 4), Duration::ZERO);
        let (rgba, w, h) = out.expect("gpu composite of a supported single layer");
        assert_eq!((w, h), (4, 4));
        assert_eq!(rgba.len(), 4 * 4 * 4);
    }

    #[test]
    fn gpu_preview_compositor_should_composite_a_colour_graded_layer() {
        // Exercises apply_effects: a mapped ColorGrade runs through a RenderGraph.
        // Probe-gated (RK-002).
        let Some(mut gpu) = GpuPreviewCompositor::new() else {
            return;
        };
        let mut desc = identity_layer(4, 4);
        desc.effects = vec![ff_filter::FilterStep::Eq {
            brightness: 0.4,
            contrast: 1.2,
            saturation: 1.0,
        }];
        let frame = VideoFrame::from_rgba(4, 4, vec![80u8; 4 * 4 * 4]).unwrap();
        let out = gpu.composite(&[(&desc, &frame)], (4, 4), Duration::ZERO);
        let (rgba, w, h) = out.expect("gpu composite of a colour-graded layer");
        assert_eq!((w, h), (4, 4));
        assert_eq!(rgba.len(), 4 * 4 * 4);
    }

    #[test]
    fn gpu_preview_compositor_should_fall_back_on_a_non_identity_transform() {
        // A positioned layer cannot be rendered correctly in v1 (pixel-vs-UV units),
        // so the compositor returns None and the runner uses the CPU path.
        // Probe-gated (RK-002).
        let Some(mut gpu) = GpuPreviewCompositor::new() else {
            return;
        };
        let mut layer = identity_layer(4, 4);
        layer.x = AnimatedValue::Static(100.0);
        let frame = VideoFrame::from_rgba(4, 4, vec![50u8; 4 * 4 * 4]).unwrap();
        assert!(
            gpu.composite(&[(&layer, &frame)], (4, 4), Duration::ZERO)
                .is_none(),
            "a non-identity transform must fall back to CPU"
        );
    }
}
