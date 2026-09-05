//! GPU preview adapter over the shared compositing core (Br3, #1626).
//!
//! Wraps [`GpuCompositor`](crate::gpu_compositor::GpuCompositor) as an
//! `ff_preview::PreviewCompositor`, so the preview runner composites on the GPU by
//! default and falls back to its built-in CPU compositor when the core returns `None`
//! (an unsupported layer, no adapter, or a GPU error). All the compositing logic, the
//! v1 identity gate, and the letterbox live in the shared core; this file only adapts
//! the preview layer type.

use std::time::Duration;

use ff_filter::{RealtimeLayer, XfadeTransition};
use ff_format::VideoFrame;
use ff_preview::PreviewCompositor;

use crate::gpu_compositor::GpuCompositor;
use crate::gpu_transition::{GpuTransition, map_transition};

/// The transition node to render `kind` on, or `None` to leave it on the runner's CPU
/// path.
///
/// Note that the runner does not currently reach any transition for an engine-derived
/// scene (#1737): it arms at the incoming clip's offset and needs the outgoing clip to
/// overlap that window, which `avio` never produces. This routing is therefore correct
/// but dormant until that is fixed.
///
/// Narrower than [`map_transition`], and the reason is measurement rather than fidelity:
/// after #1732 every mapped node produces the same pixels as `apply_xfade`, so this only
/// decides *where* the work happens. The GPU has to pay two uploads and a readback per
/// frame, which only the dips earn back — their formula is three `mix`es around two
/// `smoothstep`s per channel, the heaviest per-pixel arithmetic of the set. Measured
/// against the CPU path, over three repeats:
///
/// | kind | 1080p | 4K |
/// |---|---|---|
/// | `FadeBlack` / `FadeWhite` | **2.2x faster** (5.5 vs 12.0 ms) | **1.4x faster** (32 vs 48 ms) |
/// | `Fade` | 1.2x (5.7 vs 7.3 ms) | tie (30 vs 29 ms) |
/// | `WipeRight` | 4.4x *slower* (5.4 vs 1.2 ms) | 7.3x *slower* (51 vs 7 ms) |
/// | `Dissolve` | 2.6x *slower* | 2.1x *slower* |
///
/// So only the dips route here. A wipe is a per-row `memcpy` on the CPU and can never
/// win against a transfer; `Fade` is a single lerp, and its margin sits inside the
/// run-to-run spread at 4K. `Dissolve` looks worst of all here, but its problem is that
/// the mask is built on the CPU and *then* uploaded -- caching that field makes the CPU
/// path 7-9x faster and is #1736, which is where dissolve gets fixed.
fn preview_transition_node(kind: XfadeTransition) -> Option<GpuTransition> {
    match map_transition(kind)? {
        node @ GpuTransition::Dip { .. } => Some(node),
        GpuTransition::Fade | GpuTransition::Dissolve | GpuTransition::Wipe { .. } => None,
    }
}

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

    fn blend(
        &mut self,
        kind: XfadeTransition,
        a: &[u8],
        b: &[u8],
        progress: f32,
        w: u32,
        h: u32,
    ) -> Option<Vec<u8>> {
        let node = preview_transition_node(kind)?;
        self.core.transition(node, progress, a, b.to_vec(), w, h)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ff_filter::XfadeTransition;
    use ff_filter::{
        AnimatedValue, BlendMode, CompositeOp, RealtimeLayer, RealtimeLayerDescriptor,
    };
    use ff_format::{Color, PixelFormat, VideoFrame};

    use super::*;
    use crate::{Clip, Timeline, TimelinePlayer};

    #[test]
    fn preview_should_route_only_the_dips_to_the_gpu() {
        // The policy is measured, not assumed: the dips are the only kinds whose GPU
        // render beats the CPU one once the two uploads and the readback are paid for
        // (1080p 2.2x, 4K 1.4x, over three repeats). See `preview_transition_node`.
        for kind in [XfadeTransition::FadeBlack, XfadeTransition::FadeWhite] {
            assert!(
                matches!(
                    preview_transition_node(kind),
                    Some(GpuTransition::Dip { .. })
                ),
                "{kind:?} is measurably faster on the GPU and must route there"
            );
        }
    }

    #[test]
    fn preview_should_keep_the_cheaper_kinds_on_the_cpu() {
        // These all map to a node (#1657) and render identically (#1732); routing them
        // to the GPU would only be slower. `Fade` is here because its margin sits inside
        // the run-to-run spread at 4K, `WipeRight` because a per-row memcpy cannot lose
        // to a transfer, and `Dissolve` because its CPU-built mask is the real cost --
        // fixed by #1736, not by moving it.
        for kind in [
            XfadeTransition::Fade,
            XfadeTransition::Dissolve,
            XfadeTransition::WipeLeft,
            XfadeTransition::WipeRight,
            XfadeTransition::WipeUp,
            XfadeTransition::WipeDown,
        ] {
            assert!(
                map_transition(kind).is_some(),
                "{kind:?} still maps to a node; only the routing declines it"
            );
            assert!(
                preview_transition_node(kind).is_none(),
                "{kind:?} is faster on the CPU and must not route to the GPU"
            );
        }
    }

    #[test]
    fn preview_should_decline_a_kind_with_no_node() {
        for kind in [XfadeTransition::SlideLeft, XfadeTransition::Pixelize] {
            assert!(preview_transition_node(kind).is_none());
        }
    }

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
    fn gpu_preview_compositor_should_render_a_positioned_base_layer_unmoved() {
        // This used to assert a fallback. A lone layer is the compositor's **base**,
        // and the CPU ignores a base layer transform entirely (measured; see
        // `gpu_compositor::layer_transform`), so falling back bought nothing -- both
        // routes render the same pixels either way. #1633.
        // Probe-gated (RK-002).
        let Some(mut gpu) = GpuPreviewCompositor::new() else {
            return;
        };
        let frame = VideoFrame::from_rgba(4, 4, vec![50u8; 4 * 4 * 4]).unwrap();
        let plain = identity_layer(4, 4);
        let reference = gpu
            .composite(&[(&plain, &frame)], (4, 4), Duration::ZERO)
            .expect("an identity layer composites");

        let mut layer = identity_layer(4, 4);
        layer.x = AnimatedValue::Static(100.0);
        let moved = gpu
            .composite(&[(&layer, &frame)], (4, 4), Duration::ZERO)
            .expect("a positioned base layer must no longer fall back");
        assert_eq!(
            moved.0, reference.0,
            "a base layer transform is ignored on the CPU, so it must not move here"
        );
    }
}
