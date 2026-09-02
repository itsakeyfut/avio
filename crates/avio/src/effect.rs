//! Typed, re-editable per-clip effect model.
//!
//! A [`Clip`](crate::Clip) carries an ordered list of [`ClipEffect`]s, each a typed
//! [`EffectKind`] whose parameters are individually addressable [`Param`]s (a
//! constant or a keyframe track). This is the **authoring** layer: a host presents
//! and edits it parameter-by-parameter, enables/disables and reorders effects, and
//! keyframes any parameter, all through the id-addressed edit commands
//! ([`Command::AddEffect`](crate::Command::AddEffect) and siblings).
//!
//! It derives **down** to the execution layer: [`EffectKind::to_filter_step`] maps
//! each kind to a [`FilterStep`], so an effect renders exactly as the equivalent
//! hand-built `FilterStep` did before this model existed. The `ff-*` primitives stay
//! model-free — this type lives in `avio` because it exists to make a *clip's*
//! effects re-editable (a CLIP/EDIT concern per the engine/primitive litmus).

use std::ops::RangeInclusive;
use std::time::Duration;

use ff_filter::{AnimatedValue, AnimationTrack, FilterStep, Keyframe, Rgb};

use crate::ids::EffectId;

/// One parameter of an [`EffectKind`]: a constant value or a keyframe track.
///
/// `AnimationTrack` carries no `PartialEq`, so neither does `Param`; compare the
/// [`Const`](Self::Const) payload directly where equality is needed.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Param {
    /// A fixed value, unchanging over the clip's duration.
    Const(f64),
    /// A keyframed value animated over time.
    Animated(AnimationTrack<f64>),
}

impl Param {
    /// Whether this parameter is a [`Const`](Self::Const) (not keyframed).
    #[must_use]
    pub fn is_const(&self) -> bool {
        matches!(self, Param::Const(_))
    }

    /// The value of a [`Const`](Self::Const), else `None` for an animated parameter.
    #[must_use]
    pub fn as_const(&self) -> Option<f64> {
        match self {
            Param::Const(v) => Some(*v),
            Param::Animated(_) => None,
        }
    }

    /// Projects this parameter onto the `ff-filter` animation model: a constant
    /// becomes [`AnimatedValue::Static`], a track becomes [`AnimatedValue::Track`].
    fn to_animated(&self) -> AnimatedValue<f64> {
        match self {
            Param::Const(v) => AnimatedValue::Static(*v),
            Param::Animated(track) => AnimatedValue::Track(track.clone()),
        }
    }
}

/// Which of a clip's media streams an [`EffectKind`] applies to (#1712).
///
/// A clip keeps **one** ordered effect list ([`Clip::effects`](crate::Clip::effects))
/// for both domains, so effect ids, enable/disable, reordering, undo history and
/// [`descriptor`](EffectKind::descriptor) are shared machinery. Each derive path
/// selects its own domain (see [`Clip::video_effect_chain`](crate::Clip::video_effect_chain)
/// and [`Clip::audio_effect_chain`](crate::Clip::audio_effect_chain)), preserving the
/// relative order within that domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum EffectDomain {
    /// Applies to the clip's video frames.
    Video,
    /// Applies to the clip's audio samples.
    Audio,
}

/// A host-facing description of an effect kind and its editable parameters, returned
/// by [`EffectKind::descriptor`]. Lets a UI render a parameter panel generically,
/// without hard-coding each [`EffectKind`] variant.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct EffectDescriptor {
    /// Stable `snake_case` name of the kind (e.g. `"color_correct"`, `"motion_blur"`).
    pub name: &'static str,
    /// The kind's parameters, in a stable order.
    pub params: Vec<ParamDescriptor>,
}

/// A host-facing description of one effect parameter (see [`EffectDescriptor`]).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ParamDescriptor {
    /// Stable parameter name (e.g. `"brightness"`, `"shadows_lift.r"`).
    pub name: &'static str,
    /// The parameter's type, editable metadata, and current value.
    pub value: ParamValue,
}

/// The type, editable metadata, and current value of an effect parameter. The variant
/// tells a host which editor to render (slider / checkbox / number field / colour
/// picker / file picker / curve editor).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ParamValue {
    /// A keyframeable scalar (a slider). `current` is the constant value, or `None`
    /// when the parameter is animated by a keyframe track. An open-ended range uses
    /// [`f64::INFINITY`] as the upper bound (the host picks a UI maximum).
    Scalar {
        /// Valid inclusive range for the value.
        range: RangeInclusive<f64>,
        /// Neutral / starting default.
        default: f64,
        /// The constant value, or `None` if the parameter is animated.
        current: Option<f64>,
    },
    /// A structural on/off toggle (a checkbox).
    Bool {
        /// Default toggle state.
        default: bool,
        /// Current toggle state.
        current: bool,
    },
    /// A structural integer within a range (a number field).
    Int {
        /// Valid inclusive range for the value.
        range: RangeInclusive<i64>,
        /// Default value.
        default: i64,
        /// Current value.
        current: i64,
    },
    /// A structural RGB colour, each channel `0.0..=1.0` (a colour picker).
    Color {
        /// Current colour.
        current: [f32; 3],
    },
    /// A structural file path (a file picker). Empty = none.
    Path {
        /// Current path.
        current: String,
    },
    /// A structural set of `[input, output]` curve control points (a curve editor).
    Points {
        /// Current control points.
        current: Vec<[f32; 2]>,
    },
}

/// Builds a [`ParamValue::Scalar`] whose `current` is the constant value, or `None`
/// when `p` is animated.
fn scalar(range: RangeInclusive<f64>, default: f64, p: &Param) -> ParamValue {
    ParamValue::Scalar {
        range,
        default,
        current: p.as_const(),
    }
}

// Serde defaults for the parameter fields (#1709). Each parameter field carries its
// neutral as a `serde(default = ...)`, so a document written before that field existed
// still loads. Every value here must equal the `default` the same parameter reports
// from `descriptor()` — `deserializing_omitted_effect_fields_should_yield_descriptor_defaults`
// fails if the two ever drift apart.
#[cfg(feature = "serde")]
fn param_zero() -> Param {
    Param::Const(0.0)
}

#[cfg(feature = "serde")]
fn param_one() -> Param {
    Param::Const(1.0)
}

#[cfg(feature = "serde")]
fn param3_zero() -> [Param; 3] {
    [param_zero(), param_zero(), param_zero()]
}

#[cfg(feature = "serde")]
fn param3_one() -> [Param; 3] {
    [param_one(), param_one(), param_one()]
}

#[cfg(feature = "serde")]
fn glow_threshold_default() -> Param {
    Param::Const(0.8)
}

#[cfg(feature = "serde")]
fn glow_radius_default() -> Param {
    Param::Const(4.0)
}

/// The GPU node clamps to `2..=8`; `u8::default()` (`0`) is not a valid trail length.
#[cfg(feature = "serde")]
fn sub_frames_default() -> u8 {
    8
}

/// Green screen — the common key, and inert while `similarity` is at its neutral `0.0`.
#[cfg(feature = "serde")]
fn key_color_default() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}

/// A typed effect that a clip can carry. `#[non_exhaustive]`: more kinds are added
/// over time, so external matchers must include a `_` arm.
///
/// The set grows as effect nodes are wired into the GPU bridge; each kind documents
/// the `ff-filter` step / `ff-render` node it maps to.
///
/// # Serialization compatibility (#1709)
///
/// Every parameter field carries its **neutral** as a `serde` default, so a document
/// serialized before that field existed still loads — the missing field simply takes
/// its neutral and the effect renders as it did then. A new field added to an existing
/// variant **must** follow this, and its neutral **must** equal the `default` the field
/// reports from [`descriptor`](Self::descriptor); the
/// `deserializing_omitted_effect_fields_should_yield_descriptor_defaults` test fails if
/// they drift apart.
///
/// The exception is a field with no meaningful neutral — [`Raw::step`](Self::Raw) and
/// [`AudioRaw::step`](Self::AudioRaw) carry a whole `FilterStep`, which has no neutral
/// value, so they stay required.
///
/// The trade-off is deliberate: because parameters are optional on the wire, a
/// truncated or hand-edited document also loads, with the missing parameters at their
/// neutral, rather than being rejected.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum EffectKind {
    /// Brightness / contrast / saturation / temperature / tint adjustment (the `eq`
    /// filter on the CPU path, `ff_render::ColorGradeNode` on the GPU).
    ///
    /// Neutral parameters (`brightness = 0.0`, `contrast = 1.0`, `saturation = 1.0`,
    /// `temperature = 0.0`, `tint = 0.0`, all constant) compile to no filter at all,
    /// preserving bit-identical output.
    ///
    /// `temperature`/`tint` are a GPU-only enrichment: the CPU `eq` fallback applies
    /// brightness/contrast/saturation only and does not reproduce them (`FFmpeg` `eq`
    /// has no such parameter), while the GPU-default path applies the full grade.
    ColorCorrect {
        /// Brightness offset. Range −1.0..=1.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        brightness: Param,
        /// Contrast multiplier. Range 0.0..=3.0 (neutral: 1.0).
        #[cfg_attr(feature = "serde", serde(default = "param_one"))]
        contrast: Param,
        /// Saturation multiplier. Range 0.0..=3.0 (neutral: 1.0).
        #[cfg_attr(feature = "serde", serde(default = "param_one"))]
        saturation: Param,
        /// Colour temperature offset. Range −1.0..=1.0 (neutral: 0.0; −1.0 cool/blue,
        /// +1.0 warm/orange). GPU-only (not applied by the CPU `eq` fallback).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        temperature: Param,
        /// Colour tint offset. Range −1.0..=1.0 (neutral: 0.0; −1.0 magenta, +1.0
        /// green). GPU-only (not applied by the CPU `eq` fallback).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        tint: Param,
    },
    /// Gaussian blur (the `gblur` filter).
    Blur {
        /// Blur radius (standard deviation). Must be ≥ 0.0.
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        radius: Param,
    },
    /// Unsharp-mask sharpen (the `unsharp` filter on the CPU path,
    /// `ff_render::SharpenNode` on the GPU).
    ///
    /// A single luma sharpening amount in `[−1.5, 1.5]` (negative blurs). Because
    /// `FFmpeg`'s `unsharp` has no runtime-settable parameter, an animated `amount`
    /// animates on the GPU-default path but renders its `t = 0` value on the CPU
    /// fallback.
    Sharpen {
        /// Sharpening amount (luma). Range −1.5..=1.5 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        amount: Param,
    },
    /// Vignette (the `vignette` filter on the CPU path, `ff_render::VignetteNode`
    /// on the GPU).
    ///
    /// A single normalised darkening amount in `[0, 1]` (`0.0` = no vignette),
    /// centred on the frame. The `vignette` filter re-evaluates its angle per frame,
    /// so an animated `amount` animates on both paths.
    Vignette {
        /// Darkening amount. Range 0.0..=1.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        amount: Param,
    },
    /// Temporal film grain (the `noise` filter on the CPU path,
    /// `ff_render::FilmGrainNode` on the GPU).
    ///
    /// Luma and chroma grain strengths in the `noise` filter's `[0, 100]` scale
    /// (`0.0` = none). The grain pattern varies per frame on both paths; because
    /// `noise` has no runtime-settable parameter, an animated strength animates on
    /// the GPU-default path but renders its `t = 0` value on the CPU fallback.
    FilmGrain {
        /// Luma-plane grain strength. Range 0.0..=100.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        luma_strength: Param,
        /// Chroma-plane grain strength. Range 0.0..=100.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        chroma_strength: Param,
    },
    /// Glow / bloom (the compound `split`/`curves`/`gblur`/`blend` chain on the CPU
    /// path, `ff_render::GlowNode` on the GPU).
    ///
    /// Extracts highlights above `threshold`, blurs them by `radius`, and adds them
    /// back weighted by `intensity`. An animated parameter animates on the GPU-default
    /// path; the CPU renders its `t = 0` value (the glow sub-filters have no runtime
    /// parameter).
    Glow {
        /// Luminance threshold that triggers the glow. Range 0.0..=1.0.
        #[cfg_attr(feature = "serde", serde(default = "glow_threshold_default"))]
        threshold: Param,
        /// Gaussian blur radius (sigma) in pixels. Range 0.5..=50.0.
        #[cfg_attr(feature = "serde", serde(default = "glow_radius_default"))]
        radius: Param,
        /// Additive blend strength. Range 0.0..=2.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        intensity: Param,
    },
    /// Three-way (lift/gamma/gain) colour corrector (the `curves` filter on the CPU
    /// path, `ff_render::ColorWheelsNode` on the GPU).
    ///
    /// Each wheel is a per-channel `[R, G, B]` array. Neutral parameters
    /// (`shadows_lift = 0.0`, `midtones_gamma = 1.0`, `highlights_gain = 1.0`, all
    /// constant) compile to no filter at all. Because `curves` takes string options,
    /// an animated parameter animates on the GPU-default path but renders its `t = 0`
    /// value on the CPU fallback.
    ColorWheels {
        /// Shadows lift, additive per channel. Range −1.0..=1.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param3_zero"))]
        shadows_lift: [Param; 3],
        /// Midtones gamma, per channel. Range 0.1..=10.0 (neutral: 1.0; must be > 0).
        #[cfg_attr(feature = "serde", serde(default = "param3_one"))]
        midtones_gamma: [Param; 3],
        /// Highlights gain, per channel. Range 0.0..=4.0 (neutral: 1.0).
        #[cfg_attr(feature = "serde", serde(default = "param3_one"))]
        highlights_gain: [Param; 3],
    },
    /// Per-channel tone curves (the `curves` filter on the CPU path,
    /// `ff_render::CurvesNode` on the GPU).
    ///
    /// Each curve is a list of `[input, output]` control points in `[0, 1]`. Unlike
    /// the other kinds, a curve is a structural parameter, not a scalar, so it carries
    /// no keyframeable [`Param`]; an empty set of curves is a no-op. The master curve
    /// applies to every channel, then the per-channel curve.
    Curves {
        /// Master curve control points (applied to every channel). Empty = identity.
        #[cfg_attr(feature = "serde", serde(default))]
        master: Vec<[f32; 2]>,
        /// Red channel curve control points. Empty = identity.
        #[cfg_attr(feature = "serde", serde(default))]
        red: Vec<[f32; 2]>,
        /// Green channel curve control points. Empty = identity.
        #[cfg_attr(feature = "serde", serde(default))]
        green: Vec<[f32; 2]>,
        /// Blue channel curve control points. Empty = identity.
        #[cfg_attr(feature = "serde", serde(default))]
        blue: Vec<[f32; 2]>,
    },
    /// HSL adjustment (the `hue` filter on the CPU path, `ff_render::HslNode` on
    /// the GPU).
    ///
    /// A hue rotation in degrees, a saturation multiplier, and a lightness offset.
    /// The CPU `hue` filter works in YUV (chroma rotation plus a luma-add
    /// brightness), so it approximates the GPU node's HSL-space adjustment within a
    /// documented tolerance. Neutral parameters (`hue_shift = 0.0`,
    /// `saturation = 1.0`, `lightness = 0.0`, all constant) compile to no filter.
    /// Because `hue`'s options are string expressions, an animated parameter
    /// animates on the GPU-default path but renders its `t = 0` value on the CPU
    /// fallback.
    Hsl {
        /// Hue rotation in degrees. Range −180.0..=180.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        hue_shift: Param,
        /// Saturation multiplier. Range 0.0..=2.0 (neutral: 1.0).
        #[cfg_attr(feature = "serde", serde(default = "param_one"))]
        saturation: Param,
        /// Lightness offset. Range −1.0..=1.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        lightness: Param,
    },
    /// 3D colour LUT (the `lut3d` filter on the CPU path, `ff_render::LutNode` on
    /// the GPU), loaded from an Adobe `.cube` or Resolve `.3dl` file.
    ///
    /// Like [`Curves`](Self::Curves), a LUT is a structural parameter (a file
    /// path), not a scalar, so it carries no keyframeable [`Param`]; an empty path
    /// is a no-op. On the GPU a file that cannot be loaded (missing, malformed, or
    /// an unsupported extension) falls back to the CPU path.
    Lut {
        /// Path to the `.cube` / `.3dl` LUT file. Empty = identity.
        #[cfg_attr(feature = "serde", serde(default))]
        path: String,
    },
    /// Chroma-key (green-screen removal): makes pixels near `key_color`
    /// transparent (the `chromakey` filter on the CPU path, `ff_render::ChromaKeyNode`
    /// on the GPU).
    ///
    /// `key_color` is a structural RGB triple (each channel `0.0..=1.0`), not a
    /// keyframeable [`Param`] — the key colour is picked once, like a
    /// [`Lut`](Self::Lut) path. `similarity` (match radius) and `softness` (edge
    /// feather) are keyframeable. A constant `similarity = 0.0` removes nothing, so
    /// it compiles to no filter. Because `chromakey`'s options are static, an
    /// animated parameter animates on the GPU-default path but renders its `t = 0`
    /// value on the CPU fallback (like [`Hsl`](Self::Hsl)).
    ChromaKey {
        /// Key colour in RGB, each channel `0.0..=1.0`.
        #[cfg_attr(feature = "serde", serde(default = "key_color_default"))]
        key_color: [f32; 3],
        /// Match radius in `0.0..=1.0` (neutral: `0.0` removes nothing).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        similarity: Param,
        /// Edge softness in `0.0..=1.0` (`0.0` = hard edge).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        softness: Param,
    },
    /// Luma mask: multiplies the clip's alpha by its own BT.709 luma (the `geq`
    /// filter on the CPU path, `ff_render::LumaMaskNode` on the GPU). Bright pixels
    /// stay opaque, dark pixels become transparent; `invert` uses `1 - luma`.
    ///
    /// This is a structural effect with no scalar [`Param`] — like [`Lut`](Self::Lut)
    /// and [`Curves`](Self::Curves), its "keyframeable per parameter" requirement is
    /// satisfied vacuously (there is nothing to animate; `invert` is a one-time
    /// toggle). The mask is the clip's own frame, so no external mask source is
    /// needed.
    LumaMask {
        /// When `true`, mask by `1 - luma` (dark pixels stay opaque).
        #[cfg_attr(feature = "serde", serde(default))]
        invert: bool,
    },
    /// Rectangular shape mask: keeps the clip opaque inside the rectangle and clears
    /// the alpha outside (the `geq` filter on the CPU path, `ff_render::ShapeMaskNode`
    /// on the GPU). `invert` swaps inside and outside.
    ///
    /// `x` / `y` / `width` / `height` are keyframeable per-pixel [`Param`]s (a moving
    /// or resizing mask), so a constant rectangle compiles to the `RectMask` filter
    /// and any animated bound uses `RectMaskAnimated` (the GPU animates per frame; the
    /// CPU renders its `t = 0` bounds, like [`ChromaKey`](Self::ChromaKey)). A constant
    /// zero `width` or `height` masks nothing, so it compiles to no filter.
    ShapeMask {
        /// Left edge of the rectangle, in pixels.
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        x: Param,
        /// Top edge of the rectangle, in pixels.
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        y: Param,
        /// Rectangle width, in pixels (a constant `0` is a no-op).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        width: Param,
        /// Rectangle height, in pixels (a constant `0` is a no-op).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        height: Param,
        /// When `true`, keep the exterior and clear the interior.
        #[cfg_attr(feature = "serde", serde(default))]
        invert: bool,
    },
    /// Motion blur (the `tblend` filter on the CPU path, `ff_render::MotionBlurNode`
    /// on the GPU): a per-clip exposure trail that blends each frame with the
    /// accumulated previous output.
    ///
    /// `shutter_angle` is keyframeable per the typed-effect model, but motion blur is
    /// **stateful** — the trail accumulates across successive frames on one node
    /// instance — so a stable shutter is required and the value at `t = 0` is used on
    /// both paths (the CPU `tblend` likewise has no runtime shutter parameter). A
    /// constant `shutter_angle = 0.0` is no blur, so it compiles to no filter.
    /// `sub_frames` is a structural trail-length count (clamped `2..=8` by the GPU
    /// node); the CPU `tblend` ignores it (it blends only the two most recent frames),
    /// a documented GPU/CPU divergence.
    MotionBlur {
        /// Shutter angle in degrees. Range 0.0..=360.0 (`0.0` = no blur, `180.0` =
        /// standard film blur). Keyframeable, but rendered at its `t = 0` value.
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        shutter_angle: Param,
        /// Trail-length sub-frame count (the GPU node clamps it to `2..=8`; the CPU
        /// `tblend` ignores it).
        #[cfg_attr(feature = "serde", serde(default = "sub_frames_default"))]
        sub_frames: u8,
    },
    /// Escape hatch: a raw [`FilterStep`] the typed model has no variant for yet
    /// (`Hue`, `HFlip`, `Crop`, `Denoise`, ...).
    ///
    /// This is the **only** way to attach an untyped step, so every effect a clip
    /// carries still lives in one ordered, id-addressed list: a raw step can be
    /// enabled/disabled, reordered and removed through the `*Effect` commands like any
    /// other effect. Its interior is opaque, though — the step's own arguments are not
    /// individually keyframeable [`Param`]s and [`descriptor`](Self::descriptor)
    /// reports no parameters for it. Prefer a typed kind whenever one exists.
    Raw {
        /// The step rendered verbatim, in this effect's position in the chain.
        step: FilterStep,
    },
    /// Audio gain in decibels (the `volume` filter) — an [`EffectDomain::Audio`] effect.
    ///
    /// A neutral constant (`0.0` dB) compiles to no filter at all, preserving
    /// bit-identical audio. `gain_db` is keyframeable per the typed model, but
    /// `FFmpeg`'s `volume` step carries no runtime-settable parameter here, so an
    /// animated gain renders its `t = 0` value (the same documented limitation as
    /// [`Sharpen`](Self::Sharpen) and [`MotionBlur`](Self::MotionBlur)).
    Volume {
        /// Gain in decibels. Range −60.0..=30.0 (neutral: 0.0).
        #[cfg_attr(feature = "serde", serde(default = "param_zero"))]
        gain_db: Param,
    },
    /// Escape hatch for a raw audio [`FilterStep`] the typed model has no variant for
    /// (`ACompressor`, `NoiseReduce`, `ParametricEq`, ...) — the audio counterpart of
    /// [`Raw`](Self::Raw), and an [`EffectDomain::Audio`] effect.
    ///
    /// Like `Raw` it is opaque: the step's arguments are not individually keyframeable
    /// [`Param`]s and [`descriptor`](Self::descriptor) reports no parameters for it.
    AudioRaw {
        /// The step rendered verbatim, in this effect's position in the audio chain.
        step: FilterStep,
    },
}

/// Projects a `shadows_lift` parameter (additive, neutral `0.0`) onto the
/// `ThreeWayCC` `lift` convention (multiplicative-style, neutral `1.0`) by offsetting
/// the value by `+1.0`. A `Track` is rebuilt keyframe-by-keyframe (an animation track
/// has no scalar-offset op).
fn lift_to_animated(p: &Param) -> AnimatedValue<f64> {
    match p {
        Param::Const(v) => AnimatedValue::Static(v + 1.0),
        Param::Animated(track) => AnimatedValue::Track(track.keyframes().iter().fold(
            AnimationTrack::new(),
            |t, kf| {
                t.push(Keyframe {
                    timestamp: kf.timestamp,
                    value: kf.value + 1.0,
                    easing: kf.easing.clone(),
                })
            },
        )),
    }
}

impl EffectKind {
    /// Which media stream this kind applies to (#1712).
    ///
    /// A clip keeps one effect list for both domains; each derive path selects its own
    /// domain, so the value here decides whether a kind reaches
    /// [`Clip::video_effect_chain`](crate::Clip::video_effect_chain) or
    /// [`Clip::audio_effect_chain`](crate::Clip::audio_effect_chain).
    ///
    /// The `match` is exhaustive with **no** `_` arm: a new [`EffectKind`] variant
    /// fails to compile until its domain is declared, so a kind can never silently
    /// default to the wrong pipeline (RK-003).
    #[must_use]
    pub fn domain(&self) -> EffectDomain {
        match self {
            EffectKind::ColorCorrect { .. }
            | EffectKind::Blur { .. }
            | EffectKind::Sharpen { .. }
            | EffectKind::Vignette { .. }
            | EffectKind::FilmGrain { .. }
            | EffectKind::Glow { .. }
            | EffectKind::ColorWheels { .. }
            | EffectKind::Curves { .. }
            | EffectKind::Hsl { .. }
            | EffectKind::Lut { .. }
            | EffectKind::ChromaKey { .. }
            | EffectKind::LumaMask { .. }
            | EffectKind::ShapeMask { .. }
            | EffectKind::MotionBlur { .. }
            | EffectKind::Raw { .. } => EffectDomain::Video,
            EffectKind::Volume { .. } | EffectKind::AudioRaw { .. } => EffectDomain::Audio,
        }
    }

    /// Host-facing introspection: the kind's stable `snake_case` name and a
    /// [`ParamDescriptor`] for every parameter (name, type, range, default, current
    /// value), so a UI can render an editable parameter panel without hard-coding each
    /// variant.
    ///
    /// The `match` is exhaustive with **no** `_` arm (per #1640): adding an
    /// [`EffectKind`] variant without a descriptor here fails to compile, which
    /// guarantees the descriptor stays complete for every variant. This is in-crate, so
    /// `#[non_exhaustive]` does not force a wildcard arm (RK-003). Ranges and defaults
    /// come from each parameter's documented range on the variant; an open-ended range
    /// uses [`f64::INFINITY`] as the upper bound.
    #[must_use]
    pub fn descriptor(&self) -> EffectDescriptor {
        match self {
            EffectKind::ColorCorrect {
                brightness,
                contrast,
                saturation,
                temperature,
                tint,
            } => EffectDescriptor {
                name: "color_correct",
                params: vec![
                    ParamDescriptor {
                        name: "brightness",
                        value: scalar(-1.0..=1.0, 0.0, brightness),
                    },
                    ParamDescriptor {
                        name: "contrast",
                        value: scalar(0.0..=3.0, 1.0, contrast),
                    },
                    ParamDescriptor {
                        name: "saturation",
                        value: scalar(0.0..=3.0, 1.0, saturation),
                    },
                    ParamDescriptor {
                        name: "temperature",
                        value: scalar(-1.0..=1.0, 0.0, temperature),
                    },
                    ParamDescriptor {
                        name: "tint",
                        value: scalar(-1.0..=1.0, 0.0, tint),
                    },
                ],
            },
            EffectKind::Blur { radius } => EffectDescriptor {
                name: "blur",
                params: vec![ParamDescriptor {
                    name: "radius",
                    value: scalar(0.0..=f64::INFINITY, 0.0, radius),
                }],
            },
            EffectKind::Sharpen { amount } => EffectDescriptor {
                name: "sharpen",
                params: vec![ParamDescriptor {
                    name: "amount",
                    value: scalar(-1.5..=1.5, 0.0, amount),
                }],
            },
            EffectKind::Vignette { amount } => EffectDescriptor {
                name: "vignette",
                params: vec![ParamDescriptor {
                    name: "amount",
                    value: scalar(0.0..=1.0, 0.0, amount),
                }],
            },
            EffectKind::FilmGrain {
                luma_strength,
                chroma_strength,
            } => EffectDescriptor {
                name: "film_grain",
                params: vec![
                    ParamDescriptor {
                        name: "luma_strength",
                        value: scalar(0.0..=100.0, 0.0, luma_strength),
                    },
                    ParamDescriptor {
                        name: "chroma_strength",
                        value: scalar(0.0..=100.0, 0.0, chroma_strength),
                    },
                ],
            },
            // threshold / radius have no documented neutral (glow is a no-op at
            // intensity 0), so a sensible starting default is used.
            EffectKind::Glow {
                threshold,
                radius,
                intensity,
            } => EffectDescriptor {
                name: "glow",
                params: vec![
                    ParamDescriptor {
                        name: "threshold",
                        value: scalar(0.0..=1.0, 0.8, threshold),
                    },
                    ParamDescriptor {
                        name: "radius",
                        value: scalar(0.5..=50.0, 4.0, radius),
                    },
                    ParamDescriptor {
                        name: "intensity",
                        value: scalar(0.0..=2.0, 0.0, intensity),
                    },
                ],
            },
            EffectKind::ColorWheels {
                shadows_lift,
                midtones_gamma,
                highlights_gain,
            } => EffectDescriptor {
                name: "color_wheels",
                params: vec![
                    ParamDescriptor {
                        name: "shadows_lift.r",
                        value: scalar(-1.0..=1.0, 0.0, &shadows_lift[0]),
                    },
                    ParamDescriptor {
                        name: "shadows_lift.g",
                        value: scalar(-1.0..=1.0, 0.0, &shadows_lift[1]),
                    },
                    ParamDescriptor {
                        name: "shadows_lift.b",
                        value: scalar(-1.0..=1.0, 0.0, &shadows_lift[2]),
                    },
                    ParamDescriptor {
                        name: "midtones_gamma.r",
                        value: scalar(0.1..=10.0, 1.0, &midtones_gamma[0]),
                    },
                    ParamDescriptor {
                        name: "midtones_gamma.g",
                        value: scalar(0.1..=10.0, 1.0, &midtones_gamma[1]),
                    },
                    ParamDescriptor {
                        name: "midtones_gamma.b",
                        value: scalar(0.1..=10.0, 1.0, &midtones_gamma[2]),
                    },
                    ParamDescriptor {
                        name: "highlights_gain.r",
                        value: scalar(0.0..=4.0, 1.0, &highlights_gain[0]),
                    },
                    ParamDescriptor {
                        name: "highlights_gain.g",
                        value: scalar(0.0..=4.0, 1.0, &highlights_gain[1]),
                    },
                    ParamDescriptor {
                        name: "highlights_gain.b",
                        value: scalar(0.0..=4.0, 1.0, &highlights_gain[2]),
                    },
                ],
            },
            EffectKind::Curves {
                master,
                red,
                green,
                blue,
            } => EffectDescriptor {
                name: "curves",
                params: vec![
                    ParamDescriptor {
                        name: "master",
                        value: ParamValue::Points {
                            current: master.clone(),
                        },
                    },
                    ParamDescriptor {
                        name: "red",
                        value: ParamValue::Points {
                            current: red.clone(),
                        },
                    },
                    ParamDescriptor {
                        name: "green",
                        value: ParamValue::Points {
                            current: green.clone(),
                        },
                    },
                    ParamDescriptor {
                        name: "blue",
                        value: ParamValue::Points {
                            current: blue.clone(),
                        },
                    },
                ],
            },
            EffectKind::Hsl {
                hue_shift,
                saturation,
                lightness,
            } => EffectDescriptor {
                name: "hsl",
                params: vec![
                    ParamDescriptor {
                        name: "hue_shift",
                        value: scalar(-180.0..=180.0, 0.0, hue_shift),
                    },
                    ParamDescriptor {
                        name: "saturation",
                        value: scalar(0.0..=2.0, 1.0, saturation),
                    },
                    ParamDescriptor {
                        name: "lightness",
                        value: scalar(-1.0..=1.0, 0.0, lightness),
                    },
                ],
            },
            EffectKind::Lut { path } => EffectDescriptor {
                name: "lut",
                params: vec![ParamDescriptor {
                    name: "path",
                    value: ParamValue::Path {
                        current: path.clone(),
                    },
                }],
            },
            EffectKind::ChromaKey {
                key_color,
                similarity,
                softness,
            } => EffectDescriptor {
                name: "chroma_key",
                params: vec![
                    ParamDescriptor {
                        name: "key_color",
                        value: ParamValue::Color {
                            current: *key_color,
                        },
                    },
                    ParamDescriptor {
                        name: "similarity",
                        value: scalar(0.0..=1.0, 0.0, similarity),
                    },
                    ParamDescriptor {
                        name: "softness",
                        value: scalar(0.0..=1.0, 0.0, softness),
                    },
                ],
            },
            EffectKind::LumaMask { invert } => EffectDescriptor {
                name: "luma_mask",
                params: vec![ParamDescriptor {
                    name: "invert",
                    value: ParamValue::Bool {
                        default: false,
                        current: *invert,
                    },
                }],
            },
            EffectKind::ShapeMask {
                x,
                y,
                width,
                height,
                invert,
            } => EffectDescriptor {
                name: "shape_mask",
                params: vec![
                    ParamDescriptor {
                        name: "x",
                        value: scalar(0.0..=f64::INFINITY, 0.0, x),
                    },
                    ParamDescriptor {
                        name: "y",
                        value: scalar(0.0..=f64::INFINITY, 0.0, y),
                    },
                    ParamDescriptor {
                        name: "width",
                        value: scalar(0.0..=f64::INFINITY, 0.0, width),
                    },
                    ParamDescriptor {
                        name: "height",
                        value: scalar(0.0..=f64::INFINITY, 0.0, height),
                    },
                    ParamDescriptor {
                        name: "invert",
                        value: ParamValue::Bool {
                            default: false,
                            current: *invert,
                        },
                    },
                ],
            },
            EffectKind::MotionBlur {
                shutter_angle,
                sub_frames,
            } => EffectDescriptor {
                name: "motion_blur",
                params: vec![
                    ParamDescriptor {
                        name: "shutter_angle",
                        value: scalar(0.0..=360.0, 0.0, shutter_angle),
                    },
                    ParamDescriptor {
                        name: "sub_frames",
                        value: ParamValue::Int {
                            range: 2..=8,
                            default: 8,
                            current: i64::from(*sub_frames),
                        },
                    },
                ],
            },
            // A raw step is opaque: its arguments are not typed `Param`s, so there is
            // nothing for a host to render a parameter editor from.
            EffectKind::Raw { .. } => EffectDescriptor {
                name: "raw",
                params: Vec::new(),
            },
            EffectKind::Volume { gain_db } => EffectDescriptor {
                name: "volume",
                params: vec![ParamDescriptor {
                    name: "gain_db",
                    value: scalar(-60.0..=30.0, 0.0, gain_db),
                }],
            },
            EffectKind::AudioRaw { .. } => EffectDescriptor {
                name: "audio_raw",
                params: Vec::new(),
            },
        }
    }

    /// Compiles this kind to the [`FilterStep`] that renders it, or `None` when the
    /// kind is a no-op (a neutral, all-constant [`ColorCorrect`](Self::ColorCorrect),
    /// matching the historical "skip `eq` when neutral" behaviour so output stays
    /// bit-identical).
    ///
    /// An all-constant kind compiles to the static `FilterStep` variant (`Eq`,
    /// `GBlur`); any animated parameter compiles to the animated variant
    /// (`EqAnimated`, `GBlurAnimated`).
    // The static `eq` / `gblur` `FilterStep` variants carry `f32` params; a `Param`
    // holds `f64`, so the constant path narrows to `f32` (color/blur precision is
    // well within `f32` range — truncation is the intended, lossy conversion).
    #[allow(clippy::cast_possible_truncation)]
    #[must_use]
    pub fn to_filter_step(&self) -> Option<FilterStep> {
        match self {
            EffectKind::ColorCorrect {
                brightness,
                contrast,
                saturation,
                temperature,
                tint,
            } => {
                let all_const = brightness.is_const()
                    && contrast.is_const()
                    && saturation.is_const()
                    && temperature.is_const()
                    && tint.is_const();
                if all_const {
                    // Safe: `all_const` guarantees every `as_const` is `Some`.
                    let b = brightness.as_const().unwrap_or(0.0);
                    let c = contrast.as_const().unwrap_or(1.0);
                    let s = saturation.as_const().unwrap_or(1.0);
                    let temp = temperature.as_const().unwrap_or(0.0);
                    let ti = tint.as_const().unwrap_or(0.0);
                    #[allow(clippy::float_cmp)]
                    let neutral = b == 0.0 && c == 1.0 && s == 1.0 && temp == 0.0 && ti == 0.0;
                    if neutral {
                        return None;
                    }
                    Some(FilterStep::Eq {
                        brightness: b as f32,
                        contrast: c as f32,
                        saturation: s as f32,
                        temperature: temp as f32,
                        tint: ti as f32,
                    })
                } else {
                    Some(FilterStep::EqAnimated {
                        brightness: brightness.to_animated(),
                        contrast: contrast.to_animated(),
                        saturation: saturation.to_animated(),
                        gamma: AnimatedValue::Static(1.0),
                        temperature: temperature.to_animated(),
                        tint: tint.to_animated(),
                    })
                }
            }
            EffectKind::Blur { radius } => Some(match radius {
                Param::Const(v) => FilterStep::GBlur { sigma: *v as f32 },
                Param::Animated(_) => FilterStep::GBlurAnimated {
                    sigma: radius.to_animated(),
                },
            }),
            // Sharpen maps to `unsharp` (chroma left neutral): the GPU
            // `SharpenNode` is a luma-style unsharp mask. An animated amount
            // becomes `UnsharpAnimated` (which the GPU animates per frame; the CPU
            // renders its `t = 0` value, `unsharp` having no runtime parameter).
            EffectKind::Sharpen { amount } => Some(match amount {
                Param::Const(v) => FilterStep::Unsharp {
                    luma_strength: *v as f32,
                    chroma_strength: 0.0,
                },
                Param::Animated(_) => FilterStep::UnsharpAnimated {
                    luma_strength: amount.to_animated(),
                    chroma_strength: AnimatedValue::Static(0.0),
                },
            }),
            // The `vignette` filter self-animates via `eval=frame`, so both the
            // constant and animated amount route through the one `VignetteAnimated`
            // variant (centred; a `Static` renders a plain static vignette).
            EffectKind::Vignette { amount } => Some(FilterStep::VignetteAnimated {
                amount: amount.to_animated(),
                x0: 0.0,
                y0: 0.0,
            }),
            // FilmGrain maps to `noise`. An all-constant strength uses the static
            // `FilmGrain`; any animated strength uses `FilmGrainAnimated` (which the
            // GPU animates per frame; the CPU renders its `t = 0` value, `noise`
            // having no runtime parameter). The grain pattern is temporal regardless.
            EffectKind::FilmGrain {
                luma_strength,
                chroma_strength,
            } => Some(if luma_strength.is_const() && chroma_strength.is_const() {
                FilterStep::FilmGrain {
                    luma_strength: luma_strength.as_const().unwrap_or(0.0) as f32,
                    chroma_strength: chroma_strength.as_const().unwrap_or(0.0) as f32,
                }
            } else {
                FilterStep::FilmGrainAnimated {
                    luma_strength: luma_strength.to_animated(),
                    chroma_strength: chroma_strength.to_animated(),
                }
            }),
            // Glow is a compound step. An all-constant glow uses the static `Glow`;
            // any animated parameter uses `GlowAnimated` (which the GPU animates per
            // frame; the CPU renders its `t = 0` values).
            EffectKind::Glow {
                threshold,
                radius,
                intensity,
            } => Some(
                if threshold.is_const() && radius.is_const() && intensity.is_const() {
                    FilterStep::Glow {
                        threshold: threshold.as_const().unwrap_or(0.0) as f32,
                        radius: radius.as_const().unwrap_or(0.5) as f32,
                        intensity: intensity.as_const().unwrap_or(0.0) as f32,
                    }
                } else {
                    FilterStep::GlowAnimated {
                        threshold: threshold.to_animated(),
                        radius: radius.to_animated(),
                        intensity: intensity.to_animated(),
                    }
                },
            ),
            // ColorWheels maps to `curves` (via `ThreeWayCC`). The `shadows_lift`
            // (additive, neutral 0) is offset to the `lift` convention (neutral 1).
            // A fully neutral, all-constant corrector compiles to nothing.
            EffectKind::ColorWheels {
                shadows_lift,
                midtones_gamma,
                highlights_gain,
            } => {
                let all_const = shadows_lift
                    .iter()
                    .chain(midtones_gamma.iter())
                    .chain(highlights_gain.iter())
                    .all(Param::is_const);
                if all_const {
                    #[allow(clippy::float_cmp)]
                    let neutral = shadows_lift.iter().all(|p| p.as_const() == Some(0.0))
                        && midtones_gamma.iter().all(|p| p.as_const() == Some(1.0))
                        && highlights_gain.iter().all(|p| p.as_const() == Some(1.0));
                    if neutral {
                        return None;
                    }
                    let lift = Rgb {
                        r: (shadows_lift[0].as_const().unwrap_or(0.0) + 1.0) as f32,
                        g: (shadows_lift[1].as_const().unwrap_or(0.0) + 1.0) as f32,
                        b: (shadows_lift[2].as_const().unwrap_or(0.0) + 1.0) as f32,
                    };
                    let gamma = Rgb {
                        r: midtones_gamma[0].as_const().unwrap_or(1.0) as f32,
                        g: midtones_gamma[1].as_const().unwrap_or(1.0) as f32,
                        b: midtones_gamma[2].as_const().unwrap_or(1.0) as f32,
                    };
                    let gain = Rgb {
                        r: highlights_gain[0].as_const().unwrap_or(1.0) as f32,
                        g: highlights_gain[1].as_const().unwrap_or(1.0) as f32,
                        b: highlights_gain[2].as_const().unwrap_or(1.0) as f32,
                    };
                    Some(FilterStep::ThreeWayCC { lift, gamma, gain })
                } else {
                    Some(FilterStep::ThreeWayCCAnimated {
                        lift: [
                            lift_to_animated(&shadows_lift[0]),
                            lift_to_animated(&shadows_lift[1]),
                            lift_to_animated(&shadows_lift[2]),
                        ],
                        gamma: [
                            midtones_gamma[0].to_animated(),
                            midtones_gamma[1].to_animated(),
                            midtones_gamma[2].to_animated(),
                        ],
                        gain: [
                            highlights_gain[0].to_animated(),
                            highlights_gain[1].to_animated(),
                            highlights_gain[2].to_animated(),
                        ],
                    })
                }
            }
            // Curves map straight to the `curves` filter (control points as tuples);
            // an all-empty set of curves is the identity, so it compiles to nothing.
            EffectKind::Curves {
                master,
                red,
                green,
                blue,
            } => {
                if master.is_empty() && red.is_empty() && green.is_empty() && blue.is_empty() {
                    return None;
                }
                let pts = |c: &[[f32; 2]]| c.iter().map(|p| (p[0], p[1])).collect::<Vec<_>>();
                Some(FilterStep::Curves {
                    master: pts(master),
                    r: pts(red),
                    g: pts(green),
                    b: pts(blue),
                })
            }
            // Hsl maps to the `hue` filter (hue_shift -> h degrees, saturation -> s,
            // lightness -> b brightness). A fully neutral, all-constant adjustment
            // compiles to nothing; any animated parameter uses `HslAnimated` (which
            // the GPU animates per frame; the CPU renders its `t = 0` values).
            EffectKind::Hsl {
                hue_shift,
                saturation,
                lightness,
            } => {
                if hue_shift.is_const() && saturation.is_const() && lightness.is_const() {
                    #[allow(clippy::float_cmp)]
                    let neutral = hue_shift.as_const() == Some(0.0)
                        && saturation.as_const() == Some(1.0)
                        && lightness.as_const() == Some(0.0);
                    if neutral {
                        return None;
                    }
                    Some(FilterStep::Hsl {
                        hue: hue_shift.as_const().unwrap_or(0.0) as f32,
                        saturation: saturation.as_const().unwrap_or(1.0) as f32,
                        lightness: lightness.as_const().unwrap_or(0.0) as f32,
                    })
                } else {
                    Some(FilterStep::HslAnimated {
                        hue: hue_shift.to_animated(),
                        saturation: saturation.to_animated(),
                        lightness: lightness.to_animated(),
                    })
                }
            }
            // Lut maps straight to the `lut3d` filter (the same file path the GPU
            // LutNode loads); an empty path is the identity, so it compiles to nothing.
            EffectKind::Lut { path } => {
                if path.is_empty() {
                    return None;
                }
                Some(FilterStep::Lut3d { path: path.clone() })
            }
            // ChromaKey maps to the `chromakey` filter (key colour + similarity +
            // blend). A constant `similarity = 0.0` removes nothing, so it compiles
            // to nothing; any animated parameter uses `ChromaKeyAnimated` (the GPU
            // animates per frame; the CPU renders its `t = 0` values).
            EffectKind::ChromaKey {
                key_color,
                similarity,
                softness,
            } => {
                if similarity.is_const() && softness.is_const() {
                    #[allow(clippy::float_cmp)]
                    if similarity.as_const() == Some(0.0) {
                        return None;
                    }
                    Some(FilterStep::ChromaKey {
                        color: rgb_to_ffmpeg_hex(*key_color),
                        similarity: similarity.as_const().unwrap_or(0.0) as f32,
                        blend: softness.as_const().unwrap_or(0.0) as f32,
                    })
                } else {
                    Some(FilterStep::ChromaKeyAnimated {
                        color: rgb_to_ffmpeg_hex(*key_color),
                        similarity: similarity.to_animated(),
                        blend: softness.to_animated(),
                    })
                }
            }
            // LumaMask is structural (no scalar param): it always maps to the `geq`
            // self-luma mask. `invert` carries straight through.
            EffectKind::LumaMask { invert } => Some(FilterStep::LumaMask { invert: *invert }),
            // ShapeMask maps to the rectangular `geq` mask. All-const bounds compile to
            // `RectMask` (a constant zero width/height masks nothing, so it is a no-op);
            // any animated bound uses `RectMaskAnimated` (the GPU animates per frame;
            // the CPU renders its `t = 0` bounds).
            EffectKind::ShapeMask {
                x,
                y,
                width,
                height,
                invert,
            } => {
                if x.is_const() && y.is_const() && width.is_const() && height.is_const() {
                    let (w, h) = (param_to_u32(width), param_to_u32(height));
                    if w == 0 || h == 0 {
                        return None;
                    }
                    Some(FilterStep::RectMask {
                        x: param_to_u32(x),
                        y: param_to_u32(y),
                        width: w,
                        height: h,
                        invert: *invert,
                    })
                } else {
                    Some(FilterStep::RectMaskAnimated {
                        x: x.to_animated(),
                        y: y.to_animated(),
                        width: width.to_animated(),
                        height: height.to_animated(),
                        invert: *invert,
                    })
                }
            }
            // MotionBlur maps to `tblend`. Motion blur is stateful (the trail
            // accumulates across frames on one node), so the shutter cannot animate
            // per frame: both paths use the value at `t = 0`. A non-positive shutter
            // is no blur, so it compiles to nothing — matching the GPU classifier's
            // `<= 0.0` Skip so both paths treat an out-of-range value the same way.
            EffectKind::MotionBlur {
                shutter_angle,
                sub_frames,
            } => {
                let angle = shutter_angle.to_animated().value_at(Duration::ZERO) as f32;
                if angle <= 0.0 {
                    return None;
                }
                Some(FilterStep::MotionBlur {
                    shutter_angle_degrees: angle,
                    sub_frames: *sub_frames,
                })
            }
            // The escape hatches render their step verbatim.
            EffectKind::Raw { step } | EffectKind::AudioRaw { step } => Some(step.clone()),
            // A neutral gain is no change, so it compiles to nothing. `volume` has no
            // runtime-settable parameter here, so an animated gain renders at `t = 0`.
            EffectKind::Volume { gain_db } => {
                let db = gain_db.to_animated().value_at(Duration::ZERO);
                #[allow(clippy::float_cmp)]
                if db == 0.0 {
                    return None;
                }
                Some(FilterStep::Volume(db))
            }
        }
    }
}

/// Rounds a constant [`Param`] to a non-negative pixel count for the `RectMask`
/// filter (`u32`). An animated parameter has no constant value; callers only use this
/// on the all-const path, so it clamps to `0`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn param_to_u32(p: &Param) -> u32 {
    p.as_const().unwrap_or(0.0).max(0.0).round() as u32
}

/// Formats an RGB triple (each channel `0.0..=1.0`) as an `FFmpeg` `0xRRGGBB`
/// colour string, the canonical form [`avio::gpu`](crate::gpu) parses back to the
/// GPU node's key colour.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rgb_to_ffmpeg_hex(rgb: [f32; 3]) -> String {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    format!("0x{:02X}{:02X}{:02X}", ch(rgb[0]), ch(rgb[1]), ch(rgb[2]))
}

/// One typed effect in a [`Clip`](crate::Clip)'s ordered effect list.
///
/// Addressed by its document-scoped [`EffectId`]; toggled by `enabled` (a disabled
/// effect is skipped during derivation but kept in the list, so re-enabling restores
/// its position and parameters).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClipEffect {
    /// Stable identity within the document. [`EffectId::UNSET`] until added.
    pub id: EffectId,
    /// Whether this effect participates in derivation (`false` = bypassed, kept).
    pub enabled: bool,
    /// The typed effect and its parameters.
    pub kind: EffectKind,
}

impl ClipEffect {
    /// A new enabled effect with an [`UNSET`](EffectId::UNSET) id (the document
    /// stamps a real id when it is added).
    #[must_use]
    pub fn new(kind: EffectKind) -> Self {
        ClipEffect {
            id: EffectId::UNSET,
            enabled: true,
            kind,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use ff_filter::{Easing, Keyframe};

    use super::*;

    fn animated_track() -> AnimationTrack<f64> {
        AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear))
    }

    /// The minimal payload for each `EffectKind` variant: `{}` for a variant whose
    /// fields all have a neutral serde default, and the one required field for a
    /// variant that has one (`Raw` / `AudioRaw` carry a whole `FilterStep`, which has
    /// no neutral).
    ///
    /// The `match` is exhaustive with **no** `_` arm: a new variant fails to compile
    /// until it declares its minimal payload here, which is what keeps
    /// `MINIMAL_PAYLOADS` (and therefore the #1709 serde-default convention) applied to
    /// every variant. Same technique as `domain` / `descriptor` and the parity
    /// coverage meta-test (RK-003).
    #[cfg(feature = "serde")]
    fn minimal_payload_for(kind: &EffectKind) -> &'static str {
        match kind {
            EffectKind::ColorCorrect { .. } => r#"{"ColorCorrect":{}}"#,
            EffectKind::Blur { .. } => r#"{"Blur":{}}"#,
            EffectKind::Sharpen { .. } => r#"{"Sharpen":{}}"#,
            EffectKind::Vignette { .. } => r#"{"Vignette":{}}"#,
            EffectKind::FilmGrain { .. } => r#"{"FilmGrain":{}}"#,
            EffectKind::Glow { .. } => r#"{"Glow":{}}"#,
            EffectKind::ColorWheels { .. } => r#"{"ColorWheels":{}}"#,
            EffectKind::Curves { .. } => r#"{"Curves":{}}"#,
            EffectKind::Hsl { .. } => r#"{"Hsl":{}}"#,
            EffectKind::Lut { .. } => r#"{"Lut":{}}"#,
            EffectKind::ChromaKey { .. } => r#"{"ChromaKey":{}}"#,
            EffectKind::LumaMask { .. } => r#"{"LumaMask":{}}"#,
            EffectKind::ShapeMask { .. } => r#"{"ShapeMask":{}}"#,
            EffectKind::MotionBlur { .. } => r#"{"MotionBlur":{}}"#,
            EffectKind::Volume { .. } => r#"{"Volume":{}}"#,
            EffectKind::Raw { .. } => r#"{"Raw":{"step":"HFlip"}}"#,
            EffectKind::AudioRaw { .. } => r#"{"AudioRaw":{"step":"HFlip"}}"#,
        }
    }

    /// The minimal payload of every `EffectKind` variant, kept in sync with the enum by
    /// [`minimal_payload_for`]'s exhaustive match.
    #[cfg(feature = "serde")]
    const MINIMAL_PAYLOADS: &[&str] = &[
        r#"{"ColorCorrect":{}}"#,
        r#"{"Blur":{}}"#,
        r#"{"Sharpen":{}}"#,
        r#"{"Vignette":{}}"#,
        r#"{"FilmGrain":{}}"#,
        r#"{"Glow":{}}"#,
        r#"{"ColorWheels":{}}"#,
        r#"{"Curves":{}}"#,
        r#"{"Hsl":{}}"#,
        r#"{"Lut":{}}"#,
        r#"{"ChromaKey":{}}"#,
        r#"{"LumaMask":{}}"#,
        r#"{"ShapeMask":{}}"#,
        r#"{"MotionBlur":{}}"#,
        r#"{"Volume":{}}"#,
        r#"{"Raw":{"step":"HFlip"}}"#,
        r#"{"AudioRaw":{"step":"HFlip"}}"#,
    ];

    /// Guards the pair above: every payload in `MINIMAL_PAYLOADS` must deserialize to a
    /// variant that maps back to that same payload, so the list cannot drift from the
    /// exhaustive match (and a newly added variant, forced into the match, is caught
    /// here if it was not added to the list).
    #[cfg(feature = "serde")]
    #[test]
    fn minimal_payloads_should_cover_every_effect_kind_variant() {
        let mut seen = std::collections::BTreeSet::new();
        for json in MINIMAL_PAYLOADS {
            let kind: EffectKind =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{json} must load: {e}"));
            let expected = minimal_payload_for(&kind);
            assert_eq!(
                *json, expected,
                "{json} must be the payload declared for its variant"
            );
            assert!(seen.insert(expected), "{json} listed twice");
        }
        // 17 variants today; the exhaustive match makes adding one fail to compile, and
        // this count makes forgetting to list it fail here.
        assert_eq!(seen.len(), 17, "every EffectKind variant must be listed");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserializing_omitted_effect_fields_should_yield_descriptor_defaults() {
        // #1709: a document written before a field existed must still load, with the
        // missing field at its neutral — and that neutral must be the same one
        // `descriptor()` reports, so a host's "reset to default" and an old document
        // agree. Deserializing the *minimal* payload exercises every field at once: a
        // field without a serde default would fail to deserialize here, so this also
        // proves the convention is applied to every variant's fields.
        for json in MINIMAL_PAYLOADS {
            let kind: EffectKind =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{json} must load: {e}"));
            for p in kind.descriptor().params {
                let name = p.name;
                match p.value {
                    ParamValue::Scalar {
                        default, current, ..
                    } => assert_eq!(
                        current,
                        Some(default),
                        "{json} / {name}: serde default must equal the descriptor default"
                    ),
                    ParamValue::Bool { default, current } => {
                        assert_eq!(current, default, "{json} / {name}");
                    }
                    ParamValue::Int {
                        default, current, ..
                    } => assert_eq!(current, default, "{json} / {name}"),
                    // `descriptor()` reports no default for these, so the serde default
                    // is simply the empty / neutral value.
                    ParamValue::Path { current } => {
                        assert!(current.is_empty(), "{json} / {name}");
                    }
                    ParamValue::Points { current } => {
                        assert!(current.is_empty(), "{json} / {name}");
                    }
                    ParamValue::Color { current } => {
                        assert_eq!(current, [0.0, 1.0, 0.0], "{json} / {name}");
                    }
                }
            }
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserializing_a_pre_temperature_color_correct_should_load() {
        // The concrete #1658 regression: a ColorCorrect written before temperature/tint
        // existed still loads, with those two at their neutral.
        let json = r#"{"ColorCorrect":{"brightness":{"Const":0.2},
            "contrast":{"Const":1.1},"saturation":{"Const":0.9}}}"#;
        let kind: EffectKind = serde_json::from_str(json).expect("an older payload must load");
        let EffectKind::ColorCorrect {
            brightness,
            temperature,
            tint,
            ..
        } = &kind
        else {
            panic!("expected a ColorCorrect");
        };
        assert_eq!(
            brightness.as_const(),
            Some(0.2),
            "stored value is preserved"
        );
        assert_eq!(temperature.as_const(), Some(0.0), "neutral temperature");
        assert_eq!(tint.as_const(), Some(0.0), "neutral tint");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn effect_kind_should_round_trip_through_serde_with_neutral_defaults() {
        // A document written by this version and read back is unchanged: the neutral
        // defaults survive the round-trip rather than shifting on each save/load.
        for json in MINIMAL_PAYLOADS {
            let kind: EffectKind = serde_json::from_str(json).unwrap();
            let written = serde_json::to_string(&kind).unwrap();
            let back: EffectKind = serde_json::from_str(&written).unwrap();
            assert_eq!(
                format!("{:?}", kind.descriptor()),
                format!("{:?}", back.descriptor()),
                "{json} must round-trip unchanged"
            );
        }
    }

    #[test]
    fn domain_should_classify_video_and_audio_kinds() {
        // #1712: one list holds both domains, so the kind must declare which pipeline
        // it belongs to.
        let video = EffectKind::Blur {
            radius: Param::Const(2.0),
        };
        assert_eq!(video.domain(), EffectDomain::Video);
        assert_eq!(
            EffectKind::Raw {
                step: FilterStep::HFlip
            }
            .domain(),
            EffectDomain::Video
        );
        let audio = EffectKind::Volume {
            gain_db: Param::Const(-6.0),
        };
        assert_eq!(audio.domain(), EffectDomain::Audio);
        assert_eq!(
            EffectKind::AudioRaw {
                step: FilterStep::Volume(-3.0)
            }
            .domain(),
            EffectDomain::Audio
        );
    }

    #[test]
    fn volume_should_compile_to_volume_step() {
        let kind = EffectKind::Volume {
            gain_db: Param::Const(-6.0),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Volume(db) => assert!((db - (-6.0)).abs() < 1e-6),
            other => panic!("expected Volume, got {other:?}"),
        }
    }

    #[test]
    fn volume_neutral_should_compile_to_nothing() {
        let kind = EffectKind::Volume {
            gain_db: Param::Const(0.0),
        };
        assert!(
            kind.to_filter_step().is_none(),
            "0 dB is no change, so it is a no-op"
        );
    }

    #[test]
    fn audio_raw_should_compile_to_its_filter_step() {
        let kind = EffectKind::AudioRaw {
            step: FilterStep::Volume(-3.0),
        };
        assert!(matches!(
            kind.to_filter_step().unwrap(),
            FilterStep::Volume(_)
        ));
    }

    #[test]
    fn descriptor_should_list_volume_param() {
        let kind = EffectKind::Volume {
            gain_db: Param::Const(-6.0),
        };
        let d = kind.descriptor();
        assert_eq!(d.name, "volume");
        assert_eq!(d.params.len(), 1);
        assert_eq!(d.params[0].name, "gain_db");
        assert_eq!(
            d.params[0].value,
            ParamValue::Scalar {
                range: -60.0..=30.0,
                default: 0.0,
                current: Some(-6.0),
            }
        );
        // The audio escape hatch is opaque, like the video one.
        let raw = EffectKind::AudioRaw {
            step: FilterStep::Volume(-3.0),
        };
        assert_eq!(raw.descriptor().name, "audio_raw");
        assert!(raw.descriptor().params.is_empty());
    }

    #[test]
    fn raw_should_compile_to_its_filter_step() {
        // #1622: the escape hatch renders its step verbatim.
        let kind = EffectKind::Raw {
            step: FilterStep::Hue { degrees: 30.0 },
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Hue { degrees } => assert!((degrees - 30.0).abs() < 1e-6),
            other => panic!("expected Hue, got {other:?}"),
        }
    }

    #[test]
    fn descriptor_should_describe_raw_as_opaque() {
        // A raw step has no typed parameters, so introspection reports none.
        let kind = EffectKind::Raw {
            step: FilterStep::HFlip,
        };
        let d = kind.descriptor();
        assert_eq!(d.name, "raw");
        assert!(
            d.params.is_empty(),
            "a raw step exposes no typed parameters"
        );
    }

    #[test]
    fn descriptor_should_list_color_correct_params() {
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Const(0.2),
            contrast: Param::Const(1.1),
            saturation: Param::Const(0.9),
            temperature: Param::Const(-0.3),
            tint: Param::Const(0.4),
        };
        let d = kind.descriptor();
        assert_eq!(d.name, "color_correct");
        let names: Vec<&str> = d.params.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "brightness",
                "contrast",
                "saturation",
                "temperature",
                "tint"
            ]
        );
        assert_eq!(
            d.params[0].value,
            ParamValue::Scalar {
                range: -1.0..=1.0,
                default: 0.0,
                current: Some(0.2),
            }
        );
        assert_eq!(
            d.params[1].value,
            ParamValue::Scalar {
                range: 0.0..=3.0,
                default: 1.0,
                current: Some(1.1),
            }
        );
        assert_eq!(
            d.params[3].value,
            ParamValue::Scalar {
                range: -1.0..=1.0,
                default: 0.0,
                current: Some(-0.3),
            }
        );
    }

    #[test]
    fn descriptor_should_report_animated_scalar_as_none() {
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Animated(animated_track()),
            contrast: Param::Const(1.0),
            saturation: Param::Const(1.0),
            temperature: Param::Const(0.0),
            tint: Param::Const(0.0),
        };
        let d = kind.descriptor();
        // An animated scalar reports `current: None` (the host knows it is keyframed).
        match &d.params[0].value {
            ParamValue::Scalar { current, .. } => assert_eq!(*current, None),
            other => panic!("expected a Scalar, got {other:?}"),
        }
    }

    #[test]
    fn descriptor_should_cover_structural_param_kinds() {
        // MotionBlur.sub_frames -> Int
        let mb = EffectKind::MotionBlur {
            shutter_angle: Param::Const(180.0),
            sub_frames: 6,
        };
        assert_eq!(
            mb.descriptor().params[1].value,
            ParamValue::Int {
                range: 2..=8,
                default: 8,
                current: 6,
            }
        );
        // LumaMask.invert -> Bool
        let lm = EffectKind::LumaMask { invert: true };
        assert_eq!(
            lm.descriptor().params[0].value,
            ParamValue::Bool {
                default: false,
                current: true,
            }
        );
        // ChromaKey.key_color -> Color
        let ck = EffectKind::ChromaKey {
            key_color: [0.0, 1.0, 0.0],
            similarity: Param::Const(0.1),
            softness: Param::Const(0.0),
        };
        assert_eq!(
            ck.descriptor().params[0].value,
            ParamValue::Color {
                current: [0.0, 1.0, 0.0],
            }
        );
        // Lut.path -> Path
        let lut = EffectKind::Lut {
            path: "look.cube".to_string(),
        };
        assert_eq!(
            lut.descriptor().params[0].value,
            ParamValue::Path {
                current: "look.cube".to_string(),
            }
        );
        // Curves.master -> Points
        let curves = EffectKind::Curves {
            master: vec![[0.0, 0.0], [1.0, 1.0]],
            red: vec![],
            green: vec![],
            blue: vec![],
        };
        assert_eq!(
            curves.descriptor().params[0].value,
            ParamValue::Points {
                current: vec![[0.0, 0.0], [1.0, 1.0]],
            }
        );
    }

    #[test]
    fn descriptor_should_flatten_color_wheels_channels() {
        let kind = EffectKind::ColorWheels {
            shadows_lift: [Param::Const(0.1), Param::Const(0.2), Param::Const(0.3)],
            midtones_gamma: [Param::Const(1.0), Param::Const(1.0), Param::Const(1.0)],
            highlights_gain: [Param::Const(1.0), Param::Const(1.0), Param::Const(1.0)],
        };
        let d = kind.descriptor();
        assert_eq!(d.name, "color_wheels");
        let names: Vec<&str> = d.params.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "shadows_lift.r",
                "shadows_lift.g",
                "shadows_lift.b",
                "midtones_gamma.r",
                "midtones_gamma.g",
                "midtones_gamma.b",
                "highlights_gain.r",
                "highlights_gain.g",
                "highlights_gain.b",
            ]
        );
        assert_eq!(
            d.params[2].value,
            ParamValue::Scalar {
                range: -1.0..=1.0,
                default: 0.0,
                current: Some(0.3),
            }
        );
    }

    #[test]
    fn color_correct_neutral_const_should_compile_to_nothing() {
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Const(0.0),
            contrast: Param::Const(1.0),
            saturation: Param::Const(1.0),
            temperature: Param::Const(0.0),
            tint: Param::Const(0.0),
        };
        assert!(
            kind.to_filter_step().is_none(),
            "a neutral, all-constant ColorCorrect is a no-op (bit-identical to no eq)"
        );
    }

    #[test]
    fn color_correct_non_neutral_const_should_compile_to_eq() {
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Const(0.5),
            contrast: Param::Const(1.2),
            saturation: Param::Const(0.8),
            temperature: Param::Const(0.3),
            tint: Param::Const(-0.4),
        };
        let step = kind.to_filter_step().unwrap();
        match step {
            FilterStep::Eq {
                brightness,
                contrast,
                saturation,
                temperature,
                tint,
            } => {
                assert!((brightness - 0.5).abs() < 1e-6);
                assert!((contrast - 1.2).abs() < 1e-6);
                assert!((saturation - 0.8).abs() < 1e-6);
                assert!((temperature - 0.3).abs() < 1e-6);
                assert!((tint - (-0.4)).abs() < 1e-6);
            }
            other => panic!("expected Eq, got {other:?}"),
        }
    }

    #[test]
    fn color_correct_temperature_only_should_compile_to_eq() {
        // Only temperature/tint are non-neutral (brightness/contrast/saturation neutral):
        // it must still compile to an Eq step (not skipped as a no-op) so the GPU grade
        // carries them.
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Const(0.0),
            contrast: Param::Const(1.0),
            saturation: Param::Const(1.0),
            temperature: Param::Const(0.6),
            tint: Param::Const(0.0),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Eq {
                temperature, tint, ..
            } => {
                assert!((temperature - 0.6).abs() < 1e-6);
                assert!(tint.abs() < 1e-6);
            }
            other => panic!("expected Eq, got {other:?}"),
        }
    }

    #[test]
    fn color_correct_animated_should_compile_to_eq_animated() {
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Animated(animated_track()),
            contrast: Param::Const(1.0),
            saturation: Param::Const(1.0),
            temperature: Param::Const(0.0),
            tint: Param::Const(0.0),
        };
        assert!(matches!(
            kind.to_filter_step(),
            Some(FilterStep::EqAnimated { .. })
        ));
    }

    #[test]
    fn color_correct_animated_temperature_should_compile_to_eq_animated() {
        // An animated temperature alone routes through the animated variant.
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Const(0.0),
            contrast: Param::Const(1.0),
            saturation: Param::Const(1.0),
            temperature: Param::Animated(animated_track()),
            tint: Param::Const(0.0),
        };
        assert!(matches!(
            kind.to_filter_step(),
            Some(FilterStep::EqAnimated { .. })
        ));
    }

    #[test]
    fn blur_const_should_compile_to_gblur() {
        let kind = EffectKind::Blur {
            radius: Param::Const(4.0),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::GBlur { sigma } => assert!((sigma - 4.0).abs() < 1e-6),
            other => panic!("expected GBlur, got {other:?}"),
        }
    }

    #[test]
    fn blur_animated_should_compile_to_gblur_animated() {
        let kind = EffectKind::Blur {
            radius: Param::Animated(animated_track()),
        };
        assert!(matches!(
            kind.to_filter_step(),
            Some(FilterStep::GBlurAnimated { .. })
        ));
    }

    #[test]
    fn sharpen_const_should_compile_to_unsharp_with_neutral_chroma() {
        let kind = EffectKind::Sharpen {
            amount: Param::Const(1.0),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Unsharp {
                luma_strength,
                chroma_strength,
            } => {
                assert!((luma_strength - 1.0).abs() < 1e-6);
                assert!((chroma_strength - 0.0).abs() < 1e-6, "chroma stays neutral");
            }
            other => panic!("expected Unsharp, got {other:?}"),
        }
    }

    #[test]
    fn sharpen_animated_should_compile_to_unsharp_animated() {
        let kind = EffectKind::Sharpen {
            amount: Param::Animated(animated_track()),
        };
        assert!(matches!(
            kind.to_filter_step(),
            Some(FilterStep::UnsharpAnimated { .. })
        ));
    }

    #[test]
    fn vignette_const_should_compile_to_centred_vignette_animated() {
        let kind = EffectKind::Vignette {
            amount: Param::Const(0.6),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::VignetteAnimated { amount, x0, y0 } => {
                assert!((amount.value_at(std::time::Duration::ZERO) - 0.6).abs() < 1e-6);
                assert!(
                    (x0 - 0.0).abs() < 1e-6 && (y0 - 0.0).abs() < 1e-6,
                    "centred"
                );
            }
            other => panic!("expected VignetteAnimated, got {other:?}"),
        }
    }

    #[test]
    fn vignette_animated_should_compile_to_vignette_animated() {
        let kind = EffectKind::Vignette {
            amount: Param::Animated(animated_track()),
        };
        assert!(matches!(
            kind.to_filter_step(),
            Some(FilterStep::VignetteAnimated { .. })
        ));
    }

    #[test]
    fn film_grain_const_should_compile_to_film_grain() {
        let kind = EffectKind::FilmGrain {
            luma_strength: Param::Const(20.0),
            chroma_strength: Param::Const(5.0),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::FilmGrain {
                luma_strength,
                chroma_strength,
            } => {
                assert!((luma_strength - 20.0).abs() < 1e-6);
                assert!((chroma_strength - 5.0).abs() < 1e-6);
            }
            other => panic!("expected FilmGrain, got {other:?}"),
        }
    }

    #[test]
    fn film_grain_any_animated_strength_should_compile_to_film_grain_animated() {
        let kind = EffectKind::FilmGrain {
            luma_strength: Param::Animated(animated_track()),
            chroma_strength: Param::Const(5.0),
        };
        assert!(matches!(
            kind.to_filter_step(),
            Some(FilterStep::FilmGrainAnimated { .. })
        ));
    }

    #[test]
    fn glow_const_should_compile_to_glow() {
        let kind = EffectKind::Glow {
            threshold: Param::Const(0.8),
            radius: Param::Const(10.0),
            intensity: Param::Const(0.8),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Glow {
                threshold,
                radius,
                intensity,
            } => {
                assert!((threshold - 0.8).abs() < 1e-6);
                assert!((radius - 10.0).abs() < 1e-6);
                assert!((intensity - 0.8).abs() < 1e-6);
            }
            other => panic!("expected Glow, got {other:?}"),
        }
    }

    #[test]
    fn glow_any_animated_param_should_compile_to_glow_animated() {
        let kind = EffectKind::Glow {
            threshold: Param::Const(0.8),
            radius: Param::Animated(animated_track()),
            intensity: Param::Const(0.8),
        };
        assert!(matches!(
            kind.to_filter_step(),
            Some(FilterStep::GlowAnimated { .. })
        ));
    }

    fn cw(lift: f64, gamma: f64, gain: f64) -> EffectKind {
        EffectKind::ColorWheels {
            shadows_lift: [Param::Const(lift), Param::Const(lift), Param::Const(lift)],
            midtones_gamma: [
                Param::Const(gamma),
                Param::Const(gamma),
                Param::Const(gamma),
            ],
            highlights_gain: [Param::Const(gain), Param::Const(gain), Param::Const(gain)],
        }
    }

    #[test]
    fn color_wheels_neutral_should_compile_to_nothing() {
        assert!(
            cw(0.0, 1.0, 1.0).to_filter_step().is_none(),
            "a neutral ColorWheels is a no-op"
        );
    }

    #[test]
    fn color_wheels_const_should_offset_lift_by_one() {
        // shadows_lift 0.1 (additive) maps to the ThreeWayCC lift 1.1 (neutral 1.0).
        match cw(0.1, 1.0, 1.0).to_filter_step().unwrap() {
            FilterStep::ThreeWayCC { lift, gamma, gain } => {
                assert!((lift.r - 1.1).abs() < 1e-5, "lift = 1 + shadows_lift");
                assert!((gamma.r - 1.0).abs() < 1e-6 && (gain.r - 1.0).abs() < 1e-6);
            }
            other => panic!("expected ThreeWayCC, got {other:?}"),
        }
    }

    #[test]
    fn color_wheels_any_animated_should_compile_to_three_way_cc_animated() {
        let kind = EffectKind::ColorWheels {
            shadows_lift: [
                Param::Animated(animated_track()),
                Param::Const(0.0),
                Param::Const(0.0),
            ],
            midtones_gamma: [Param::Const(1.0), Param::Const(1.0), Param::Const(1.0)],
            highlights_gain: [Param::Const(1.0), Param::Const(1.0), Param::Const(1.0)],
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::ThreeWayCCAnimated { lift, .. } => {
                // The animated lift track (values ~0.5) is offset by +1 (~1.5).
                assert!(
                    lift[0].value_at(std::time::Duration::ZERO) > 1.0,
                    "the animated lift track is offset to the neutral-1.0 convention"
                );
            }
            other => panic!("expected ThreeWayCCAnimated, got {other:?}"),
        }
    }

    #[test]
    fn curves_empty_should_compile_to_nothing() {
        let kind = EffectKind::Curves {
            master: vec![],
            red: vec![],
            green: vec![],
            blue: vec![],
        };
        assert!(
            kind.to_filter_step().is_none(),
            "an all-empty Curves is the identity (no-op)"
        );
    }

    #[test]
    fn curves_should_compile_to_curves_with_tuple_points() {
        let kind = EffectKind::Curves {
            master: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            red: vec![],
            green: vec![],
            blue: vec![],
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Curves { master, r, .. } => {
                assert_eq!(master, vec![(0.0, 0.0), (0.5, 0.7), (1.0, 1.0)]);
                assert!(r.is_empty(), "an empty per-channel curve stays empty");
            }
            other => panic!("expected Curves, got {other:?}"),
        }
    }

    #[test]
    fn hsl_neutral_should_compile_to_nothing() {
        let kind = EffectKind::Hsl {
            hue_shift: Param::Const(0.0),
            saturation: Param::Const(1.0),
            lightness: Param::Const(0.0),
        };
        assert!(
            kind.to_filter_step().is_none(),
            "a neutral HSL adjustment is a no-op"
        );
    }

    #[test]
    fn hsl_const_should_compile_to_hsl() {
        let kind = EffectKind::Hsl {
            hue_shift: Param::Const(20.0),
            saturation: Param::Const(1.2),
            lightness: Param::Const(0.05),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Hsl {
                hue,
                saturation,
                lightness,
            } => {
                assert!((hue - 20.0).abs() < 1e-5);
                assert!((saturation - 1.2).abs() < 1e-5);
                assert!((lightness - 0.05).abs() < 1e-5);
            }
            other => panic!("expected Hsl, got {other:?}"),
        }
    }

    #[test]
    fn hsl_any_animated_should_compile_to_hsl_animated() {
        let kind = EffectKind::Hsl {
            hue_shift: Param::Animated(animated_track()),
            saturation: Param::Const(1.0),
            lightness: Param::Const(0.0),
        };
        assert!(
            matches!(
                kind.to_filter_step().unwrap(),
                FilterStep::HslAnimated { .. }
            ),
            "any animated parameter routes through HslAnimated"
        );
    }

    #[test]
    fn lut_empty_path_should_compile_to_nothing() {
        let kind = EffectKind::Lut {
            path: String::new(),
        };
        assert!(
            kind.to_filter_step().is_none(),
            "an empty LUT path is the identity (no-op)"
        );
    }

    #[test]
    fn lut_should_compile_to_lut3d() {
        let kind = EffectKind::Lut {
            path: "grade.cube".to_string(),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::Lut3d { path } => assert_eq!(path, "grade.cube"),
            other => panic!("expected Lut3d, got {other:?}"),
        }
    }

    #[test]
    fn chroma_key_zero_similarity_should_compile_to_nothing() {
        let kind = EffectKind::ChromaKey {
            key_color: [0.0, 1.0, 0.0],
            similarity: Param::Const(0.0),
            softness: Param::Const(0.1),
        };
        assert!(
            kind.to_filter_step().is_none(),
            "similarity 0 removes nothing, so it is a no-op"
        );
    }

    #[test]
    fn chroma_key_const_should_compile_to_chroma_key_with_hex_colour() {
        let kind = EffectKind::ChromaKey {
            key_color: [0.0, 1.0, 0.0],
            similarity: Param::Const(0.3),
            softness: Param::Const(0.1),
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::ChromaKey {
                color,
                similarity,
                blend,
            } => {
                assert_eq!(color, "0x00FF00");
                assert!((similarity - 0.3).abs() < 1e-5);
                assert!((blend - 0.1).abs() < 1e-5);
            }
            other => panic!("expected ChromaKey, got {other:?}"),
        }
    }

    #[test]
    fn chroma_key_any_animated_should_compile_to_chroma_key_animated() {
        let kind = EffectKind::ChromaKey {
            key_color: [0.0, 1.0, 0.0],
            similarity: Param::Animated(animated_track()),
            softness: Param::Const(0.1),
        };
        assert!(
            matches!(
                kind.to_filter_step().unwrap(),
                FilterStep::ChromaKeyAnimated { .. }
            ),
            "any animated parameter routes through ChromaKeyAnimated"
        );
    }

    #[test]
    fn luma_mask_should_compile_to_luma_mask_filter_step() {
        for invert in [false, true] {
            let kind = EffectKind::LumaMask { invert };
            match kind.to_filter_step() {
                Some(FilterStep::LumaMask { invert: got }) => assert_eq!(got, invert),
                other => panic!("expected LumaMask {{ invert: {invert} }}, got {other:?}"),
            }
        }
    }

    #[test]
    fn shape_mask_const_should_compile_to_rect_mask_with_rounded_bounds() {
        let kind = EffectKind::ShapeMask {
            x: Param::Const(10.4),
            y: Param::Const(20.6),
            width: Param::Const(30.0),
            height: Param::Const(40.0),
            invert: true,
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::RectMask {
                x,
                y,
                width,
                height,
                invert,
            } => {
                assert_eq!((x, y, width, height), (10, 21, 30, 40));
                assert!(invert);
            }
            other => panic!("expected RectMask, got {other:?}"),
        }
    }

    #[test]
    fn shape_mask_zero_size_should_compile_to_nothing() {
        let kind = EffectKind::ShapeMask {
            x: Param::Const(0.0),
            y: Param::Const(0.0),
            width: Param::Const(0.0),
            height: Param::Const(10.0),
            invert: false,
        };
        assert!(
            kind.to_filter_step().is_none(),
            "a zero-width rectangle masks nothing, so it is a no-op"
        );
    }

    #[test]
    fn shape_mask_any_animated_should_compile_to_rect_mask_animated() {
        let kind = EffectKind::ShapeMask {
            x: Param::Animated(animated_track()),
            y: Param::Const(0.0),
            width: Param::Const(30.0),
            height: Param::Const(40.0),
            invert: false,
        };
        assert!(
            matches!(
                kind.to_filter_step().unwrap(),
                FilterStep::RectMaskAnimated { .. }
            ),
            "any animated bound routes through RectMaskAnimated"
        );
    }

    #[test]
    fn motion_blur_const_should_map_to_motion_blur_step() {
        let kind = EffectKind::MotionBlur {
            shutter_angle: Param::Const(180.0),
            sub_frames: 4,
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::MotionBlur {
                shutter_angle_degrees,
                sub_frames,
            } => {
                assert!((shutter_angle_degrees - 180.0).abs() < 1e-4);
                assert_eq!(sub_frames, 4);
            }
            other => panic!("expected MotionBlur, got {other:?}"),
        }
    }

    #[test]
    fn motion_blur_non_positive_shutter_should_map_to_none() {
        // A zero (and any out-of-range negative) shutter is no blur, so it compiles to
        // nothing on both paths (the GPU classifier likewise skips `<= 0.0`).
        for angle in [0.0, -10.0] {
            let kind = EffectKind::MotionBlur {
                shutter_angle: Param::Const(angle),
                sub_frames: 4,
            };
            assert!(
                kind.to_filter_step().is_none(),
                "a non-positive shutter angle ({angle}) is no blur, so it is a no-op"
            );
        }
    }

    #[test]
    fn motion_blur_animated_shutter_should_render_t0_value() {
        // The trail needs a stable node, so an animated shutter renders its t=0 value
        // (here 180) rather than a later keyframe (90).
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 180.0, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(1), 90.0, Easing::Linear));
        let kind = EffectKind::MotionBlur {
            shutter_angle: Param::Animated(track),
            sub_frames: 6,
        };
        match kind.to_filter_step().unwrap() {
            FilterStep::MotionBlur {
                shutter_angle_degrees,
                sub_frames,
            } => {
                assert!(
                    (shutter_angle_degrees - 180.0).abs() < 1e-4,
                    "must use the t=0 shutter value (180), got {shutter_angle_degrees}"
                );
                assert_eq!(sub_frames, 6);
            }
            other => panic!("expected MotionBlur, got {other:?}"),
        }
    }
}
