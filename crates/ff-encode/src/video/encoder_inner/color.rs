//! Color format conversion helpers.
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]

use super::AVPixelFormat;

/// Convert ff-format PixelFormat to FFmpeg AVPixelFormat.
pub(super) fn pixel_format_to_av(format: ff_format::PixelFormat) -> AVPixelFormat {
    use ff_format::PixelFormat;

    match format {
        PixelFormat::Yuv420p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P,
        PixelFormat::Yuv422p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P,
        PixelFormat::Yuv444p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P,
        PixelFormat::Rgb24 => ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24,
        PixelFormat::Bgr24 => ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24,
        PixelFormat::Rgba => ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA,
        PixelFormat::Bgra => ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA,
        PixelFormat::Gray8 => ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8,
        PixelFormat::Nv12 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV12,
        PixelFormat::Nv21 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV21,
        PixelFormat::Yuv420p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE,
        PixelFormat::Yuv422p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE,
        PixelFormat::Yuv444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE,
        PixelFormat::Yuva444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUVA444P10LE,
        PixelFormat::P010le => ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE,
        PixelFormat::Other(v) => v as AVPixelFormat,
        _ => {
            log::warn!(
                "pixel_format has no AV mapping, falling back to Yuv420p \
                 format={format:?} fallback=AV_PIX_FMT_YUV420P"
            );
            ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P
        }
    }
}

/// Convert FFmpeg AVPixelFormat back to ff-format PixelFormat.
pub(super) fn from_av_pixel_format(fmt: AVPixelFormat) -> ff_format::PixelFormat {
    use ff_format::PixelFormat;
    if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P {
        PixelFormat::Yuv420p
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P {
        PixelFormat::Yuv422p
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P {
        PixelFormat::Yuv444p
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24 {
        PixelFormat::Rgb24
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24 {
        PixelFormat::Bgr24
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA {
        PixelFormat::Rgba
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA {
        PixelFormat::Bgra
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8 {
        PixelFormat::Gray8
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_NV12 {
        PixelFormat::Nv12
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_NV21 {
        PixelFormat::Nv21
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE {
        PixelFormat::Yuv420p10le
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE {
        PixelFormat::Yuv422p10le
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE {
        PixelFormat::Yuv444p10le
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUVA444P10LE {
        PixelFormat::Yuva444p10le
    } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE {
        PixelFormat::P010le
    } else {
        PixelFormat::Other(fmt as u32)
    }
}

/// Convert ff-format ColorSpace to the FFmpeg AVColorSpace constant.
pub(super) fn color_space_to_av(cs: ff_format::ColorSpace) -> ff_sys::AVColorSpace {
    use ff_format::ColorSpace;
    match cs {
        ColorSpace::Bt709 => ff_sys::AVColorSpace_AVCOL_SPC_BT709,
        ColorSpace::Bt470bg => ff_sys::AVColorSpace_AVCOL_SPC_BT470BG,
        ColorSpace::Smpte170m => ff_sys::AVColorSpace_AVCOL_SPC_SMPTE170M,
        ColorSpace::Smpte240m => ff_sys::AVColorSpace_AVCOL_SPC_SMPTE240M,
        ColorSpace::Fcc => ff_sys::AVColorSpace_AVCOL_SPC_FCC,
        ColorSpace::Ycgco => ff_sys::AVColorSpace_AVCOL_SPC_YCGCO,
        ColorSpace::Bt2020Ncl => ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL,
        ColorSpace::Bt2020Cl => ff_sys::AVColorSpace_AVCOL_SPC_BT2020_CL,
        ColorSpace::Rgb => ff_sys::AVColorSpace_AVCOL_SPC_RGB,
        ColorSpace::Unknown => ff_sys::AVColorSpace_AVCOL_SPC_UNSPECIFIED,
        _ => ff_sys::AVColorSpace_AVCOL_SPC_UNSPECIFIED,
    }
}

/// Convert ff-format ColorTransfer to the FFmpeg AVColorTransferCharacteristic constant.
pub(super) fn color_transfer_to_av(
    trc: ff_format::ColorTransfer,
) -> ff_sys::AVColorTransferCharacteristic {
    use ff_format::ColorTransfer;
    match trc {
        ColorTransfer::Bt709 => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_BT709,
        ColorTransfer::Gamma22 => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_GAMMA22,
        ColorTransfer::Gamma28 => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_GAMMA28,
        ColorTransfer::Smpte170m => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_SMPTE170M,
        ColorTransfer::Smpte240m => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_SMPTE240M,
        ColorTransfer::Linear => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_LINEAR,
        ColorTransfer::Srgb => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_IEC61966_2_1,
        ColorTransfer::Bt2020_10 => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_BT2020_10,
        ColorTransfer::Bt2020_12 => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_BT2020_12,
        ColorTransfer::Pq => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_SMPTEST2084,
        ColorTransfer::Hlg => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_ARIB_STD_B67,
        ColorTransfer::Unknown => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_UNSPECIFIED,
        _ => ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_UNSPECIFIED,
    }
}

/// Convert ff-format ColorPrimaries to the FFmpeg AVColorPrimaries constant.
pub(super) fn color_primaries_to_av(cp: ff_format::ColorPrimaries) -> ff_sys::AVColorPrimaries {
    use ff_format::ColorPrimaries;
    match cp {
        ColorPrimaries::Bt709 => ff_sys::AVColorPrimaries_AVCOL_PRI_BT709,
        ColorPrimaries::Bt470bg => ff_sys::AVColorPrimaries_AVCOL_PRI_BT470BG,
        ColorPrimaries::Smpte170m => ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE170M,
        ColorPrimaries::Smpte240m => ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE240M,
        ColorPrimaries::Film => ff_sys::AVColorPrimaries_AVCOL_PRI_FILM,
        ColorPrimaries::DciP3 => ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE431,
        ColorPrimaries::DisplayP3 => ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE432,
        ColorPrimaries::Bt2020 => ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020,
        ColorPrimaries::Unknown => ff_sys::AVColorPrimaries_AVCOL_PRI_UNSPECIFIED,
        _ => ff_sys::AVColorPrimaries_AVCOL_PRI_UNSPECIFIED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::PixelFormat;

    /// `PixelFormat::Other(v)` must pass through to the raw `AVPixelFormat` integer unchanged,
    /// not fall back to `AV_PIX_FMT_YUV420P`.
    #[test]
    fn pixel_format_other_should_passthrough_av_value() {
        // AVPixelFormat value 29 = AV_PIX_FMT_GBRP on most FFmpeg builds.
        let result = pixel_format_to_av(PixelFormat::Other(29));
        assert_eq!(
            result, 29,
            "Other(29) must map to AVPixelFormat 29, got {result}"
        );
    }

    /// An unrecognised `AVPixelFormat` integer must be wrapped in `PixelFormat::Other`,
    /// not silently coerced to `Yuv420p`.
    #[test]
    fn from_av_pixel_format_unknown_should_return_other() {
        let result = from_av_pixel_format(99);
        assert_eq!(
            result,
            PixelFormat::Other(99),
            "AVPixelFormat 99 must yield Other(99), got {result:?}"
        );
    }

    /// Round-trip: `from_av_pixel_format(pixel_format_to_av(Other(v))) == Other(v)`.
    /// Acceptance criterion from issue #1018.
    #[test]
    fn pixel_format_other_round_trip_should_be_identity() {
        let original = PixelFormat::Other(29);
        let av_fmt = pixel_format_to_av(original);
        let round_tripped = from_av_pixel_format(av_fmt);
        assert_eq!(
            round_tripped, original,
            "Other(29) round-trip must be identity; got {round_tripped:?}"
        );
    }
}

/// Convert ff-format SampleFormat to FFmpeg AVSampleFormat.
pub(super) fn sample_format_to_av(format: ff_format::SampleFormat) -> ff_sys::AVSampleFormat {
    use ff_format::SampleFormat;
    use ff_sys::swresample::sample_format;

    match format {
        SampleFormat::U8 => sample_format::U8,
        SampleFormat::I16 => sample_format::S16,
        SampleFormat::I32 => sample_format::S32,
        SampleFormat::F32 => sample_format::FLT,
        SampleFormat::F64 => sample_format::DBL,
        SampleFormat::U8p => sample_format::U8P,
        SampleFormat::I16p => sample_format::S16P,
        SampleFormat::I32p => sample_format::S32P,
        SampleFormat::F32p => sample_format::FLTP,
        SampleFormat::F64p => sample_format::DBLP,
        _ => {
            log::warn!(
                "sample_format has no AV mapping, falling back to FLTP \
                 format={format:?} fallback=FLTP"
            );
            sample_format::FLTP
        }
    }
}
