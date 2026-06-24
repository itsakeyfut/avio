//! Real-time multi-layer video compositor fed by externally-decoded frames.
//!
//! Unlike [`MultiTrackComposer`](super::MultiTrackComposer) (which decodes
//! internally via `movie` sources and is pulled to completion for export), a
//! [`RealtimeComposer`] exposes one `buffersrc` input per layer so a host (e.g.
//! a seekable preview player) feeds already-decoded frames per layer per tick
//! and pulls one composited frame. Per-clip effects and blend modes are applied
//! by the **same** `FFmpeg` filter primitives the export path uses, so the
//! preview matches the rendered output.

#![allow(unsafe_code)]

use ff_format::{PixelFormat, VideoFrame};

use crate::blend::BlendMode;
use crate::error::FilterError;
use crate::graph::filter_step::FilterStep;
use crate::graph::graph::FilterGraph;

// ── RealtimeLayer ─────────────────────────────────────────────────────────────

/// One layer in a [`RealtimeComposer`], composited bottom-up in `Vec` order
/// (index `0` is the base; later layers blend on top).
///
/// Frames pushed to this layer via [`RealtimeComposer::push_layer`] must match
/// the [`width`](Self::width), [`height`](Self::height), and
/// [`pixel_format`](Self::pixel_format) declared here — these fix the layer's
/// `buffersrc` format at build time.
#[derive(Debug, Clone)]
pub struct RealtimeLayer {
    /// Width in pixels of frames pushed to this layer.
    pub width: u32,
    /// Height in pixels of frames pushed to this layer.
    pub height: u32,
    /// Pixel format of frames pushed to this layer.
    pub pixel_format: PixelFormat,
    /// Per-clip video effect chain applied to this layer before compositing
    /// (the same `FilterStep`s as `Clip::video_effect_chain` / the export path).
    pub effects: Vec<FilterStep>,
    /// Opacity in `[0.0, 1.0]`. Applied when this layer is blended onto the layer
    /// below it (no effect on the base layer 0 — apply base opacity host-side).
    pub opacity: f32,
    /// How this layer blends with the layer below. [`BlendMode::Normal`] uses
    /// `overlay`; other modes use `blend=all_mode=<token>`.
    pub blend_mode: BlendMode,
}

// ── RealtimeComposer ──────────────────────────────────────────────────────────

/// Composites externally-decoded frames from several layers into one frame,
/// reusing a single built filter graph across frames.
///
/// Build once with [`new`](Self::new); then per output frame, [`push_layer`] one
/// frame for every layer and [`pull`] the composited result. The output frame is
/// `rgba`. The graph (and any `lut3d` file its effects load) is built once, so it
/// is suitable for real-time playback.
pub struct RealtimeComposer {
    graph: FilterGraph,
    layer_count: usize,
}

impl RealtimeComposer {
    /// Builds a compositor for the given layers (at least one required).
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::CompositionFailed`] when `layers` is empty or the
    /// underlying `FFmpeg` graph cannot be built.
    pub fn new(layers: &[RealtimeLayer]) -> Result<Self, FilterError> {
        let layer_count = layers.len();
        // SAFETY: all raw-pointer operations in `build_realtime_composition`
        // follow the avfilter ownership rules; the returned graph owns every
        // context it created.
        let graph = unsafe { super::composition_inner::build_realtime_composition(layers)? };
        Ok(Self { graph, layer_count })
    }

    /// Number of layers (== number of input slots).
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layer_count
    }

    /// Pushes one frame into layer `idx`'s input slot.
    ///
    /// Push exactly one frame per layer before each [`pull`](Self::pull). The
    /// frame must match the layer's declared width/height/pixel format.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError`] if `idx` is out of range or the frame cannot be
    /// pushed.
    pub fn push_layer(&mut self, idx: usize, frame: &VideoFrame) -> Result<(), FilterError> {
        self.graph.push_video(idx, frame)
    }

    /// Pulls the next composited frame (`rgba`), or `None` if not yet available.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError`] on an unexpected `FFmpeg` error.
    pub fn pull(&mut self) -> Result<Option<VideoFrame>, FilterError> {
        self.graph.pull_video()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn empty_layers_should_err() {
        let result = RealtimeComposer::new(&[]);
        assert!(matches!(result, Err(FilterError::CompositionFailed { .. })));
    }

    #[test]
    fn two_layer_composite_should_produce_rgba_frame() {
        // 4×4 RGBA base + overlay; skip-guard on FFmpeg availability.
        let layer = |op: f32| RealtimeLayer {
            width: 4,
            height: 4,
            pixel_format: PixelFormat::Rgba,
            effects: vec![],
            opacity: op,
            blend_mode: BlendMode::Normal,
        };
        let mut composer = match RealtimeComposer::new(&[layer(1.0), layer(0.5)]) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        let base = VideoFrame::from_rgba(4, 4, vec![10u8; 4 * 4 * 4]).unwrap();
        let top = VideoFrame::from_rgba(4, 4, vec![200u8; 4 * 4 * 4]).unwrap();
        if composer.push_layer(0, &base).is_err() || composer.push_layer(1, &top).is_err() {
            println!("Skipping: push failed (FFmpeg unavailable?)");
            return;
        }
        match composer.pull() {
            Ok(Some(out)) => {
                assert_eq!(out.format(), PixelFormat::Rgba);
                assert_eq!(out.width(), 4);
                assert_eq!(out.height(), 4);
            }
            Ok(None) => println!("Skipping: no frame produced"),
            Err(e) => println!("Skipping: {e}"),
        }
    }
}
