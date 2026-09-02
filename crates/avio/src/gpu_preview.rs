//! GPU preview adapter over the shared compositing core (Br3, #1626).
//!
//! Wraps [`GpuCompositor`](crate::gpu_compositor::GpuCompositor) as an
//! `ff_preview::PreviewCompositor`, so the preview runner composites on the GPU by
//! default and falls back to its built-in CPU compositor when the core returns `None`
//! (an unsupported layer, no adapter, or a GPU error). All the compositing logic, the
//! v1 identity gate, and the letterbox live in the shared core; this file only adapts
//! the preview layer type.

use std::time::Duration;

use ff_filter::RealtimeLayer;
use ff_format::VideoFrame;
use ff_preview::PreviewCompositor;

use crate::gpu_compositor::GpuCompositor;

/// Preview adapter over [`GpuCompositor`]: composites the runner's layers on the GPU,
/// falling back to `None` (the runner's CPU path) on unsupported content or a GPU error.
pub struct GpuPreviewCompositor {
    core: GpuCompositor,
}

impl GpuPreviewCompositor {
    /// Initialises the GPU core, or `None` when no adapter is available (so the
    /// runner keeps its CPU compositor). Logs the selected path once (lifecycle).
    #[must_use]
    pub fn new() -> Option<Self> {
        if let Some(core) = GpuCompositor::new() {
            log::info!("preview compositor path=gpu");
            Some(Self { core })
        } else {
            log::info!("preview compositor path=cpu");
            None
        }
    }
}

impl PreviewCompositor for GpuPreviewCompositor {
    fn composite(
        &mut self,
        layers: &[(&RealtimeLayer, &VideoFrame)],
        canvas: (u32, u32),
        t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)> {
        self.core.composite(layers, canvas, t)
    }
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
            temperature: 0.0,
            tint: 0.0,
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
