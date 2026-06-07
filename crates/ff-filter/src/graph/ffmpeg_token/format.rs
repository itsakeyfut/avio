#![forbid(clippy::wildcard_enum_match_arm)]

use ff_format::{ColorRange, ColorSpace, PixelFormat};

use crate::graph::FfmpegToken;

impl FfmpegToken for ColorRange {
    // https://ffmpeg.org/ffmpeg-filters.html#colorspace
    fn ffmpeg_token(&self) -> Option<&'static str> {
        Some(match self {
            Self::Limited => "tv",
            Self::Full => "pc",
            Self::Unknown | _ => {
                return None;
            }
        })
    }
}

impl FfmpegToken for ColorSpace {
    // https://ffmpeg.org/ffmpeg-filters.html#colorspace
    fn ffmpeg_token(&self) -> Option<&'static str> {
        Some(match self {
            Self::Bt709 => "bt709",
            Self::Bt601 => "bt601",
            Self::Bt2020 => "bt2020",
            Self::DciP3 => todo!(),
            Self::Srgb => todo!(),
            Self::Unknown | _ => {
                return None;
            }
        })
    }
}

impl FfmpegToken for PixelFormat {
    // ffmpeg -pix_fmts
    fn ffmpeg_token(&self) -> Option<&'static str> {
        Some(match self {
            Self::Rgb24 => "rgb24",
            Self::Rgba => "rgba",
            Self::Bgr24 => "bgr24",
            Self::Bgra => "bgra",
            Self::Yuv420p => "yuv420p",
            Self::Yuv422p => "yuv422p",
            Self::Yuv444p => "yuv444p",
            Self::Nv12 => "nv12",
            Self::Nv21 => "nv21",
            Self::Yuv420p10le => "yuv420p10le",
            Self::Yuv422p10le => "yuv422p10le",
            Self::Yuv444p10le => "yuv444p10le",
            Self::Yuva444p10le => "yuva444p10le",
            Self::P010le => "p010le",
            Self::Gray8 => "gray",
            Self::Gbrpf32le => "gbrpf32le",
            Self::Other(_) => todo!(),
            _ => {
                return None;
            }
        })
    }
}
