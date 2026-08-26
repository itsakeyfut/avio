//! Type mapping functions: `FFmpeg` codec/format IDs to our safe enums, and PTS conversion.

use std::time::Duration;

use ff_format::codec::{AudioCodec, SubtitleCodec, VideoCodec};
use ff_format::color::{ColorPrimaries, ColorRange, ColorSpace};
use ff_format::{PixelFormat, Rational, SampleFormat};

/// Maps an `FFmpeg` `AVCodecID` to our [`VideoCodec`] enum.
pub(super) fn map_video_codec(codec_id: ff_sys::AVCodecID) -> VideoCodec {
    match codec_id {
        ff_sys::AVCodecID_AV_CODEC_ID_H264 => VideoCodec::H264,
        ff_sys::AVCodecID_AV_CODEC_ID_HEVC => VideoCodec::H265,
        ff_sys::AVCodecID_AV_CODEC_ID_VP8 => VideoCodec::Vp8,
        ff_sys::AVCodecID_AV_CODEC_ID_VP9 => VideoCodec::Vp9,
        ff_sys::AVCodecID_AV_CODEC_ID_AV1 => VideoCodec::Av1,
        ff_sys::AVCodecID_AV_CODEC_ID_PRORES => VideoCodec::ProRes,
        ff_sys::AVCodecID_AV_CODEC_ID_MPEG4 => VideoCodec::Mpeg4,
        ff_sys::AVCodecID_AV_CODEC_ID_MPEG2VIDEO => VideoCodec::Mpeg2,
        ff_sys::AVCodecID_AV_CODEC_ID_MJPEG => VideoCodec::Mjpeg,
        _ => {
            log::warn!(
                "video_codec has no mapping, using Unknown \
                 codec_id={codec_id}"
            );
            VideoCodec::Unknown
        }
    }
}

/// Maps an `FFmpeg` `AVPixelFormat` to our [`PixelFormat`] enum.
pub(super) fn map_pixel_format(format: i32) -> PixelFormat {
    #[expect(clippy::cast_sign_loss, reason = "AVPixelFormat values are positive")]
    let format_u32 = format as u32;

    match format_u32 {
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24 as u32 => PixelFormat::Rgb24,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA as u32 => PixelFormat::Rgba,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24 as u32 => PixelFormat::Bgr24,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA as u32 => PixelFormat::Bgra,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P as u32 => PixelFormat::Yuv420p,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P as u32 => PixelFormat::Yuv422p,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P as u32 => PixelFormat::Yuv444p,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_NV12 as u32 => PixelFormat::Nv12,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_NV21 as u32 => PixelFormat::Nv21,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE as u32 => PixelFormat::Yuv420p10le,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE as u32 => PixelFormat::Yuv422p10le,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE as u32 => PixelFormat::P010le,
        x if x == ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8 as u32 => PixelFormat::Gray8,
        _ => {
            log::warn!(
                "pixel_format has no mapping, using Other \
                 format={format_u32}"
            );
            PixelFormat::Other(format_u32)
        }
    }
}

/// Maps an `FFmpeg` `AVColorSpace` to our [`ColorSpace`] enum.
pub(super) fn map_color_space(color_space: ff_sys::AVColorSpace) -> ColorSpace {
    match color_space {
        ff_sys::AVColorSpace_AVCOL_SPC_BT709 => ColorSpace::Bt709,
        ff_sys::AVColorSpace_AVCOL_SPC_BT470BG => ColorSpace::Bt470bg,
        ff_sys::AVColorSpace_AVCOL_SPC_SMPTE170M => ColorSpace::Smpte170m,
        ff_sys::AVColorSpace_AVCOL_SPC_SMPTE240M => ColorSpace::Smpte240m,
        ff_sys::AVColorSpace_AVCOL_SPC_FCC => ColorSpace::Fcc,
        ff_sys::AVColorSpace_AVCOL_SPC_YCGCO => ColorSpace::Ycgco,
        ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL => ColorSpace::Bt2020Ncl,
        ff_sys::AVColorSpace_AVCOL_SPC_BT2020_CL => ColorSpace::Bt2020Cl,
        ff_sys::AVColorSpace_AVCOL_SPC_RGB => ColorSpace::Rgb,
        _ => {
            log::warn!(
                "color_space has no mapping, using Unknown \
                 color_space={color_space}"
            );
            ColorSpace::Unknown
        }
    }
}

/// Maps an `FFmpeg` `AVColorRange` to our [`ColorRange`] enum.
pub(super) fn map_color_range(color_range: ff_sys::AVColorRange) -> ColorRange {
    match color_range {
        ff_sys::AVColorRange_AVCOL_RANGE_MPEG => ColorRange::Limited,
        ff_sys::AVColorRange_AVCOL_RANGE_JPEG => ColorRange::Full,
        _ => {
            log::warn!(
                "color_range has no mapping, using Unknown \
                 color_range={color_range}"
            );
            ColorRange::Unknown
        }
    }
}

/// Maps an `FFmpeg` `AVColorPrimaries` to our [`ColorPrimaries`] enum.
pub(super) fn map_color_primaries(color_primaries: ff_sys::AVColorPrimaries) -> ColorPrimaries {
    match color_primaries {
        ff_sys::AVColorPrimaries_AVCOL_PRI_BT709 => ColorPrimaries::Bt709,
        ff_sys::AVColorPrimaries_AVCOL_PRI_BT470BG => ColorPrimaries::Bt470bg,
        ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE170M => ColorPrimaries::Smpte170m,
        ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE240M => ColorPrimaries::Smpte240m,
        ff_sys::AVColorPrimaries_AVCOL_PRI_FILM => ColorPrimaries::Film,
        ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE431 => ColorPrimaries::DciP3,
        ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE432 => ColorPrimaries::DisplayP3,
        ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020 => ColorPrimaries::Bt2020,
        _ => {
            log::warn!(
                "color_primaries has no mapping, using Unknown \
                 color_primaries={color_primaries}"
            );
            ColorPrimaries::Unknown
        }
    }
}

/// Maps an `FFmpeg` `AVCodecID` to our [`AudioCodec`] enum.
pub(super) fn map_audio_codec(codec_id: ff_sys::AVCodecID) -> AudioCodec {
    match codec_id {
        ff_sys::AVCodecID_AV_CODEC_ID_AAC => AudioCodec::Aac,
        ff_sys::AVCodecID_AV_CODEC_ID_MP3 => AudioCodec::Mp3,
        ff_sys::AVCodecID_AV_CODEC_ID_OPUS => AudioCodec::Opus,
        ff_sys::AVCodecID_AV_CODEC_ID_FLAC => AudioCodec::Flac,
        ff_sys::AVCodecID_AV_CODEC_ID_VORBIS => AudioCodec::Vorbis,
        ff_sys::AVCodecID_AV_CODEC_ID_AC3 => AudioCodec::Ac3,
        ff_sys::AVCodecID_AV_CODEC_ID_EAC3 => AudioCodec::Eac3,
        ff_sys::AVCodecID_AV_CODEC_ID_DTS => AudioCodec::Dts,
        ff_sys::AVCodecID_AV_CODEC_ID_ALAC => AudioCodec::Alac,
        // PCM variants
        ff_sys::AVCodecID_AV_CODEC_ID_PCM_S16LE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_S16BE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_S24LE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_S24BE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_S32LE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_S32BE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_F32LE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_F32BE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_F64LE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_F64BE
        | ff_sys::AVCodecID_AV_CODEC_ID_PCM_U8 => AudioCodec::Pcm,
        _ => {
            log::warn!(
                "audio_codec has no mapping, using Unknown \
                 codec_id={codec_id}"
            );
            AudioCodec::Unknown
        }
    }
}

/// Maps an `FFmpeg` `AVSampleFormat` to our [`SampleFormat`] enum.
pub(super) fn map_sample_format(format: i32) -> SampleFormat {
    #[expect(clippy::cast_sign_loss, reason = "AVSampleFormat values are positive")]
    let format_u32 = format as u32;

    match format_u32 {
        // Packed (interleaved) formats
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8 as u32 => SampleFormat::U8,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16 as u32 => SampleFormat::I16,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32 as u32 => SampleFormat::I32,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT as u32 => SampleFormat::F32,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBL as u32 => SampleFormat::F64,
        // Planar formats
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8P as u32 => SampleFormat::U8p,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16P as u32 => SampleFormat::I16p,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32P as u32 => SampleFormat::I32p,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLTP as u32 => SampleFormat::F32p,
        x if x == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBLP as u32 => SampleFormat::F64p,
        // Unknown format
        _ => {
            log::warn!(
                "sample_format has no mapping, using Other \
                 format={format_u32}"
            );
            SampleFormat::Other(format_u32)
        }
    }
}

/// Maps an `FFmpeg` `AVCodecID` to our [`SubtitleCodec`] enum.
pub(super) fn map_subtitle_codec(codec_id: ff_sys::AVCodecID) -> SubtitleCodec {
    match codec_id {
        ff_sys::AVCodecID_AV_CODEC_ID_SRT | ff_sys::AVCodecID_AV_CODEC_ID_SUBRIP => {
            SubtitleCodec::Srt
        }
        ff_sys::AVCodecID_AV_CODEC_ID_SSA | ff_sys::AVCodecID_AV_CODEC_ID_ASS => SubtitleCodec::Ass,
        ff_sys::AVCodecID_AV_CODEC_ID_DVB_SUBTITLE => SubtitleCodec::Dvb,
        ff_sys::AVCodecID_AV_CODEC_ID_HDMV_PGS_SUBTITLE => SubtitleCodec::Hdmv,
        ff_sys::AVCodecID_AV_CODEC_ID_WEBVTT => SubtitleCodec::Webvtt,
        _ => {
            let name = super::video::extract_codec_name(codec_id);
            log::warn!("unknown subtitle codec codec_id={codec_id}");
            SubtitleCodec::Other(name)
        }
    }
}

/// Converts a PTS value to a [`Duration`] using the given time base.
///
/// Returns [`Duration::ZERO`] for non-positive PTS values.
pub(super) fn pts_to_duration(pts: i64, time_base: Rational) -> Duration {
    if pts <= 0 {
        return Duration::ZERO;
    }
    // secs = pts * num / den
    // Note: precision loss from i64/i32 to f64 is acceptable for media timestamps
    #[expect(clippy::cast_precision_loss, reason = "media timestamps are bounded")]
    let secs = (pts as f64) * f64::from(time_base.num()) / f64::from(time_base.den());
    if secs > 0.0 {
        Duration::from_secs_f64(secs)
    } else {
        Duration::ZERO
    }
}
