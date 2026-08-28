//! Blend mode definitions for video compositing operations.

// BlendMode

/// Specifies how two video layers are combined during compositing.
///
/// Each variant corresponds **1:1** to a mode of `FFmpeg`'s `blend` filter
/// `all_mode` option (40 modes total, matching `vf_blend.c`). [`BlendMode::Normal`]
/// is the standard alpha-over composite, built via the `overlay` filter; every
/// other variant is built via `blend all_mode=<token>`. The canonical token for
/// each variant is provided by `FfmpegToken` (all variants map to a valid token).
///
/// For **Porter-Duff alpha compositing** (over / under / in / out / atop / xor)
/// use [`CompositeOp`](crate::CompositeOp) instead — that is a separate concept
/// (alpha channel operators, not pixel-value blend modes). Note `BlendMode::Xor`
/// is the *arithmetic* `xor` blend, distinct from `CompositeOp::Xor`.
// Open catalog: mirrors `FFmpeg`'s `blend all_mode` set, which can grow.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BlendMode {
    // Standard modes
    /// Standard alpha-over composite (`top * opacity + bottom * (1 − opacity)`).
    /// Built via `overlay=format=auto:shortest=1` (token: `normal`).
    Normal,
    /// Multiply per-channel pixel values; darkens. `blend all_mode=multiply`.
    Multiply,
    /// Inverse of multiply; lightens. `blend all_mode=screen`.
    Screen,
    /// Multiply/Screen by base luminance. `blend all_mode=overlay`.
    Overlay,
    /// Gentle contrast enhancement. `blend all_mode=softlight`.
    SoftLight,
    /// Harsher Overlay driven by the top layer. `blend all_mode=hardlight`.
    HardLight,
    /// Brightens the base. `blend all_mode=dodge`.
    ColorDodge,
    /// Darkens the base. `blend all_mode=burn`.
    ColorBurn,
    /// Keeps the darker pixel per channel. `blend all_mode=darken`.
    Darken,
    /// Keeps the lighter pixel per channel. `blend all_mode=lighten`.
    Lighten,
    /// Per-channel absolute difference. `blend all_mode=difference`.
    Difference,
    /// Lower-contrast Difference. `blend all_mode=exclusion`.
    Exclusion,
    /// Linear addition, clamped. `blend all_mode=addition`.
    Add,
    /// Linear subtraction, clamped. `blend all_mode=subtract`.
    Subtract,

    // Additional FFmpeg blend modes
    /// Bitwise AND of the two pixels. `blend all_mode=and`.
    And,
    /// Arithmetic mean of the two pixels. `blend all_mode=average`.
    Average,
    /// Bleach-bypass look. `blend all_mode=bleach`.
    Bleach,
    /// Per-channel division. `blend all_mode=divide`.
    Divide,
    /// Extremity (distance from mid-grey). `blend all_mode=extremity`.
    Extremity,
    /// Freeze. `blend all_mode=freeze`.
    Freeze,
    /// Geometric mean. `blend all_mode=geometric`.
    Geometric,
    /// Glow. `blend all_mode=glow`.
    Glow,
    /// Grain extract (alias of `difference128`). `blend all_mode=grainextract`.
    GrainExtract,
    /// Grain merge (alias of `addition128`). `blend all_mode=grainmerge`.
    GrainMerge,
    /// Hard mix. `blend all_mode=hardmix`.
    HardMix,
    /// Hard overlay. `blend all_mode=hardoverlay`.
    HardOverlay,
    /// Harmonic mean. `blend all_mode=harmonic`.
    Harmonic,
    /// Heat. `blend all_mode=heat`.
    Heat,
    /// Linear interpolation. `blend all_mode=interpolate`.
    Interpolate,
    /// Linear light. `blend all_mode=linearlight`.
    LinearLight,
    /// Multiply scaled by 128. `blend all_mode=multiply128`.
    Multiply128,
    /// Negation. `blend all_mode=negation`.
    Negation,
    /// Bitwise OR of the two pixels. `blend all_mode=or`.
    Or,
    /// Phoenix. `blend all_mode=phoenix`.
    Phoenix,
    /// Pin light. `blend all_mode=pinlight`.
    PinLight,
    /// Reflect. `blend all_mode=reflect`.
    Reflect,
    /// Soft difference. `blend all_mode=softdifference`.
    SoftDifference,
    /// Stain. `blend all_mode=stain`.
    Stain,
    /// Vivid light. `blend all_mode=vividlight`.
    VividLight,
    /// Arithmetic XOR of the two pixels. `blend all_mode=xor`.
    /// Distinct from the Porter-Duff [`CompositeOp::Xor`](crate::CompositeOp).
    Xor,
}
