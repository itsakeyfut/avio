#![forbid(clippy::wildcard_enum_match_arm)]

use ff_format::{AlphaMode, ColorRange, ColorSpace, PixelFormat};

use crate::graph::FfmpegToken;

impl FfmpegToken for ColorRange {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // color_range_names[] (format filter color_ranges=): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c
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
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // color_space_names[] (format filter color_spaces=): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c
        Some(match self {
            Self::Bt709 => "bt709",
            Self::Bt2020 => "bt2020nc",
            Self::Srgb => "gbr",
            // TODO(#1217): split `Bt601` into `bt470bg` (625) / `smpte170m` (525); skip until then.
            Self::Bt601 => return None,
            Self::DciP3 => return None,
            Self::Unknown | _ => {
                return None;
            }
        })
    }
}

impl FfmpegToken for PixelFormat {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // av_pix_fmt_descriptors[] (format filter pix_fmts=): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c
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
            Self::Other(_) => return None, // TODO
            _ => {
                return None;
            }
        })
    }
}

impl FfmpegToken for AlphaMode {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // overlay alpha= option (alpha_format unit): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/vf_overlay.c
        Some(match self {
            Self::Straight => "straight",
            Self::Premultiplied => "premultiplied",
            Self::Unknown | _ => {
                return None;
            }
        })
    }
}
