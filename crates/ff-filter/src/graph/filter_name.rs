use crate::{BlendMode, ScaleAlgorithm, ToneMap, XfadeTransition, YadifMode};

pub trait FfmpegToken {
    fn ffmpeg_token(&self) -> &'static str;
}

impl FfmpegToken for ScaleAlgorithm {
    fn ffmpeg_token(&self) -> &'static str {
        // https://ffmpeg.org/ffmpeg-filters.html#Scaling
        match self {
            Self::Fast => "fast_bilinear",
            Self::Bilinear => "bilinear",
            Self::Bicubic => "bicubic",
            Self::Lanczos => "lanczos",
        }
    }
}

impl FfmpegToken for ToneMap {
    fn ffmpeg_token(&self) -> &'static str {
        // https://ffmpeg.org/ffmpeg-filters.html#Tone-mapping
        match self {
            Self::Hable => "hable",
            Self::Reinhard => "reinhard",
            Self::Mobius => "mobius",
        }
    }
}

impl FfmpegToken for YadifMode {
    fn ffmpeg_token(&self) -> &'static str {
        // https://ffmpeg.org/ffmpeg-filters.html#yadif-1
        match self {
            Self::Frame => "0",
            Self::Field => "1",
            Self::FrameNospatial => "2",
            Self::FieldNospatial => "3",
        }
    }
}

impl FfmpegToken for XfadeTransition {
    fn ffmpeg_token(&self) -> &'static str {
        // https://ffmpeg.org/ffmpeg-filters.html#xfade
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
        }
    }
}

impl FfmpegToken for BlendMode {
    fn ffmpeg_token(&self) -> &'static str {
        // https://ffmpeg.org/ffmpeg-filters.html#blend-1
        match self {
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
            Self::Hue => todo!(),
            Self::Saturation => todo!(),
            Self::Color => todo!(),
            Self::Luminosity => todo!(),
            Self::PorterDuffOver => todo!(),
            Self::PorterDuffUnder => todo!(),
            Self::PorterDuffIn => todo!(),
            Self::PorterDuffOut => todo!(),
            Self::PorterDuffAtop => todo!(),
            Self::PorterDuffXor => todo!(),
        }
    }
}
