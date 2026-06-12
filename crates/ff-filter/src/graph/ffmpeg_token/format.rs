#![forbid(clippy::wildcard_enum_match_arm)]

use ff_format::{AlphaMode, ColorPrimaries, ColorRange, ColorSpace, ColorTransfer, PixelFormat};

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
            Self::Bt470bg => "bt470bg",
            Self::Smpte170m => "smpte170m",
            Self::Bt2020Ncl => "bt2020nc",
            Self::Bt2020Cl => "bt2020c",
            Self::Rgb => "gbr",
            Self::Fcc => "fcc",
            Self::Smpte240m => "smpte240m",
            Self::Ycgco => "ycgco",
            Self::Unknown | _ => {
                return None;
            }
        })
    }
}

impl FfmpegToken for ColorPrimaries {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // color_primaries_names[] (setparams color_primaries=): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c
        Some(match self {
            Self::Bt709 => "bt709",
            Self::Bt470bg => "bt470bg",
            Self::Smpte170m => "smpte170m",
            Self::Smpte240m => "smpte240m",
            Self::Film => "film",
            Self::DciP3 => "smpte431",
            Self::DisplayP3 => "smpte432",
            Self::Bt2020 => "bt2020",
            Self::Unknown | _ => {
                return None;
            }
        })
    }
}

impl FfmpegToken for ColorTransfer {
    fn ffmpeg_token(&self) -> Option<&'static str> {
        // color_transfer_names[] (setparams color_trc=): https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavutil/pixdesc.c
        Some(match self {
            Self::Bt709 => "bt709",
            Self::Gamma22 => "gamma22",
            Self::Gamma28 => "gamma28",
            Self::Smpte170m => "smpte170m",
            Self::Smpte240m => "smpte240m",
            Self::Linear => "linear",
            Self::Srgb => "iec61966-2-1",
            Self::Bt2020_10 => "bt2020-10",
            Self::Bt2020_12 => "bt2020-12",
            Self::Pq => "smpte2084",
            Self::Hlg => "arib-std-b67",
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

#[cfg(test)]
mod tests {
    use super::*;

    // These deterministic string-equality tests are the only regression guard for the
    // colour-token mappings until the setparams consumer lands (#1227); they assert the
    // FFmpeg-canonical token, which deliberately differs from `name()` (e.g. `gbr` vs `rgb`).

    #[test]
    fn color_space_should_emit_canonical_tokens() {
        assert_eq!(ColorSpace::Bt709.ffmpeg_token(), Some("bt709"));
        assert_eq!(ColorSpace::Bt470bg.ffmpeg_token(), Some("bt470bg"));
        assert_eq!(ColorSpace::Smpte170m.ffmpeg_token(), Some("smpte170m"));
        assert_eq!(ColorSpace::Bt2020Ncl.ffmpeg_token(), Some("bt2020nc"));
        assert_eq!(ColorSpace::Bt2020Cl.ffmpeg_token(), Some("bt2020c"));
        assert_eq!(ColorSpace::Rgb.ffmpeg_token(), Some("gbr"));
        assert_eq!(ColorSpace::Fcc.ffmpeg_token(), Some("fcc"));
        assert_eq!(ColorSpace::Smpte240m.ffmpeg_token(), Some("smpte240m"));
        assert_eq!(ColorSpace::Ycgco.ffmpeg_token(), Some("ycgco"));
        assert_eq!(ColorSpace::Unknown.ffmpeg_token(), None);
    }

    #[test]
    fn color_primaries_should_emit_canonical_tokens() {
        assert_eq!(ColorPrimaries::Bt709.ffmpeg_token(), Some("bt709"));
        assert_eq!(ColorPrimaries::Bt470bg.ffmpeg_token(), Some("bt470bg"));
        assert_eq!(ColorPrimaries::Smpte170m.ffmpeg_token(), Some("smpte170m"));
        assert_eq!(ColorPrimaries::Smpte240m.ffmpeg_token(), Some("smpte240m"));
        assert_eq!(ColorPrimaries::Film.ffmpeg_token(), Some("film"));
        assert_eq!(ColorPrimaries::DciP3.ffmpeg_token(), Some("smpte431"));
        assert_eq!(ColorPrimaries::DisplayP3.ffmpeg_token(), Some("smpte432"));
        assert_eq!(ColorPrimaries::Bt2020.ffmpeg_token(), Some("bt2020"));
        assert_eq!(ColorPrimaries::Unknown.ffmpeg_token(), None);
    }

    #[test]
    fn color_transfer_should_emit_canonical_tokens() {
        assert_eq!(ColorTransfer::Bt709.ffmpeg_token(), Some("bt709"));
        assert_eq!(ColorTransfer::Gamma22.ffmpeg_token(), Some("gamma22"));
        assert_eq!(ColorTransfer::Gamma28.ffmpeg_token(), Some("gamma28"));
        assert_eq!(ColorTransfer::Smpte170m.ffmpeg_token(), Some("smpte170m"));
        assert_eq!(ColorTransfer::Smpte240m.ffmpeg_token(), Some("smpte240m"));
        assert_eq!(ColorTransfer::Linear.ffmpeg_token(), Some("linear"));
        assert_eq!(ColorTransfer::Srgb.ffmpeg_token(), Some("iec61966-2-1"));
        assert_eq!(ColorTransfer::Bt2020_10.ffmpeg_token(), Some("bt2020-10"));
        assert_eq!(ColorTransfer::Bt2020_12.ffmpeg_token(), Some("bt2020-12"));
        assert_eq!(ColorTransfer::Pq.ffmpeg_token(), Some("smpte2084"));
        assert_eq!(ColorTransfer::Hlg.ffmpeg_token(), Some("arib-std-b67"));
        assert_eq!(ColorTransfer::Unknown.ffmpeg_token(), None);
    }
}
