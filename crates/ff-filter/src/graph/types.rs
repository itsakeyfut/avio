//! Supporting types for [`super::FilterGraphBuilder`] and [`super::FilterGraph`].

// Supporting enums

/// Tone-mapping algorithm for HDR-to-SDR conversion.
///
/// Used with [`super::FilterGraphBuilder::tone_map`].
///
/// # Choosing an algorithm
///
/// | Variant | Characteristic | When to use |
/// |---------|---------------|-------------|
/// | [`Hable`](Self::Hable) | Filmic, rich contrast | Film / cinematic content |
/// | [`Reinhard`](Self::Reinhard) | Simple, fast, neutral | Fast previews, general video |
/// | [`Mobius`](Self::Mobius) | Smooth highlights | Bright outdoor or HDR10 content |
// Open catalog: `FFmpeg`'s `tonemap` supports more operators than are exposed here.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ToneMap {
    /// Hable (Uncharted 2) filmic tone mapping.
    ///
    /// Produces a warm, cinematic look with compressed shadows and highlights.
    /// The most commonly used algorithm for film and narrative video content.
    Hable,
    /// Reinhard tone mapping.
    ///
    /// A simple, globally uniform operator. Fast and neutral; a safe default
    /// when color-accurate reproduction matters more than filmic aesthetics.
    Reinhard,
    /// Mobius tone mapping.
    ///
    /// A smooth, shoulder-based curve that preserves mid-tones while gently
    /// rolling off bright highlights. Well suited for outdoor and HDR10 content.
    Mobius,
}

impl ToneMap {
    /// Returns the libavfilter `tonemap` algorithm name for this variant.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Hable => "hable",
            Self::Reinhard => "reinhard",
            Self::Mobius => "mobius",
        }
    }
}

/// Hardware acceleration backend for filter graph operations.
///
/// When set on the builder, upload/download filters are inserted automatically
/// around the filter chain. This is independent of `ff_decode::HardwareAccel`
/// and is defined here to avoid a hard dependency on `ff-decode`.
// Open catalog: more hardware backends (QSV, D3D11VA, Vulkan, …) can be added.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwAccel {
    /// NVIDIA CUDA.
    Cuda,
    /// Apple `VideoToolbox`.
    VideoToolbox,
    /// VA-API (Video Acceleration API, Linux).
    Vaapi,
}

/// An RGB colour value used by the three-way colour corrector.
///
/// Each channel is a multiplicative factor (neutral = `1.0`).
/// Values above `1.0` push the channel warmer/brighter; values below `1.0`
/// pull it cooler/darker.  Negative values are clamped at the `FFmpeg` layer.
///
/// See [`super::FilterGraphBuilder::three_way_cc`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rgb {
    /// Red channel multiplier (neutral: `1.0`).
    pub r: f32,
    /// Green channel multiplier (neutral: `1.0`).
    pub g: f32,
    /// Blue channel multiplier (neutral: `1.0`).
    pub b: f32,
}

impl Rgb {
    /// Neutral value — no colour shift on any channel.
    pub const NEUTRAL: Rgb = Rgb {
        r: 1.0,
        g: 1.0,
        b: 1.0,
    };
}

/// Resampling algorithm for the `scale` filter.
///
/// Used with [`super::FilterGraphBuilder::scale`].
// Open catalog: `swscale` exposes more flags (neighbor, area, gauss, …) than these.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ScaleAlgorithm {
    /// Fast bilinear interpolation (default). Good balance of speed and quality.
    Fast,
    /// Bilinear interpolation. Slightly slower than [`Fast`](Self::Fast) but
    /// produces smoother results.
    Bilinear,
    /// Bicubic interpolation. Higher quality than bilinear with moderate overhead.
    Bicubic,
    /// Lanczos interpolation — sharpest output, highest CPU cost.
    Lanczos,
}

impl ScaleAlgorithm {
    /// Returns the `sws_flags` string passed to the `scale` filter.
    #[must_use]
    pub const fn as_flags_str(self) -> &'static str {
        match self {
            Self::Fast => "fast_bilinear",
            Self::Bilinear => "bilinear",
            Self::Bicubic => "bicubic",
            Self::Lanczos => "lanczos",
        }
    }
}

/// Deinterlacing mode for the `yadif` filter.
///
/// Used with [`super::FilterGraphBuilder::yadif`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum YadifMode {
    /// Output one frame per frame (progressive output).
    Frame = 0,
    /// Output one frame per field (doubles the frame rate).
    Field = 1,
    /// Frame mode without spatial interlacing check.
    FrameNospatial = 2,
    /// Field mode without spatial interlacing check.
    FieldNospatial = 3,
}

/// Backend algorithm for the [`PitchShift`](super::FilterStep::PitchShift) and
/// [`TimeStretch`](super::FilterStep::TimeStretch) steps.
///
/// Used with [`super::FilterGraph::pitch_shift_rubberband`] and
/// [`super::FilterGraph::time_stretch_rubberband`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PitchAlgo {
    /// Signal-processing path: `asetrate` + `atempo` (pitch) or `atempo`
    /// (time-stretch). Always available, but shifts formants.
    #[default]
    Signal,
    /// `FFmpeg`'s `rubberband` filter: formant-preserving and higher quality.
    /// Requires an `FFmpeg` built `--enable-librubberband`; when the filter is
    /// absent the graph build falls back to [`Signal`](Self::Signal).
    Rubberband,
}

/// `FFmpeg`'s per-pixel noise for `xfade=dissolve` (`vf_xfade.c::frand`), keyed by
/// integer pixel coordinates and returning a value in `[0, 1)`.
///
/// Transcribed literally from the pinned C, and the literalness is load-bearing. The
/// argument reaches ~110 000 at 1080p, where `f32` resolves to about 0.008 and the
/// `* 43758.545` then the fractional part amplify any difference into an unrelated
/// value, so **the argument must be accumulated in `f32`**: computing it in `f64` yields
/// a pixel set 48% different from `FFmpeg`'s. With the `f32` argument, Rust's `f32::sin`
/// reproduces `FFmpeg`'s `sinf` for every pixel at every progress -- both measured
/// against a real export (#1732).
///
/// Lives here, beside [`XfadeTransition`], because it is a property of that transition
/// and every consumer needs the *same* one: the CPU reference in `ff-preview`, the GPU
/// node's mask in `ff-render`, and the export path in `avio` would otherwise each carry
/// a copy of arithmetic that has to agree bit for bit.
#[must_use]
pub fn xfade_frand(x: u32, y: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)] // pixel coordinates are exact in f32
    let arg = (x as f32) * 12.9898 + (y as f32) * 78.233;
    let r = arg.sin() * 43758.545;
    r - r.floor()
}

/// [`xfade_frand`] tabulated for a whole frame, row-major: `field[y * w + x]`.
///
/// The hash depends only on the pixel coordinates, so a dissolve that recomputes it every
/// frame is doing the same work `n` times. Building it once costs what one frame used to
/// (measured in release at 12.0 ms for 1080p and 47.4 ms for 4K), and every later frame
/// of the transition then reads it (1.7 ms and 7.7 ms), which is what brings a 4 K
/// dissolve back inside a 30 fps budget: 49.0 ms per frame today against 7.7 ms cached,
/// where the budget is 33.3 ms (#1736).
///
/// Held by the caller, because only the caller knows when the frame size changes: the
/// length is the whole of what ties a field to a frame, so a consumer must check
/// `field.len() == w * h` before trusting one rather than assume its own dimensions still
/// hold. `ff_preview::apply_xfade` does exactly that and falls back to computing.
#[must_use]
pub fn xfade_frand_field(w: u32, h: u32) -> Vec<f32> {
    let mut field = Vec::with_capacity((w as usize) * (h as usize));
    for y in 0..h {
        for x in 0..w {
            field.push(xfade_frand(x, y));
        }
    }
    field
}

#[cfg(test)]
mod frand_field_tests {
    use super::{xfade_frand, xfade_frand_field};

    #[test]
    fn xfade_frand_field_should_tabulate_xfade_frand_at_every_pixel() {
        // Deliberately not square: `w * h` alone cannot tell a row-major field from a
        // transposed one, so only a non-square frame catches an x/y swap.
        const W: u32 = 7;
        const H: u32 = 5;
        let field = xfade_frand_field(W, H);
        assert_eq!(field.len(), (W as usize) * (H as usize));
        for y in 0..H {
            for x in 0..W {
                let n = (y * W + x) as usize;
                assert_eq!(
                    field[n],
                    xfade_frand(x, y),
                    "field[{n}] must be frand({x}, {y}) exactly: the dissolve agrees with \
                     FFmpeg pixel for pixel, so an approximation here reveals a different set"
                );
            }
        }
    }

    #[test]
    fn xfade_frand_field_should_be_empty_for_a_zero_dimension() {
        assert!(xfade_frand_field(0, 4).is_empty());
        assert!(xfade_frand_field(4, 0).is_empty());
    }
}

/// The per-pixel selection `xfade=dissolve` makes at `progress`, as an RGBA mask: `255`
/// where clip B shows through, `0` where clip A does.
///
/// `vf_xfade.c` writes the choice as `smooth = frand(x,y)*2 + progress*2 - 1.5`, taking
/// clip A where `smooth >= 0.5`; with `FFmpeg`'s `progress` running 1 -> 0 that reduces
/// to clip B wherever [`xfade_frand`] is below `progress` in this crate's convention.
///
/// Exists as a *mask* so the GPU path can render the same dissolve: the hash cannot be
/// recomputed in `WGSL` (see [`xfade_frand`]), so `ff_render::DissolveTransitionNode`
/// takes this instead and the two paths agree by construction.
#[must_use]
pub fn dissolve_mask(w: u32, h: u32, progress: f32) -> Vec<u8> {
    let mut mask = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        for x in 0..w {
            if xfade_frand(x, y) < progress {
                let i = ((y * w + x) * 4) as usize;
                mask[i..i + 4].fill(255);
            }
        }
    }
    mask
}

/// Transition type for the `xfade` cross-dissolve filter.
///
/// Used with [`super::FilterGraphBuilder::xfade`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum XfadeTransition {
    /// Reveal clip B one pixel at a time, thresholding a pseudo-random value against
    /// the progress. **Not** a cross-blend: at 50% every pixel is still fully clip A or
    /// fully clip B, never a mix of the two. Use [`Fade`](Self::Fade) for a blend.
    Dissolve,
    /// Linear cross-blend: every pixel is `mix(A, B, progress)`, so at 50% the frame is
    /// the arithmetic mean of the two clips.
    Fade,
    /// Wipe from right to left.
    WipeLeft,
    /// Wipe from left to right.
    WipeRight,
    /// Wipe upward.
    WipeUp,
    /// Wipe downward.
    WipeDown,
    /// Slide from right.
    SlideLeft,
    /// Slide from left.
    SlideRight,
    /// Slide upward.
    SlideUp,
    /// Slide downward.
    SlideDown,
    /// Circular iris open.
    CircleOpen,
    /// Circular iris close.
    CircleClose,
    /// Fade through gray.
    FadeGrays,
    /// Pixelize transition.
    Pixelize,
    /// Dip through black: clip A fades to black, then black fades to clip B.
    /// `progress = 0.5` is the fully black frame.
    FadeBlack,
    /// Dip through white, the mirror of [`FadeBlack`](Self::FadeBlack).
    FadeWhite,
}

impl XfadeTransition {
    /// Returns the `FFmpeg` `xfade` transition name string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dissolve => "dissolve",
            Self::Fade => "fade",
            Self::WipeLeft => "wipeleft",
            Self::WipeRight => "wiperight",
            Self::WipeUp => "wipeup",
            Self::WipeDown => "wipedown",
            Self::SlideLeft => "slideleft",
            Self::SlideRight => "slideright",
            Self::SlideUp => "slideup",
            Self::SlideDown => "slidedown",
            Self::CircleOpen => "circleopen",
            Self::CircleClose => "circleclose",
            Self::FadeGrays => "fadegrays",
            Self::Pixelize => "pixelize",
            Self::FadeBlack => "fadeblack",
            Self::FadeWhite => "fadewhite",
        }
    }
}

/// A single band for the parametric equalizer.
///
/// Used with [`super::FilterGraphBuilder::equalizer`].
// Open catalog: more biquad band types (lowpass, highpass, notch, …) can be added.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EqBand {
    /// Low-shelf EQ: boosts or cuts all frequencies below `freq_hz`.
    ///
    /// `slope` controls the steepness of the shelf (typical range 0.1–1.0).
    LowShelf {
        /// Centre frequency in Hz.
        freq_hz: f64,
        /// Gain in dB (positive = boost, negative = cut).
        gain_db: f64,
        /// Shelf slope (0.1–1.0; 1.0 is the steepest shelf).
        slope: f64,
    },
    /// High-shelf EQ: boosts or cuts all frequencies above `freq_hz`.
    ///
    /// `slope` controls the steepness of the shelf (typical range 0.1–1.0).
    HighShelf {
        /// Centre frequency in Hz.
        freq_hz: f64,
        /// Gain in dB (positive = boost, negative = cut).
        gain_db: f64,
        /// Shelf slope (0.1–1.0; 1.0 is the steepest shelf).
        slope: f64,
    },
    /// Peaking (bell) EQ: boosts or cuts a band centred on `freq_hz`.
    ///
    /// Higher `q` values produce a narrower bell.
    Peak {
        /// Centre frequency in Hz.
        freq_hz: f64,
        /// Gain in dB (positive = boost, negative = cut).
        gain_db: f64,
        /// Q factor controlling bandwidth (higher Q = narrower band).
        q: f64,
    },
}

impl EqBand {
    /// Returns the `libavfilter` filter name for this band type.
    pub(crate) fn filter_name(&self) -> &'static str {
        match self {
            Self::LowShelf { .. } => "lowshelf",
            Self::HighShelf { .. } => "highshelf",
            Self::Peak { .. } => "equalizer",
        }
    }

    /// Returns the args string passed to `avfilter_graph_create_filter`.
    pub(crate) fn args(&self) -> String {
        match self {
            Self::LowShelf {
                freq_hz,
                gain_db,
                slope,
            } => format!("f={freq_hz}:g={gain_db}:s={slope}"),
            Self::HighShelf {
                freq_hz,
                gain_db,
                slope,
            } => format!("f={freq_hz}:g={gain_db}:s={slope}"),
            Self::Peak {
                freq_hz,
                gain_db,
                q,
            } => format!("f={freq_hz}:g={gain_db}:width_type=q:width={q}"),
        }
    }
}

/// Options for the `drawtext` filter.
///
/// Used with [`super::FilterGraphBuilder::drawtext`].
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DrawTextOptions {
    /// Text string (UTF-8). Special characters (`:`, `'`, `\`) are escaped automatically.
    pub text: String,
    /// X position as an `FFmpeg` expression string, e.g. `"(w-text_w)/2"` or `"10"`.
    pub x: String,
    /// Y position as an `FFmpeg` expression string, e.g. `"h-th-10"` or `"10"`.
    pub y: String,
    /// Font size in points.
    pub font_size: u32,
    /// Font color as an `FFmpeg` color string, e.g. `"white"` or `"0xFFFFFF"`.
    pub font_color: String,
    /// Optional path to a TrueType font file. Uses default font when `None`.
    pub font_file: Option<String>,
    /// Opacity 0.0 (transparent) to 1.0 (opaque), applied as an alpha channel on `fontcolor`.
    pub opacity: f32,
    /// Optional background box fill color, e.g. `"black@0.5"`. No box when `None`.
    pub box_color: Option<String>,
    /// Background box border width in pixels. Ignored when `box_color` is `None`.
    pub box_border_width: u32,
}

#[cfg(test)]
mod tests {
    use super::{dissolve_mask, xfade_frand};

    #[test]
    fn xfade_frand_should_stay_in_the_unit_interval() {
        // `dissolve_mask`'s `frand < progress` relies on the range: a value at or above
        // 1 would never be revealed even at full progress, and a negative one would be
        // revealed immediately.
        for y in 0..64u32 {
            for x in 0..64u32 {
                let v = xfade_frand(x, y);
                assert!((0.0..1.0).contains(&v), "frand({x}, {y}) = {v}");
            }
        }
    }

    #[test]
    fn dissolve_mask_endpoints_should_be_empty_then_full() {
        assert!(dissolve_mask(16, 16, 0.0).iter().all(|&m| m == 0));
        assert!(dissolve_mask(16, 16, 1.0).iter().all(|&m| m == 255));
    }

    #[test]
    fn dissolve_mask_should_reveal_about_progress_worth_of_pixels() {
        // `frand` stands in for a uniform draw, so the revealed fraction tracks progress.
        // Loose on purpose: the exact set is pinned against a real `FFmpeg` export by
        // `avio`'s `xfade_reference_parity`, and pinning it twice would only duplicate
        // that without adding a way to be wrong.
        let (w, h) = (64u32, 64u32);
        let total = f64::from(w * h);
        for (progress, want) in [(0.25f32, 0.25f64), (0.5, 0.5), (0.75, 0.75)] {
            let mask = dissolve_mask(w, h, progress);
            let set = mask.chunks_exact(4).filter(|px| px[0] == 255).count();
            #[allow(clippy::cast_precision_loss)]
            let ratio = set as f64 / total;
            assert!(
                (ratio - want).abs() < 0.08,
                "progress {progress} revealed {ratio:.3}, expected about {want}"
            );
        }
    }
}
