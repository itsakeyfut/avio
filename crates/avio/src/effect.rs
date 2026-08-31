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

use ff_filter::{AnimatedValue, AnimationTrack, FilterStep};

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
/// The v1 curated set is [`ColorCorrect`](Self::ColorCorrect), [`Blur`](Self::Blur),
/// and [`Sharpen`](Self::Sharpen).
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
}
