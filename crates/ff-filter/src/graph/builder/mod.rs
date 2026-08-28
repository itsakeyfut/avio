//! [`FilterGraphBuilder`] — consuming builder for filter graphs.

use std::path::Path;
use std::time::Duration;

pub(super) use super::FilterGraph;
pub(super) use super::filter_step::FilterStep;
pub(super) use super::types::{
    DrawTextOptions, EqBand, HwAccel, Rgb, ScaleAlgorithm, ToneMap, XfadeTransition, YadifMode,
};
pub(super) use crate::animation::{AnimatedValue, AnimationEntry};
pub(super) use crate::blend::BlendMode;
pub(super) use crate::error::FilterError;
use crate::filter_inner::FilterGraphInner;

mod audio;
mod video;

// FilterGraphBuilder

/// Builder for constructing a [`FilterGraph`].
///
/// Create one with [`FilterGraph::builder()`], chain the desired filter
/// methods, then call [`build`](Self::build) to obtain the graph.
///
/// # Examples
///
/// ```ignore
/// use ff_filter::{FilterGraph, ToneMap};
///
/// let graph = FilterGraph::builder()
///     .scale(1280, 720)
///     .tone_map(ToneMap::Hable)
///     .build()?;
/// ```
#[derive(Debug, Default, Clone)]
pub struct FilterGraphBuilder {
    pub(super) steps: Vec<FilterStep>,
    pub(super) hw: Option<HwAccel>,
    /// Registered animation entries, transferred to [`FilterGraph`] on [`build()`](Self::build).
    pub(super) animations: Vec<AnimationEntry>,
}

impl FilterGraphBuilder {
    /// Creates an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the accumulated filter steps.
    ///
    /// Used by `filter_inner` to build sub-graphs (e.g. the top layer of a
    /// [`FilterStep::Blend`] compound step).
    pub(crate) fn steps(&self) -> &[FilterStep] {
        &self.steps
    }

    /// Appends a raw [`FilterStep`] to the chain.
    ///
    /// Lets callers build a graph from a `FilterStep` list they already hold —
    /// e.g. the same per-clip effects passed to `Clip::with_video_effect` — so a
    /// host can run the identical chain on a preview frame without reconstructing
    /// it through the typed builder methods.
    #[must_use]
    pub fn add_step(mut self, step: FilterStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Append an arbitrary `FFmpeg` avfilter as an effect step on the current
    /// stream — the escape hatch for filters not covered by the typed builder
    /// methods.
    ///
    /// `filter` is the avfilter name (e.g. `"selectivecolor"`) and `args` its
    /// option string (empty for none). The filter name is checked for existence
    /// at [`build`](Self::build); the `args` are validated by `FFmpeg` on the
    /// first push. Equivalent to
    /// `add_step(FilterStep::Raw { filter, args })`.
    #[must_use]
    pub fn raw_filter(self, filter: impl Into<String>, args: impl Into<String>) -> Self {
        self.add_step(FilterStep::Raw {
            filter: filter.into(),
            args: args.into(),
        })
    }

    /// Enable hardware-accelerated filtering.
    ///
    /// When set, `hwupload` and `hwdownload` filters are inserted around the
    /// filter chain automatically.
    #[must_use]
    pub fn hardware(mut self, hw: HwAccel) -> Self {
        self.hw = Some(hw);
        self
    }

    // Build

    /// Build the [`FilterGraph`].
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::BuildFailed`] if `steps` is empty (there is
    /// nothing to filter). The actual `FFmpeg` graph is constructed lazily on the
    /// first [`push_video`](FilterGraph::push_video) or
    /// [`push_audio`](FilterGraph::push_audio) call.
    pub fn build(self) -> Result<FilterGraph, FilterError> {
        if self.steps.is_empty() {
            return Err(FilterError::BuildFailed);
        }

        // Validate overlay coordinates: negative x or y places the overlay
        // entirely off-screen, which is almost always a misconfiguration
        // (e.g. a watermark larger than the video). Catch it early with a
        // descriptive error rather than silently producing invisible output.
        for step in &self.steps {
            if let FilterStep::ParametricEq { bands } = step
                && bands.is_empty()
            {
                return Err(FilterError::InvalidConfig {
                    reason: "equalizer bands must not be empty".to_string(),
                });
            }
            if let FilterStep::Speed { factor } = step
                && !(0.1..=100.0).contains(factor)
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("speed factor {factor} out of range [0.1, 100.0]"),
                });
            }
            if let FilterStep::LoudnessNormalize {
                target_lufs,
                true_peak_db,
                lra,
            } = step
            {
                if *target_lufs >= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "loudness_normalize target_lufs {target_lufs} must be < 0.0"
                        ),
                    });
                }
                if *true_peak_db > 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "loudness_normalize true_peak_db {true_peak_db} must be <= 0.0"
                        ),
                    });
                }
                if *lra <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("loudness_normalize lra {lra} must be > 0.0"),
                    });
                }
            }
            if let FilterStep::NormalizePeak { target_db } = step
                && *target_db > 0.0
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("normalize_peak target_db {target_db} must be <= 0.0"),
                });
            }
            if let FilterStep::FreezeFrame { pts, duration } = step {
                if *pts < 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("freeze_frame pts {pts} must be >= 0.0"),
                    });
                }
                if *duration <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("freeze_frame duration {duration} must be > 0.0"),
                    });
                }
            }
            if let FilterStep::Crop { width, height, .. } = step
                && (*width == 0 || *height == 0)
            {
                return Err(FilterError::InvalidConfig {
                    reason: "crop width and height must be > 0".to_string(),
                });
            }
            if let FilterStep::CropAnimated { width, height, .. } = step {
                let w0 = width.value_at(Duration::ZERO);
                let h0 = height.value_at(Duration::ZERO);
                if w0 <= 0.0 || h0 <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: "crop width and height must be > 0".to_string(),
                    });
                }
            }
            if let FilterStep::GBlurAnimated { sigma } = step {
                let s0 = sigma.value_at(Duration::ZERO);
                if s0 < 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("gblur sigma {s0} must be >= 0.0"),
                    });
                }
            }
            if let FilterStep::EqAnimated {
                brightness,
                contrast,
                saturation,
                gamma,
            } = step
            {
                let b = brightness.value_at(Duration::ZERO);
                if !(-1.0..=1.0).contains(&b) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("eq brightness {b} out of range [-1.0, 1.0]"),
                    });
                }
                let c = contrast.value_at(Duration::ZERO);
                if !(0.0..=3.0).contains(&c) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("eq contrast {c} out of range [0.0, 3.0]"),
                    });
                }
                let s = saturation.value_at(Duration::ZERO);
                if !(0.0..=3.0).contains(&s) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("eq saturation {s} out of range [0.0, 3.0]"),
                    });
                }
                let g = gamma.value_at(Duration::ZERO);
                if !(0.1..=10.0).contains(&g) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("eq gamma {g} out of range [0.1, 10.0]"),
                    });
                }
            }
            if let FilterStep::ColorBalanceAnimated { lift, gamma, gain } = step {
                for (label, av) in [("lift", lift), ("gamma", gamma), ("gain", gain)] {
                    let (r, g, b) = av.value_at(Duration::ZERO);
                    for (channel, v) in [("r", r), ("g", g), ("b", b)] {
                        if !(-1.0..=1.0).contains(&v) {
                            return Err(FilterError::InvalidConfig {
                                reason: format!(
                                    "color_correct {label}.{channel} {v} out of range [-1.0, 1.0]"
                                ),
                            });
                        }
                    }
                }
            }
            if let FilterStep::FadeIn { duration, .. }
            | FilterStep::FadeOut { duration, .. }
            | FilterStep::FadeInWhite { duration, .. }
            | FilterStep::FadeOutWhite { duration, .. } = step
                && *duration <= 0.0
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("fade duration {duration} must be > 0.0"),
                });
            }
            if let FilterStep::AFadeIn { duration, .. } | FilterStep::AFadeOut { duration, .. } =
                step
                && *duration <= 0.0
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("afade duration {duration} must be > 0.0"),
                });
            }
            if let FilterStep::XFade { duration, .. } = step
                && *duration <= 0.0
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("xfade duration {duration} must be > 0.0"),
                });
            }
            if let FilterStep::ANoiseGate {
                attack_ms,
                release_ms,
                ..
            } = step
            {
                if *attack_ms <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("agate attack_ms {attack_ms} must be > 0.0"),
                    });
                }
                if *release_ms <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("agate release_ms {release_ms} must be > 0.0"),
                    });
                }
            }
            if let FilterStep::ACompressor {
                ratio,
                attack_ms,
                release_ms,
                ..
            } = step
            {
                if *ratio < 1.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("compressor ratio {ratio} must be >= 1.0"),
                    });
                }
                if *attack_ms <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("compressor attack_ms {attack_ms} must be > 0.0"),
                    });
                }
                if *release_ms <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("compressor release_ms {release_ms} must be > 0.0"),
                    });
                }
            }
            if let FilterStep::ChannelMap { mapping } = step
                && mapping.is_empty()
            {
                return Err(FilterError::InvalidConfig {
                    reason: "channel_map mapping must not be empty".to_string(),
                });
            }
            if let FilterStep::ConcatVideo { n } = step
                && *n < 2
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("concat_video n={n} must be >= 2"),
                });
            }
            if let FilterStep::ConcatAudio { n } = step
                && *n < 2
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("concat_audio n={n} must be >= 2"),
                });
            }
            if let FilterStep::DrawText { opts } = step {
                if opts.text.is_empty() {
                    return Err(FilterError::InvalidConfig {
                        reason: "drawtext text must not be empty".to_string(),
                    });
                }
                if !(0.0..=1.0).contains(&opts.opacity) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "drawtext opacity {} out of range [0.0, 1.0]",
                            opts.opacity
                        ),
                    });
                }
            }
            if let FilterStep::Ticker {
                text,
                speed_px_per_sec,
                ..
            } = step
            {
                if text.is_empty() {
                    return Err(FilterError::InvalidConfig {
                        reason: "ticker text must not be empty".to_string(),
                    });
                }
                if *speed_px_per_sec <= 0.0 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("ticker speed_px_per_sec {speed_px_per_sec} must be > 0.0"),
                    });
                }
            }
            if let FilterStep::Overlay { x, y } = step
                && (*x < 0 || *y < 0)
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!(
                        "overlay position ({x}, {y}) is off-screen; \
                         ensure the watermark fits within the video dimensions"
                    ),
                });
            }
            if let FilterStep::Lut3d { path } = step {
                let ext = Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if !matches!(ext, "cube" | "3dl") {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("unsupported LUT format: .{ext}; expected .cube or .3dl"),
                    });
                }
                if !Path::new(path).exists() {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("LUT file not found: {path}"),
                    });
                }
            }
            if let FilterStep::SubtitlesSrt { path, .. } = step {
                let ext = Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if ext != "srt" {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("unsupported subtitle format: .{ext}; expected .srt"),
                    });
                }
                if !Path::new(path).exists() {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("subtitle file not found: {path}"),
                    });
                }
            }
            if let FilterStep::SubtitlesAss { path } = step {
                let ext = Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if !matches!(ext, "ass" | "ssa") {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "unsupported subtitle format: .{ext}; expected .ass or .ssa"
                        ),
                    });
                }
                if !Path::new(path).exists() {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("subtitle file not found: {path}"),
                    });
                }
            }
            if let FilterStep::ChromaKey {
                similarity, blend, ..
            } = step
            {
                if !(0.0..=1.0).contains(similarity) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "chromakey similarity {similarity} out of range [0.0, 1.0]"
                        ),
                    });
                }
                if !(0.0..=1.0).contains(blend) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("chromakey blend {blend} out of range [0.0, 1.0]"),
                    });
                }
            }
            if let FilterStep::ColorKey {
                similarity, blend, ..
            } = step
            {
                if !(0.0..=1.0).contains(similarity) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("colorkey similarity {similarity} out of range [0.0, 1.0]"),
                    });
                }
                if !(0.0..=1.0).contains(blend) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("colorkey blend {blend} out of range [0.0, 1.0]"),
                    });
                }
            }
            if let FilterStep::SpillSuppress { strength, .. } = step
                && !(0.0..=1.0).contains(strength)
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("spill_suppress strength {strength} out of range [0.0, 1.0]"),
                });
            }
            if let FilterStep::LumaKey {
                threshold,
                tolerance,
                softness,
                ..
            } = step
            {
                if !(0.0..=1.0).contains(threshold) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("lumakey threshold {threshold} out of range [0.0, 1.0]"),
                    });
                }
                if !(0.0..=1.0).contains(tolerance) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("lumakey tolerance {tolerance} out of range [0.0, 1.0]"),
                    });
                }
                if !(0.0..=1.0).contains(softness) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("lumakey softness {softness} out of range [0.0, 1.0]"),
                    });
                }
            }
            if let FilterStep::FeatherMask { radius } = step
                && *radius == 0
            {
                return Err(FilterError::InvalidConfig {
                    reason: "feather_mask radius must be > 0".to_string(),
                });
            }
            if let FilterStep::RectMask { width, height, .. } = step
                && (*width == 0 || *height == 0)
            {
                return Err(FilterError::InvalidConfig {
                    reason: "rect_mask width and height must be > 0".to_string(),
                });
            }
            if let FilterStep::PolygonMatte { vertices, .. } = step {
                if vertices.len() < 3 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "polygon_matte requires at least 3 vertices, got {}",
                            vertices.len()
                        ),
                    });
                }
                if vertices.len() > 16 {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "polygon_matte supports up to 16 vertices, got {}",
                            vertices.len()
                        ),
                    });
                }
                for &(x, y) in vertices {
                    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                        return Err(FilterError::InvalidConfig {
                            reason: format!(
                                "polygon_matte vertex ({x}, {y}) out of range [0.0, 1.0]"
                            ),
                        });
                    }
                }
            }
            if let FilterStep::OverlayImage { path, opacity, .. } = step {
                let ext = Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if ext != "png" {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("unsupported image format: .{ext}; expected .png"),
                    });
                }
                if !(0.0..=1.0).contains(opacity) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("overlay_image opacity {opacity} out of range [0.0, 1.0]"),
                    });
                }
                if !Path::new(path).exists() {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("overlay image not found: {path}"),
                    });
                }
            }
            if let FilterStep::Eq {
                brightness,
                contrast,
                saturation,
            } = step
            {
                if !(-1.0..=1.0).contains(brightness) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("eq brightness {brightness} out of range [-1.0, 1.0]"),
                    });
                }
                if !(0.0..=3.0).contains(contrast) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("eq contrast {contrast} out of range [0.0, 3.0]"),
                    });
                }
                if !(0.0..=3.0).contains(saturation) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("eq saturation {saturation} out of range [0.0, 3.0]"),
                    });
                }
            }
            if let FilterStep::Curves { master, r, g, b } = step {
                for (channel, pts) in [
                    ("master", master.as_slice()),
                    ("r", r.as_slice()),
                    ("g", g.as_slice()),
                    ("b", b.as_slice()),
                ] {
                    for &(x, y) in pts {
                        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                            return Err(FilterError::InvalidConfig {
                                reason: format!(
                                    "curves {channel} control point ({x}, {y}) out of range [0.0, 1.0]"
                                ),
                            });
                        }
                    }
                }
            }
            if let FilterStep::WhiteBalance {
                temperature_k,
                tint,
            } = step
            {
                if !(1000..=40000).contains(temperature_k) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "white_balance temperature_k {temperature_k} out of range [1000, 40000]"
                        ),
                    });
                }
                if !(-1.0..=1.0).contains(tint) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!("white_balance tint {tint} out of range [-1.0, 1.0]"),
                    });
                }
            }
            if let FilterStep::Hue { degrees } = step
                && !(-360.0..=360.0).contains(degrees)
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("hue degrees {degrees} out of range [-360.0, 360.0]"),
                });
            }
            if let FilterStep::Gamma { r, g, b } = step {
                for (channel, val) in [("r", r), ("g", g), ("b", b)] {
                    if !(0.1..=10.0).contains(val) {
                        return Err(FilterError::InvalidConfig {
                            reason: format!("gamma {channel} {val} out of range [0.1, 10.0]"),
                        });
                    }
                }
            }
            if let FilterStep::ThreeWayCC { gamma, .. } = step {
                for (channel, val) in [("r", gamma.r), ("g", gamma.g), ("b", gamma.b)] {
                    if val <= 0.0 {
                        return Err(FilterError::InvalidConfig {
                            reason: format!("three_way_cc gamma.{channel} {val} must be > 0.0"),
                        });
                    }
                }
            }
            if let FilterStep::Vignette { angle, .. } = step
                && !((0.0)..=std::f32::consts::FRAC_PI_2).contains(angle)
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("vignette angle {angle} out of range [0.0, π/2]"),
                });
            }
            if let FilterStep::Pad { width, height, .. } = step
                && (*width == 0 || *height == 0)
            {
                return Err(FilterError::InvalidConfig {
                    reason: "pad width and height must be > 0".to_string(),
                });
            }
            if let FilterStep::FitToAspect { width, height, .. } = step
                && (*width == 0 || *height == 0)
            {
                return Err(FilterError::InvalidConfig {
                    reason: "fit_to_aspect width and height must be > 0".to_string(),
                });
            }
            if let FilterStep::FillToAspect { width, height } = step
                && (*width == 0 || *height == 0)
            {
                return Err(FilterError::InvalidConfig {
                    reason: "fill_to_aspect width and height must be > 0".to_string(),
                });
            }
            if let FilterStep::GBlur { sigma } = step
                && *sigma < 0.0
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("gblur sigma {sigma} must be >= 0.0"),
                });
            }
            if let FilterStep::Unsharp {
                luma_strength,
                chroma_strength,
            } = step
            {
                if !(-1.5..=1.5).contains(luma_strength) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "unsharp luma_strength {luma_strength} out of range [-1.5, 1.5]"
                        ),
                    });
                }
                if !(-1.5..=1.5).contains(chroma_strength) {
                    return Err(FilterError::InvalidConfig {
                        reason: format!(
                            "unsharp chroma_strength {chroma_strength} out of range [-1.5, 1.5]"
                        ),
                    });
                }
            }
            if let FilterStep::Hqdn3d {
                luma_spatial,
                chroma_spatial,
                luma_tmp,
                chroma_tmp,
            } = step
            {
                for (name, val) in [
                    ("luma_spatial", luma_spatial),
                    ("chroma_spatial", chroma_spatial),
                    ("luma_tmp", luma_tmp),
                    ("chroma_tmp", chroma_tmp),
                ] {
                    if *val < 0.0 {
                        return Err(FilterError::InvalidConfig {
                            reason: format!("hqdn3d {name} {val} must be >= 0.0"),
                        });
                    }
                }
            }
            if let FilterStep::Nlmeans { strength } = step
                && (*strength < 1.0 || *strength > 30.0)
            {
                return Err(FilterError::InvalidConfig {
                    reason: format!("nlmeans strength {strength} out of range [1.0, 30.0]"),
                });
            }
        }

        crate::filter_inner::validate_filter_steps(&self.steps)?;
        let output_resolution = self.steps.iter().rev().find_map(|s| {
            if let FilterStep::Scale { width, height, .. } = s {
                Some((*width, *height))
            } else {
                None
            }
        });
        Ok(FilterGraph {
            inner: FilterGraphInner::new(self.steps, self.hw),
            output_resolution,
            pending_animations: self.animations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::AlphaMode;

    #[test]
    fn builder_empty_steps_should_return_error() {
        let result = FilterGraph::builder().build();
        assert!(
            matches!(result, Err(FilterError::BuildFailed)),
            "expected BuildFailed, got {result:?}"
        );
    }

    #[test]
    fn builder_steps_should_accumulate_in_order() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .scale(1280, 720, ScaleAlgorithm::Fast)
            .volume(-3.0)
            .build();
        assert!(
            result.is_ok(),
            "builder with multiple valid steps must succeed, got {result:?}"
        );
    }

    #[test]
    fn builder_with_valid_steps_should_succeed() {
        let result = FilterGraph::builder()
            .scale(1280, 720, ScaleAlgorithm::Fast)
            .build();
        assert!(
            result.is_ok(),
            "builder with a known filter step must succeed, got {result:?}"
        );
    }

    #[test]
    fn output_resolution_should_be_none_when_no_scale() {
        let fg = FilterGraph::builder().trim(0.0, 5.0).build().unwrap();
        assert_eq!(fg.output_resolution(), None);
    }

    #[test]
    fn output_resolution_should_be_last_scale_dimensions() {
        let fg = FilterGraph::builder()
            .scale(1280, 720, ScaleAlgorithm::Fast)
            .build()
            .unwrap();
        assert_eq!(fg.output_resolution(), Some((1280, 720)));
    }

    #[test]
    fn output_resolution_should_use_last_scale_when_multiple_present() {
        let fg = FilterGraph::builder()
            .scale(1920, 1080, ScaleAlgorithm::Fast)
            .scale(1280, 720, ScaleAlgorithm::Bicubic)
            .build()
            .unwrap();
        assert_eq!(fg.output_resolution(), Some((1280, 720)));
    }

    #[test]
    fn rgb_neutral_constant_should_have_all_channels_one() {
        assert_eq!(Rgb::NEUTRAL.r, 1.0);
        assert_eq!(Rgb::NEUTRAL.g, 1.0);
        assert_eq!(Rgb::NEUTRAL.b, 1.0);
    }

    // blend()

    #[test]
    fn blend_normal_full_opacity_should_use_overlay_filter() {
        // build() must succeed; filter_name() == "overlay" is validated inside
        // validate_filter_steps at build time.
        let top = FilterGraphBuilder::new().trim(0.0, 5.0);
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .blend(top, BlendMode::Normal, 1.0, AlphaMode::Straight)
            .build();
        assert!(
            result.is_ok(),
            "blend(Normal, opacity=1.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn blend_normal_half_opacity_should_apply_colorchannelmixer() {
        // build() must succeed; the colorchannelmixer step is added at graph
        // construction time (push_video) — tested end-to-end in integration tests.
        let top = FilterGraphBuilder::new().trim(0.0, 5.0);
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .blend(top, BlendMode::Normal, 0.5, AlphaMode::Straight)
            .build();
        assert!(
            result.is_ok(),
            "blend(Normal, opacity=0.5) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn blend_opacity_above_one_should_be_clamped_to_one() {
        // Clamping happens in blend(); out-of-range opacity must not cause build() to fail.
        let top = FilterGraphBuilder::new().trim(0.0, 5.0);
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .blend(top, BlendMode::Normal, 2.5, AlphaMode::Straight)
            .build();
        assert!(
            result.is_ok(),
            "blend with opacity=2.5 must clamp to 1.0 and build successfully, got {result:?}"
        );
    }

    #[test]
    fn colorkey_out_of_range_similarity_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .colorkey("green", 1.5, 0.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "colorkey similarity > 1.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn colorkey_out_of_range_blend_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .colorkey("green", 0.3, -0.1)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "colorkey blend < 0.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn lumakey_out_of_range_threshold_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .lumakey(1.5, 0.1, 0.0, false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "lumakey threshold > 1.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn lumakey_out_of_range_tolerance_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .lumakey(0.9, -0.1, 0.0, false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "lumakey tolerance < 0.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn lumakey_out_of_range_softness_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .lumakey(0.9, 0.1, 1.5, false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "lumakey softness > 1.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn spill_suppress_out_of_range_strength_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .spill_suppress("green", 1.5)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "spill_suppress strength > 1.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn spill_suppress_negative_strength_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .spill_suppress("green", -0.1)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "spill_suppress strength < 0.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn feather_mask_zero_radius_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .feather_mask(0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "feather_mask radius=0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn rect_mask_zero_width_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .rect_mask(0, 0, 0, 32, false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "rect_mask width=0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn rect_mask_zero_height_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .rect_mask(0, 0, 32, 0, false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "rect_mask height=0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn polygon_matte_fewer_than_3_vertices_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .polygon_matte(vec![(0.0, 0.0), (1.0, 0.0)], false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "polygon_matte with < 3 vertices must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn polygon_matte_more_than_16_vertices_should_return_invalid_config() {
        let verts = (0..17)
            .map(|i| {
                let angle = i as f32 * 2.0 * std::f32::consts::PI / 17.0;
                (0.5 + 0.4 * angle.cos(), 0.5 + 0.4 * angle.sin())
            })
            .collect();
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .polygon_matte(verts, false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "polygon_matte with > 16 vertices must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn polygon_matte_out_of_range_vertex_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .polygon_matte(vec![(0.0, 0.0), (1.5, 0.0), (0.0, 1.0)], false)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "polygon_matte with vertex x > 1.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn polygon_matte_geq_uses_valid_constants_and_no_bare_star_minus() {
        // Regression for two `geq` eval failures:
        // 1. `geq` has no `iw`/`ih` constants (those are scale/overlay's) — it uses
        //    `W`/`H`; using `iw`/`ih` fails with "Undefined constant".
        // 2. A diagonal edge has negative `dx`/`dy`; they must be parenthesised so
        //    the expression contains no bare `*-`/`/-`.
        let steps = FilterGraph::builder()
            .polygon_matte(vec![(0.2, 0.1), (0.9, 0.5), (0.3, 0.9)], false)
            .steps()
            .to_vec();
        let args = steps[0].args();
        assert!(
            !args.contains("iw") && !args.contains("ih"),
            "geq must use W/H, not iw/ih: {args}"
        );
        assert!(
            args.contains("*W") && args.contains("*H"),
            "geq should use W/H: {args}"
        );
        assert!(
            !args.contains("*-") && !args.contains("/-"),
            "geq expression must not contain a bare '*-'/'/-': {args}"
        );
    }

    #[test]
    fn chromakey_out_of_range_similarity_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .chromakey("green", 1.5, 0.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "chromakey similarity > 1.0 must return InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn chromakey_out_of_range_blend_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .trim(0.0, 5.0)
            .chromakey("green", 0.3, -0.1)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "chromakey blend < 0.0 must return InvalidConfig, got {result:?}"
        );
    }
}
