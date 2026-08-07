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
/// Build once with [`new`](Self::new); then per output frame,
/// [`push_layer`](Self::push_layer) one frame for every layer and
/// [`pull`](Self::pull) the composited result. The output frame is
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
        Self::with_canvas(layers, None)
    }

    /// Like [`new`](Self::new), but composites onto a fixed project canvas: the
    /// output is letterboxed/pillarboxed to `canvas = (width, height)` so the
    /// composited frame matches the project's output aspect. `None` composites at
    /// the base layer's own size (identical to [`new`](Self::new)).
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::CompositionFailed`] when `layers` is empty or the
    /// underlying `FFmpeg` graph cannot be built.
    pub fn with_canvas(
        layers: &[RealtimeLayer],
        canvas: Option<(u32, u32)>,
    ) -> Result<Self, FilterError> {
        let layer_count = layers.len();
        // SAFETY: all raw-pointer operations in `build_realtime_composition`
        // follow the avfilter ownership rules; the returned graph owns every
        // context it created.
        let graph =
            unsafe { super::composition_inner::build_realtime_composition(layers, canvas)? };
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
    fn base_layer_with_glow_compound_effect_should_build() {
        // Regression: `Glow` is a compound `FilterStep` (split → curves → gblur →
        // blend). The realtime compositor drives per-layer effects through
        // `add_and_link_step`, which must dispatch compound steps to their builders
        // rather than create a single `split` filter with the whole subgraph as
        // args (which failed with "No such option: split").
        //
        // Probe-gate: a no-effect base layer needs only `buffer`/`format`/
        // `buffersink`. CI's Linux FFmpeg is built with no filters, so even that
        // fails to build there — skip. Where it builds, the filter set is present,
        // so the `Glow` layer *must* also build; that is the regression assertion
        // (it fails before the fix, on any host with filters).
        let probe = RealtimeLayer {
            width: 8,
            height: 8,
            pixel_format: PixelFormat::Rgba,
            effects: vec![],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        if RealtimeComposer::new(&[probe]).is_err() {
            println!("Skipping: FFmpeg filters unavailable");
            return;
        }

        let layer = RealtimeLayer {
            width: 8,
            height: 8,
            pixel_format: PixelFormat::Rgba,
            effects: vec![FilterStep::Glow {
                threshold: 0.6,
                radius: 4.0,
                intensity: 0.5,
            }],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let mut composer = RealtimeComposer::new(&[layer])
            .expect("Glow compound step must dispatch and build once FFmpeg filters exist");
        let frame = VideoFrame::from_rgba(8, 8, vec![120u8; 8 * 8 * 4]).unwrap();
        if composer.push_layer(0, &frame).is_err() {
            println!("Skipping: push failed (FFmpeg unavailable?)");
            return;
        }
        match composer.pull() {
            Ok(Some(out)) => assert_eq!(out.format(), PixelFormat::Rgba),
            Ok(None) => println!("Skipping: no frame produced"),
            Err(e) => println!("Skipping: {e}"),
        }
    }

    #[test]
    fn with_canvas_letterboxes_output_to_canvas_size() {
        // A 640×360 (16:9) base composited onto a 1080×1920 (9:16) canvas must
        // produce a 1080×1920 frame (letterboxed), not the base's own size.
        let base = RealtimeLayer {
            width: 640,
            height: 360,
            pixel_format: PixelFormat::Rgba,
            effects: vec![],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let mut composer = match RealtimeComposer::with_canvas(&[base], Some((1080, 1920))) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        let frame = VideoFrame::from_rgba(640, 360, vec![120u8; 640 * 360 * 4]).unwrap();
        if composer.push_layer(0, &frame).is_err() {
            println!("Skipping: push failed (FFmpeg unavailable?)");
            return;
        }
        match composer.pull() {
            Ok(Some(out)) => {
                assert_eq!(out.width(), 1080);
                assert_eq!(out.height(), 1920);
                let rgba = out.to_rgba().expect("rgba");
                assert_eq!(rgba.len(), 1080 * 1920 * 4);
            }
            Ok(None) => println!("Skipping: no frame produced"),
            Err(e) => println!("Skipping: {e}"),
        }
    }

    #[test]
    fn base_layer_large_scale_then_offset_crop_does_not_overrun() {
        // Regression: a large up-`Scale` followed by a `Crop` with a large centre
        // offset used to segfault when reading back the composited frame — the crop
        // view's `data[i]` is offset into the scaled buffer while its linesize stays
        // the parent's, so copying `stride * rows` overran the buffer by the offset.
        // 640×360 → scale 3412×1920 → crop 1080×1920 at x=1166 (centre).
        let base = RealtimeLayer {
            width: 640,
            height: 360,
            pixel_format: PixelFormat::Rgba,
            effects: vec![
                FilterStep::Scale {
                    width: 3412,
                    height: 1920,
                    algorithm: crate::graph::types::ScaleAlgorithm::Bicubic,
                },
                FilterStep::Crop {
                    x: 1166,
                    y: 0,
                    width: 1080,
                    height: 1920,
                },
            ],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let mut composer = match RealtimeComposer::new(&[base]) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        let frame = VideoFrame::from_rgba(640, 360, vec![120u8; 640 * 360 * 4]).unwrap();
        if composer.push_layer(0, &frame).is_err() {
            println!("Skipping: push failed (FFmpeg unavailable?)");
            return;
        }
        match composer.pull() {
            Ok(Some(out)) => {
                assert_eq!(out.width(), 1080);
                assert_eq!(out.height(), 1920);
                let rgba = out.to_rgba().expect("rgba");
                assert_eq!(rgba.len(), 1080 * 1920 * 4);
            }
            Ok(None) => println!("Skipping: no frame produced"),
            Err(e) => println!("Skipping: {e}"),
        }
    }

    #[test]
    fn base_layer_fit_to_aspect_is_scaled_and_padded() {
        // Regression: `FitToAspect` is a compound scale+pad. The realtime compositor
        // used to apply only the scale, so the frame was never padded to the target
        // canvas. A 640×360 (16:9) base fitted to 1080×1920 (9:16) must be scaled to
        // fit (1080×608) *and* padded to the full 1080×1920, not left at 1080×608.
        let base = RealtimeLayer {
            width: 640,
            height: 360,
            pixel_format: PixelFormat::Rgba,
            effects: vec![FilterStep::FitToAspect {
                width: 1080,
                height: 1920,
                color: "black".to_string(),
            }],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let mut composer = match RealtimeComposer::new(&[base]) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        let frame = VideoFrame::from_rgba(640, 360, vec![120u8; 640 * 360 * 4]).unwrap();
        if composer.push_layer(0, &frame).is_err() {
            println!("Skipping: push failed (FFmpeg unavailable?)");
            return;
        }
        match composer.pull() {
            Ok(Some(out)) => {
                assert_eq!(out.width(), 1080);
                assert_eq!(out.height(), 1920);
            }
            Ok(None) => println!("Skipping: no frame produced"),
            Err(e) => println!("Skipping: {e}"),
        }
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

    #[test]
    fn differently_sized_layers_with_blend_mode_should_composite() {
        // Regression: the `blend` filter (Screen/Multiply/…) requires equal-sized
        // inputs, so the composer must scale each layer to the base size. Base 4×4,
        // overlay 8×6, Screen. Output is the base size.
        let base = RealtimeLayer {
            width: 4,
            height: 4,
            pixel_format: PixelFormat::Rgba,
            effects: vec![],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let top = RealtimeLayer {
            width: 8,
            height: 6,
            pixel_format: PixelFormat::Rgba,
            effects: vec![],
            opacity: 1.0,
            blend_mode: BlendMode::Screen,
        };
        let mut composer = match RealtimeComposer::new(&[base, top]) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        let bf = VideoFrame::from_rgba(4, 4, vec![80u8; 4 * 4 * 4]).unwrap();
        let tf = VideoFrame::from_rgba(8, 6, vec![80u8; 8 * 6 * 4]).unwrap();
        if composer.push_layer(0, &bf).is_err() || composer.push_layer(1, &tf).is_err() {
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

    #[test]
    fn base_layer_crop_then_scale_zooms_into_the_cropped_region() {
        // Crop the right half then scale back to full size ("crop & zoom"). The
        // top-left output pixel (black in the source's left half) must become the
        // white of the cropped right half — proving crop+scale actually resamples.
        let base = RealtimeLayer {
            width: 8,
            height: 8,
            pixel_format: PixelFormat::Rgba,
            effects: vec![
                FilterStep::Crop {
                    x: 4,
                    y: 0,
                    width: 4,
                    height: 8,
                },
                FilterStep::Scale {
                    width: 8,
                    height: 8,
                    algorithm: crate::graph::types::ScaleAlgorithm::Bilinear,
                },
            ],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let mut composer = match RealtimeComposer::new(&[base]) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        // Left half (cols 0..4) black, right half (cols 4..8) white.
        let mut data = vec![0u8; 8 * 8 * 4];
        for row in 0..8 {
            for col in 4..8 {
                let p = (row * 8 + col) * 4;
                data[p] = 255;
                data[p + 1] = 255;
                data[p + 2] = 255;
                data[p + 3] = 255;
            }
        }
        for row in 0..8 {
            for col in 0..4 {
                data[(row * 8 + col) * 4 + 3] = 255; // opaque black
            }
        }
        let frame = VideoFrame::from_rgba(8, 8, data).unwrap();
        if composer.push_layer(0, &frame).is_err() {
            println!("Skipping: push failed (FFmpeg unavailable?)");
            return;
        }
        match composer.pull() {
            Ok(Some(out)) => {
                assert_eq!(out.width(), 8);
                assert_eq!(out.height(), 8);
                let rgba = out.to_rgba().expect("rgba");
                // Top-left pixel: source left half was black; after cropping to the
                // right (white) half and scaling up it must be near-white.
                assert!(
                    rgba[0] > 200,
                    "expected top-left to be white after crop+zoom, got {}",
                    rgba[0]
                );
            }
            Ok(None) => println!("Skipping: no frame produced"),
            Err(e) => println!("Skipping: {e}"),
        }
    }

    #[test]
    fn base_layer_crop_resizes_the_output_frame() {
        // A base-layer `Crop` shrinks the composited frame: the output must carry
        // the cropped dimensions (not the pushed 8×8), and its RGBA buffer length
        // must equal `width * height * 4`. Consumers that report the decoded size
        // instead of this one would mis-tag the buffer and drop the frame.
        let base = RealtimeLayer {
            width: 8,
            height: 8,
            pixel_format: PixelFormat::Rgba,
            effects: vec![FilterStep::Crop {
                x: 0,
                y: 0,
                width: 4,
                height: 6,
            }],
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        };
        let mut composer = match RealtimeComposer::new(&[base]) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping: {e}");
                return;
            }
        };
        let frame = VideoFrame::from_rgba(8, 8, vec![120u8; 8 * 8 * 4]).unwrap();
        if composer.push_layer(0, &frame).is_err() {
            println!("Skipping: push failed (FFmpeg unavailable?)");
            return;
        }
        match composer.pull() {
            Ok(Some(out)) => {
                assert_eq!(out.width(), 4);
                assert_eq!(out.height(), 6);
                let rgba = out.to_rgba().expect("rgba");
                assert_eq!(rgba.len(), 4 * 6 * 4);
            }
            Ok(None) => println!("Skipping: no frame produced"),
            Err(e) => println!("Skipping: {e}"),
        }
    }
}
