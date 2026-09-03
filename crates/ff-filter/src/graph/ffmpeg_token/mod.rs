#![forbid(clippy::wildcard_enum_match_arm)]

use crate::{BlendMode, ScaleAlgorithm, ToneMap, XfadeTransition, YadifMode};

mod format;

pub trait FfmpegToken {
    fn ffmpeg_token(&self) -> Option<&'static str>;
}

impl FfmpegToken for ScaleAlgorithm {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // sws_flags unit: https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libswscale/options.c
        Some(match self {
            Self::Fast => "fast_bilinear",
            Self::Bilinear => "bilinear",
            Self::Bicubic => "bicubic",
            Self::Lanczos => "lanczos",
        })
    }
}

impl FfmpegToken for ToneMap {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // tonemap unit: https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_tonemap.c
        Some(match self {
            Self::Hable => "hable",
            Self::Reinhard => "reinhard",
            Self::Mobius => "mobius",
        })
    }
}

impl FfmpegToken for YadifMode {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // mode unit (AV_OPT_TYPE_INT, range 0-3): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/yadif_common.c
        Some(match self {
            Self::Frame => "0",
            Self::Field => "1",
            Self::FrameNospatial => "2",
            Self::FieldNospatial => "3",
        })
    }
}

impl FfmpegToken for XfadeTransition {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // transition unit: https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_xfade.c
        Some(match self {
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
        })
    }
}

impl FfmpegToken for BlendMode {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // all_mode unit (every BlendMode maps 1:1 to a token): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_blend.c
        Some(match self {
            Self::Normal => "normal",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
            Self::SoftLight => "softlight",
            Self::HardLight => "hardlight",
            Self::ColorDodge => "dodge",
            Self::ColorBurn => "burn",
            Self::Darken => "darken",
            Self::Lighten => "lighten",
            Self::Difference => "difference",
            Self::Exclusion => "exclusion",
            Self::Add => "addition",
            Self::Subtract => "subtract",
            Self::And => "and",
            Self::Average => "average",
            Self::Bleach => "bleach",
            Self::Divide => "divide",
            Self::Extremity => "extremity",
            Self::Freeze => "freeze",
            Self::Geometric => "geometric",
            Self::Glow => "glow",
            Self::GrainExtract => "grainextract",
            Self::GrainMerge => "grainmerge",
            Self::HardMix => "hardmix",
            Self::HardOverlay => "hardoverlay",
            Self::Harmonic => "harmonic",
            Self::Heat => "heat",
            Self::Interpolate => "interpolate",
            Self::LinearLight => "linearlight",
            Self::Multiply128 => "multiply128",
            Self::Negation => "negation",
            Self::Or => "or",
            Self::Phoenix => "phoenix",
            Self::PinLight => "pinlight",
            Self::Reflect => "reflect",
            Self::SoftDifference => "softdifference",
            Self::Stain => "stain",
            Self::VividLight => "vividlight",
            Self::Xor => "xor",
        })
    }
}

#[cfg(test)]
mod blend_mode_token_tests {
    use super::*;

    /// Every `BlendMode` variant must emit its exact FFmpeg `blend all_mode` token
    /// (verified against pinned `vf_blend.c`; the `all_mode` set is identical in
    /// release/7.1 and release/8.0). No variant returns `None`.
    #[test]
    fn blend_mode_should_emit_all_mode_token_for_every_variant() {
        let cases = [
            (BlendMode::Normal, "normal"),
            (BlendMode::Multiply, "multiply"),
            (BlendMode::Screen, "screen"),
            (BlendMode::Overlay, "overlay"),
            (BlendMode::SoftLight, "softlight"),
            (BlendMode::HardLight, "hardlight"),
            (BlendMode::ColorDodge, "dodge"),
            (BlendMode::ColorBurn, "burn"),
            (BlendMode::Darken, "darken"),
            (BlendMode::Lighten, "lighten"),
            (BlendMode::Difference, "difference"),
            (BlendMode::Exclusion, "exclusion"),
            (BlendMode::Add, "addition"),
            (BlendMode::Subtract, "subtract"),
            (BlendMode::And, "and"),
            (BlendMode::Average, "average"),
            (BlendMode::Bleach, "bleach"),
            (BlendMode::Divide, "divide"),
            (BlendMode::Extremity, "extremity"),
            (BlendMode::Freeze, "freeze"),
            (BlendMode::Geometric, "geometric"),
            (BlendMode::Glow, "glow"),
            (BlendMode::GrainExtract, "grainextract"),
            (BlendMode::GrainMerge, "grainmerge"),
            (BlendMode::HardMix, "hardmix"),
            (BlendMode::HardOverlay, "hardoverlay"),
            (BlendMode::Harmonic, "harmonic"),
            (BlendMode::Heat, "heat"),
            (BlendMode::Interpolate, "interpolate"),
            (BlendMode::LinearLight, "linearlight"),
            (BlendMode::Multiply128, "multiply128"),
            (BlendMode::Negation, "negation"),
            (BlendMode::Or, "or"),
            (BlendMode::Phoenix, "phoenix"),
            (BlendMode::PinLight, "pinlight"),
            (BlendMode::Reflect, "reflect"),
            (BlendMode::SoftDifference, "softdifference"),
            (BlendMode::Stain, "stain"),
            (BlendMode::VividLight, "vividlight"),
            (BlendMode::Xor, "xor"),
        ];
        assert_eq!(cases.len(), 40, "BlendMode must have exactly 40 modes");
        for (mode, token) in cases {
            assert_eq!(
                mode.ffmpeg_token(),
                Some(token),
                "{mode:?} must emit token {token:?}"
            );
        }
    }
}
