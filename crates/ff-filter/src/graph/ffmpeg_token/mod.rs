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
        })
    }
}

impl FfmpegToken for BlendMode {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // all_mode unit: https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_blend.c
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
            Self::Hue => return None,             // TODO
            Self::Saturation => return None,      // TODO
            Self::Color => return None,           // TODO
            Self::Luminosity => return None,      // TODO
            Self::PorterDuffOver => return None,  // TODO
            Self::PorterDuffUnder => return None, // TODO
            Self::PorterDuffIn => return None,    // TODO
            Self::PorterDuffOut => return None,   // TODO
            Self::PorterDuffAtop => return None,  // TODO
            Self::PorterDuffXor => return None,   // TODO
        })
    }
}
