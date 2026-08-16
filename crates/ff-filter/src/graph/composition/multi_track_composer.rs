//! Multi-track video composition onto a solid-colour canvas.

#![allow(unsafe_code)]

use std::path::PathBuf;

use crate::animation::AnimatedValue;
use crate::blend::BlendMode;
use crate::composite::CompositeOp;
use crate::error::FilterError;
use crate::graph::graph::FilterGraph;
use crate::graph::types::Rgb;

// ── ProxySource ───────────────────────────────────────────────────────────────

/// A low-resolution proxy substitute for a [`VideoLayer`]'s source.
///
/// When set on [`VideoLayer::proxy`], frames are decoded from [`path`](Self::path)
/// instead of the layer's `source`, then scaled up to `width` × `height` (the
/// original source resolution) before any other processing. This lets callers
/// render from small proxy files for speed while producing full-resolution output.
#[derive(Debug, Clone)]
pub struct ProxySource {
    /// Path to the proxy media file to decode from.
    pub path: PathBuf,
    /// Original source width, in pixels. Decoded proxy frames are scaled to this.
    pub width: u32,
    /// Original source height, in pixels. Decoded proxy frames are scaled to this.
    pub height: u32,
}

// ── VideoLayer ────────────────────────────────────────────────────────────────

/// A single video layer in a [`MultiTrackComposer`] composition.
///
/// Layers are composited in the order they are added, the first
/// ([`add_layer`](MultiTrackComposer::add_layer)) at the bottom of the stack.
/// Trim / timeline placement are expressed by the caller as leading
/// [`FilterStep`](crate::graph::filter_step::FilterStep)s in [`effects`](Self::effects)
/// (e.g. [`Trim`](crate::graph::filter_step::FilterStep::Trim) +
/// [`OffsetPts`](crate::graph::filter_step::FilterStep::OffsetPts)); this layer type is
/// model-free.
///
/// The `x`, `y`, `scale_x`, `scale_y`, `rotation`, and `opacity` fields accept
/// either a constant ([`AnimatedValue::Static`]) or a time-varying keyframe
/// track ([`AnimatedValue::Track`]).  The initial filter graph is built from
/// the value at `Duration::ZERO`; per-frame updates are added in issue #363.
#[derive(Debug, Clone)]
pub struct VideoLayer {
    /// Source media file path, or a `lavfi` filtergraph string when
    /// [`input_format`](Self::input_format) is `Some("lavfi")`.
    pub source: PathBuf,
    /// Optional low-resolution proxy to decode from instead of `source`.
    ///
    /// When `Some`, frames are decoded from the proxy and scaled up to the
    /// original source resolution before trim/effects/compositing, so the
    /// output is identical in size to a non-proxy render. Ignored when
    /// [`input_format`](Self::input_format) is `Some("lavfi")`.
    pub proxy: Option<ProxySource>,
    /// Optional `FFmpeg` input format name passed to the `movie` filter.
    ///
    /// When `Some("lavfi")`, `source` is interpreted as a filtergraph string
    /// (e.g. `"color=s=1920x1080:c=black@0.0,drawtext=text='Title'"`) and opened
    /// via `FFmpeg`'s `lavfi` virtual demuxer. When `None` (the default), `source`
    /// is opened as an ordinary media file.
    pub input_format: Option<String>,
    /// X offset on the canvas in pixels (top-left origin).
    pub x: AnimatedValue<f64>,
    /// Y offset on the canvas in pixels.
    pub y: AnimatedValue<f64>,
    /// Horizontal scale factor applied to the source frame (`1.0` = original width).
    pub scale_x: AnimatedValue<f64>,
    /// Vertical scale factor applied to the source frame (`1.0` = original height).
    pub scale_y: AnimatedValue<f64>,
    /// Clockwise rotation in degrees (`0.0` = no rotation).
    pub rotation: AnimatedValue<f64>,
    /// Opacity (`0.0` = fully transparent, `1.0` = fully opaque).
    pub opacity: AnimatedValue<f64>,
    /// How this layer blends with the layer(s) below it.
    ///
    /// [`BlendMode::Normal`] uses the `FFmpeg` `overlay` filter (standard alpha-over composite).
    /// All other modes use the `FFmpeg` `blend` filter with the corresponding `all_mode`.
    /// Defaults to [`BlendMode::Normal`].
    ///
    /// Colour blend only — applies when [`composite_op`](Self::composite_op) is
    /// [`CompositeOp::Over`]. For a non-`Over` composite op the layer is composited
    /// by the Porter-Duff operator and `blend_mode` is not additionally applied.
    pub blend_mode: BlendMode,
    /// Porter-Duff alpha-compositing operator for placing this layer over the
    /// canvas. Defaults to [`CompositeOp::Over`] (standard alpha-over).
    ///
    /// `Over` keeps the existing [`blend_mode`](Self::blend_mode) compositing
    /// (overlay / `blend all_mode`). `Under`/`In`/`Out`/`Atop`/`Xor` composite the
    /// layer via the corresponding Porter-Duff construction (the colour
    /// `blend_mode` is not applied in that case).
    ///
    /// For a non-`Over` op, [`opacity`](Self::opacity) uses its initial value
    /// only — animated opacity is not tracked per-frame on these paths (the
    /// `blend all_opacity` / `overlay` alpha is set once at build time).
    pub composite_op: CompositeOp,
    /// Per-layer video filter steps applied to this layer's decoded stream before compositing.
    ///
    /// Applied in order after scale/rotate/opacity and before the `overlay` node. Any timeline
    /// trim / placement steps (`Trim` + `ResetPts` + `OffsetPts`) the caller emits belong at the
    /// front of this list so they precede timing-sensitive effects (`Speed`). Typical use:
    /// `FilterStep::Eq { brightness, contrast, saturation }` for per-clip color correction.
    /// An empty `Vec` (the default) is a no-op.
    pub effects: Vec<crate::graph::filter_step::FilterStep>,
}

// ── MultiTrackComposer ────────────────────────────────────────────────────────

/// Composes multiple video layers onto a solid-colour canvas.
///
/// Layers are composited in the order they are added (first at the bottom).  The
/// resulting [`FilterGraph`] is source-only — call [`FilterGraph::pull_video`]
/// in a loop to extract the output frames.  The graph terminates when the
/// last (top) layer finishes.
///
/// # Examples
///
/// ```ignore
/// use ff_filter::{AnimatedValue, MultiTrackComposer, VideoLayer};
///
/// let mut graph = MultiTrackComposer::new(1920, 1080)
///     .add_layer(VideoLayer {
///         source: "clip.mp4".into(),
///         proxy: None,
///         input_format: None,
///         x: AnimatedValue::Static(0.0),
///         y: AnimatedValue::Static(0.0),
///         scale_x: AnimatedValue::Static(1.0),
///         scale_y: AnimatedValue::Static(1.0),
///         rotation: AnimatedValue::Static(0.0),
///         opacity: AnimatedValue::Static(1.0),
///         blend_mode: BlendMode::Normal,
///         composite_op: CompositeOp::Over,
///         effects: vec![],
///     })
///     .build()?;
///
/// while let Some(frame) = graph.pull_video()? {
///     // encode or display `frame`
/// }
/// ```
pub struct MultiTrackComposer {
    canvas_width: u32,
    canvas_height: u32,
    background: Rgb,
    frame_rate: f64,
    layers: Vec<VideoLayer>,
}

impl MultiTrackComposer {
    /// Creates a new composer with a black canvas and no layers.
    pub fn new(canvas_width: u32, canvas_height: u32) -> Self {
        Self {
            canvas_width,
            canvas_height,
            background: Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
            },
            frame_rate: 30.0,
            layers: Vec::new(),
        }
    }

    /// Sets the canvas background colour and returns the updated composer.
    #[must_use]
    pub fn background(self, rgb: Rgb) -> Self {
        Self {
            background: rgb,
            ..self
        }
    }

    /// Sets the output frame rate (frames per second) of the composition.
    ///
    /// The background canvas and the per-layer `fps` conform are generated at
    /// this rate, so it MUST match the rate the caller encodes/muxes at —
    /// otherwise the composited video is stretched or compressed by
    /// `this_rate / encode_rate` while the audio stays correct, desyncing A/V.
    /// Defaults to `30.0`.
    #[must_use]
    pub fn frame_rate(self, fps: f64) -> Self {
        Self {
            frame_rate: if fps > 0.0 { fps } else { 30.0 },
            ..self
        }
    }

    /// Appends a video layer and returns the updated composer.
    ///
    /// A layer that cross-fades from its predecessor carries a
    /// [`FilterStep::XFade`](crate::graph::filter_step::FilterStep::XFade) in its
    /// [`effects`](VideoLayer::effects); the compositor wires the transition's
    /// second input from the preceding layer at build time.
    #[must_use]
    pub fn add_layer(self, layer: VideoLayer) -> Self {
        let mut layers = self.layers;
        layers.push(layer);
        Self { layers, ..self }
    }

    /// Builds a source-only [`FilterGraph`] that composites all layers.
    ///
    /// # Errors
    ///
    /// - [`FilterError::CompositionFailed`] — canvas width or height is zero,
    ///   no layers were added, or an underlying `FFmpeg` graph-construction
    ///   call failed.
    pub fn build(self) -> Result<FilterGraph, FilterError> {
        if self.canvas_width == 0 || self.canvas_height == 0 {
            return Err(FilterError::CompositionFailed {
                reason: format!(
                    "canvas dimensions must be non-zero: {}x{}",
                    self.canvas_width, self.canvas_height
                ),
            });
        }
        if self.layers.is_empty() {
            return Err(FilterError::CompositionFailed {
                reason: "no layers".to_string(),
            });
        }
        // Layers are composited in insertion order (caller emits them in the
        // intended bottom-to-top order); the compositor no longer reorders.
        let layers = self.layers;
        // SAFETY: all raw pointer operations follow the avfilter ownership rules:
        // - avfilter_graph_alloc() returns an owned pointer freed via
        //   avfilter_graph_free() on error or stored in FilterGraphInner on success.
        // - avfilter_graph_create_filter() adds contexts owned by the graph.
        // - avfilter_link() connects pads; connections are owned by the graph.
        // - avfilter_graph_config() finalises the graph.
        // - NonNull::new_unchecked() is called only after ret >= 0 checks.
        unsafe {
            super::composition_inner::build_video_composition(
                self.canvas_width,
                self.canvas_height,
                self.background,
                self.frame_rate,
                &layers,
            )
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::graph::filter_step::FilterStep;

    #[test]
    fn composer_zero_canvas_size_should_err() {
        // width = 0
        let result = MultiTrackComposer::new(0, 1080)
            .add_layer(VideoLayer {
                source: "clip.mp4".into(),
                proxy: None,
                input_format: None,
                x: AnimatedValue::Static(0.0),
                y: AnimatedValue::Static(0.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                opacity: AnimatedValue::Static(1.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                effects: vec![],
            })
            .build();
        assert!(
            matches!(result, Err(FilterError::CompositionFailed { .. })),
            "expected CompositionFailed for zero width, got {result:?}"
        );

        // height = 0
        let result = MultiTrackComposer::new(1920, 0)
            .add_layer(VideoLayer {
                source: "clip.mp4".into(),
                proxy: None,
                input_format: None,
                x: AnimatedValue::Static(0.0),
                y: AnimatedValue::Static(0.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                opacity: AnimatedValue::Static(1.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                effects: vec![],
            })
            .build();
        assert!(
            matches!(result, Err(FilterError::CompositionFailed { .. })),
            "expected CompositionFailed for zero height, got {result:?}"
        );
    }

    #[test]
    fn composer_canvas_larger_than_track_should_succeed() {
        // A 1920×1080 canvas is larger than a typical 640×480 source track.
        // Canvas size is independent of layer resolution — placement at (x, y)
        // is handled by the overlay filter; no auto-scale is applied.
        // The validation guard must not reject non-zero canvas dimensions.
        // If the build fails it must be for an FFmpeg reason (e.g. source file
        // not found), not because of canvas size.
        let result = MultiTrackComposer::new(1920, 1080)
            .add_layer(VideoLayer {
                source: "nonexistent_640x480.mp4".into(),
                proxy: None,
                input_format: None,
                x: AnimatedValue::Static(100.0),
                y: AnimatedValue::Static(100.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                opacity: AnimatedValue::Static(1.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                effects: vec![],
            })
            .build();
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("canvas") && !reason.contains("zero"),
                "build failed due to canvas size, which must not happen for 1920x1080: {reason}"
            );
        }
        // Ok(_) is also acceptable if the movie source happened to be present.
    }

    #[test]
    fn composer_empty_layers_should_return_err() {
        let result = MultiTrackComposer::new(1920, 1080).build();
        assert!(
            matches!(result, Err(FilterError::CompositionFailed { .. })),
            "expected CompositionFailed, got {result:?}"
        );
    }

    #[test]
    fn video_layer_with_proxy_should_insert_scale_filter() {
        // When a proxy is set, a scale filter is inserted right after the movie
        // source to upscale to the original resolution. Build fails (nonexistent
        // file) but must NOT fail at "filter not found: scale".
        let result = MultiTrackComposer::new(1920, 1080)
            .add_layer(VideoLayer {
                source: "nonexistent.mp4".into(),
                proxy: Some(ProxySource {
                    path: "nonexistent_proxy_quarter.mp4".into(),
                    width: 1920,
                    height: 1080,
                }),
                input_format: None,
                x: AnimatedValue::Static(0.0),
                y: AnimatedValue::Static(0.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                opacity: AnimatedValue::Static(1.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                effects: vec![],
            })
            .build();
        assert!(result.is_err(), "expected error (nonexistent proxy file)");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("filter not found: scale"),
                "scale filter must exist and be created for proxy upscale; got: {reason}"
            );
        }
    }

    #[test]
    fn video_layer_with_offset_pts_effect_should_insert_setpts() {
        // A timeline offset is expressed by the caller as an OffsetPts effect,
        // which builds a `setpts` node. Build fails (nonexistent file) but NOT at
        // "filter not found: setpts".
        let result = MultiTrackComposer::new(1920, 1080)
            .add_layer(VideoLayer {
                source: "nonexistent.mp4".into(),
                proxy: None,
                input_format: None,
                x: AnimatedValue::Static(0.0),
                y: AnimatedValue::Static(0.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                opacity: AnimatedValue::Static(1.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                effects: vec![FilterStep::OffsetPts { seconds: 2.0 }],
            })
            .build();
        assert!(result.is_err(), "expected error (nonexistent file)");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("filter not found: setpts"),
                "setpts must exist in FFmpeg and be created; got: {reason}"
            );
        }
    }

    #[test]
    fn layer_without_timeline_effects_should_not_insert_setpts() {
        // A layer whose effects carry no Trim/OffsetPts inserts no `setpts` node.
        let result = MultiTrackComposer::new(1920, 1080)
            .add_layer(VideoLayer {
                source: "nonexistent.mp4".into(),
                proxy: None,
                input_format: None,
                x: AnimatedValue::Static(0.0),
                y: AnimatedValue::Static(0.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                opacity: AnimatedValue::Static(1.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                effects: vec![],
            })
            .build();
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("setpts"),
                "no setpts node must appear without timeline effects; got: {reason}"
            );
        }
    }
}
