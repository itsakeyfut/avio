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

/// A typed effect that a clip can carry. `#[non_exhaustive]`: more kinds are added
/// over time, so external matchers must include a `_` arm.
///
/// The set grows as effect nodes are wired into the GPU bridge; each kind documents
/// the `ff-filter` step / `ff-render` node it maps to.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum EffectKind {
    /// Brightness / contrast / saturation adjustment (the `eq` filter).
    ///
    /// Neutral parameters (`brightness = 0.0`, `contrast = 1.0`, `saturation = 1.0`,
    /// all constant) compile to no filter at all, preserving bit-identical output.
    ColorCorrect {
        /// Brightness offset. Range −1.0..=1.0 (neutral: 0.0).
        brightness: Param,
        /// Contrast multiplier. Range 0.0..=3.0 (neutral: 1.0).
        contrast: Param,
        /// Saturation multiplier. Range 0.0..=3.0 (neutral: 1.0).
        saturation: Param,
    },
    /// Gaussian blur (the `gblur` filter).
    Blur {
        /// Blur radius (standard deviation). Must be ≥ 0.0.
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
        luma_strength: Param,
        /// Chroma-plane grain strength. Range 0.0..=100.0 (neutral: 0.0).
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
        threshold: Param,
        /// Gaussian blur radius (sigma) in pixels. Range 0.5..=50.0.
        radius: Param,
        /// Additive blend strength. Range 0.0..=2.0 (neutral: 0.0).
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
        shadows_lift: [Param; 3],
        /// Midtones gamma, per channel. Range 0.1..=10.0 (neutral: 1.0; must be > 0).
        midtones_gamma: [Param; 3],
        /// Highlights gain, per channel. Range 0.0..=4.0 (neutral: 1.0).
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
        master: Vec<[f32; 2]>,
        /// Red channel curve control points. Empty = identity.
        red: Vec<[f32; 2]>,
        /// Green channel curve control points. Empty = identity.
        green: Vec<[f32; 2]>,
        /// Blue channel curve control points. Empty = identity.
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
        hue_shift: Param,
        /// Saturation multiplier. Range 0.0..=2.0 (neutral: 1.0).
        saturation: Param,
        /// Lightness offset. Range −1.0..=1.0 (neutral: 0.0).
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
        path: String,
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
            } => {
                let all_const =
                    brightness.is_const() && contrast.is_const() && saturation.is_const();
                if all_const {
                    // Safe: `all_const` guarantees every `as_const` is `Some`.
                    let b = brightness.as_const().unwrap_or(0.0);
                    let c = contrast.as_const().unwrap_or(1.0);
                    let s = saturation.as_const().unwrap_or(1.0);
                    #[allow(clippy::float_cmp)]
                    let neutral = b == 0.0 && c == 1.0 && s == 1.0;
                    if neutral {
                        return None;
                    }
                    Some(FilterStep::Eq {
                        brightness: b as f32,
                        contrast: c as f32,
                        saturation: s as f32,
                    })
                } else {
                    Some(FilterStep::EqAnimated {
                        brightness: brightness.to_animated(),
                        contrast: contrast.to_animated(),
                        saturation: saturation.to_animated(),
                        gamma: AnimatedValue::Static(1.0),
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
        }
    }
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

    #[test]
    fn color_correct_neutral_const_should_compile_to_nothing() {
        let kind = EffectKind::ColorCorrect {
            brightness: Param::Const(0.0),
            contrast: Param::Const(1.0),
            saturation: Param::Const(1.0),
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
        };
        let step = kind.to_filter_step().unwrap();
        match step {
            FilterStep::Eq {
                brightness,
                contrast,
                saturation,
            } => {
                assert!((brightness - 0.5).abs() < 1e-6);
                assert!((contrast - 1.2).abs() < 1e-6);
                assert!((saturation - 0.8).abs() < 1e-6);
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
}
