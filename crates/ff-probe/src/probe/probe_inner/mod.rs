//! Media file probing.
//!
//! [`probe_file`] builds a [`MediaInfo`] from an owned `ff_sys::InputFormatContext`
//! through the crate's safe accessors, so it holds no `unsafe` itself. The only
//! `unsafe` left in `ff-probe` is confined to two child helpers: the channel-mask
//! union read in [`audio`] and the `avcodec_get_name` call in [`video`]. It is the
//! sole public symbol here and is called exclusively by [`super::builder::open`]
//! after all safe preconditions (file exists, file size readable) are satisfied.

#![allow(unsafe_code)]

use std::path::Path;

use ff_format::MediaInfo;

use crate::error::ProbeError;

mod audio;
mod chapters;
mod core;
mod mapping;
mod subtitle;
mod video;

/// Opens, probes, and closes a media file, returning fully-populated [`MediaInfo`].
///
/// # Safety invariant
///
/// The caller (`builder::open`) has already verified that `path` exists and that
/// `file_size` was read successfully. `probe_file` itself performs no file-system
/// checks — it delegates those to the `FFmpeg` API.
pub(crate) fn probe_file(path: &Path, file_size: u64) -> Result<MediaInfo, ProbeError> {
    // Open the file with FFmpeg. The owned context frees itself on drop, so
    // every early return below is leak-free without an explicit close.
    let mut ctx = ff_sys::InputFormatContext::open(path).map_err(|e| ProbeError::CannotOpen {
        path: path.to_path_buf(),
        reason: ff_sys::av_error_string(e.code()),
    })?;

    // Find stream info - this populates codec information.
    ctx.find_stream_info()
        .map_err(|e| ProbeError::InvalidMedia {
            path: path.to_path_buf(),
            reason: ff_sys::av_error_string(e.code()),
        })?;

    // Extract basic information through the owned context's safe accessors.
    let (format, format_long_name, duration) = core::extract_format_info(&ctx);

    // Calculate container bitrate
    let bitrate = core::calculate_container_bitrate(&ctx, file_size, duration);

    // Extract container metadata
    let media_metadata = ctx.metadata();

    // Extract video streams
    let video_streams = video::extract_video_streams(&ctx);

    // Extract audio streams
    let audio_streams = audio::extract_audio_streams(&ctx);

    // Extract subtitle streams
    let subtitle_streams = subtitle::extract_subtitle_streams(&ctx);

    // Extract chapter info
    let chapter_list = chapters::extract_chapters(&ctx);

    // The owned `ctx` closes its input and frees itself when it drops at the end
    // of this function; no manual close is needed.

    log::debug!(
        "probe complete video_streams={} audio_streams={} subtitle_streams={} chapters={}",
        video_streams.len(),
        audio_streams.len(),
        subtitle_streams.len(),
        chapter_list.len()
    );

    // Build MediaInfo
    let mut builder = MediaInfo::builder()
        .path(path)
        .format(format)
        .duration(duration)
        .file_size(file_size)
        .video_streams(video_streams)
        .audio_streams(audio_streams)
        .subtitle_streams(subtitle_streams)
        .chapters(chapter_list)
        .metadata_map(media_metadata);

    if let Some(name) = format_long_name {
        builder = builder.format_long_name(name);
    }

    if let Some(bps) = bitrate {
        builder = builder.bitrate(bps);
    }

    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ff_format::codec::{AudioCodec, VideoCodec};
    use ff_format::color::{ColorPrimaries, ColorRange, ColorSpace};
    use ff_format::{PixelFormat, Rational, SampleFormat};

    use super::mapping::{
        map_audio_codec, map_color_primaries, map_color_range, map_color_space, map_pixel_format,
        map_sample_format, map_video_codec, pts_to_duration,
    };

    /// `AV_TIME_BASE` constant from `FFmpeg` (microseconds per second).
    const AV_TIME_BASE: i64 = 1_000_000;

    #[test]
    fn test_av_time_base_constant() {
        // Verify our constant matches the expected value
        assert_eq!(AV_TIME_BASE, 1_000_000);
    }

    // ========================================================================
    // pts_to_duration Tests
    // ========================================================================

    #[test]
    fn pts_to_duration_should_convert_millisecond_timebase_correctly() {
        // 1/1000 timebase: 5000 pts = 5 seconds
        let tb = Rational::new(1, 1000);
        let dur = pts_to_duration(5000, tb);
        assert_eq!(dur, Duration::from_secs(5));
    }

    #[test]
    fn pts_to_duration_should_convert_mpeg_ts_timebase_correctly() {
        // 1/90000 timebase: 90000 pts = 1 second
        let tb = Rational::new(1, 90000);
        let dur = pts_to_duration(90000, tb);
        assert!((dur.as_secs_f64() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pts_to_duration_should_return_zero_for_zero_pts() {
        let tb = Rational::new(1, 1000);
        assert_eq!(pts_to_duration(0, tb), Duration::ZERO);
    }

    #[test]
    fn pts_to_duration_should_return_zero_for_negative_pts() {
        let tb = Rational::new(1, 1000);
        assert_eq!(pts_to_duration(-1, tb), Duration::ZERO);
    }

    #[test]
    fn test_duration_conversion() {
        // Test duration calculation logic
        let duration_us: i64 = 5_500_000; // 5.5 seconds
        let secs = (duration_us / AV_TIME_BASE) as u64;
        let micros = (duration_us % AV_TIME_BASE) as u32;
        let duration = Duration::new(secs, micros * 1000);

        assert_eq!(duration.as_secs(), 5);
        assert_eq!(duration.subsec_micros(), 500_000);
    }

    // ========================================================================
    // Video Codec Mapping Tests
    // ========================================================================

    #[test]
    fn test_map_video_codec_h264() {
        let codec = map_video_codec(ff_sys::AVCodecID_AV_CODEC_ID_H264);
        assert_eq!(codec, VideoCodec::H264);
    }

    #[test]
    fn test_map_video_codec_hevc() {
        let codec = map_video_codec(ff_sys::AVCodecID_AV_CODEC_ID_HEVC);
        assert_eq!(codec, VideoCodec::H265);
    }

    #[test]
    fn test_map_video_codec_vp9() {
        let codec = map_video_codec(ff_sys::AVCodecID_AV_CODEC_ID_VP9);
        assert_eq!(codec, VideoCodec::Vp9);
    }

    #[test]
    fn test_map_video_codec_av1() {
        let codec = map_video_codec(ff_sys::AVCodecID_AV_CODEC_ID_AV1);
        assert_eq!(codec, VideoCodec::Av1);
    }

    #[test]
    fn test_map_video_codec_unknown() {
        // Use a codec ID that's not explicitly mapped
        let codec = map_video_codec(ff_sys::AVCodecID_AV_CODEC_ID_THEORA);
        assert_eq!(codec, VideoCodec::Unknown);
    }

    // ========================================================================
    // Pixel Format Mapping Tests
    // ========================================================================

    #[test]
    fn test_map_pixel_format_yuv420p() {
        let format = map_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P as i32);
        assert_eq!(format, PixelFormat::Yuv420p);
    }

    #[test]
    fn test_map_pixel_format_rgba() {
        let format = map_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA as i32);
        assert_eq!(format, PixelFormat::Rgba);
    }

    #[test]
    fn test_map_pixel_format_nv12() {
        let format = map_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_NV12 as i32);
        assert_eq!(format, PixelFormat::Nv12);
    }

    #[test]
    fn test_map_pixel_format_yuv420p10le() {
        let format = map_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE as i32);
        assert_eq!(format, PixelFormat::Yuv420p10le);
    }

    #[test]
    fn test_map_pixel_format_unknown() {
        // Use a pixel format that's not explicitly mapped
        let format = map_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_PAL8 as i32);
        assert!(matches!(format, PixelFormat::Other(_)));
    }

    // ========================================================================
    // Color Space Mapping Tests
    // ========================================================================

    #[test]
    fn test_map_color_space_bt709() {
        let space = map_color_space(ff_sys::AVColorSpace_AVCOL_SPC_BT709);
        assert_eq!(space, ColorSpace::Bt709);
    }

    #[test]
    fn test_map_color_space_sd() {
        let space = map_color_space(ff_sys::AVColorSpace_AVCOL_SPC_BT470BG);
        assert_eq!(space, ColorSpace::Bt470bg);

        let space = map_color_space(ff_sys::AVColorSpace_AVCOL_SPC_SMPTE170M);
        assert_eq!(space, ColorSpace::Smpte170m);
    }

    #[test]
    fn test_map_color_space_bt2020() {
        let space = map_color_space(ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL);
        assert_eq!(space, ColorSpace::Bt2020Ncl);

        let space = map_color_space(ff_sys::AVColorSpace_AVCOL_SPC_BT2020_CL);
        assert_eq!(space, ColorSpace::Bt2020Cl);
    }

    #[test]
    fn test_map_color_space_rgb() {
        let space = map_color_space(ff_sys::AVColorSpace_AVCOL_SPC_RGB);
        assert_eq!(space, ColorSpace::Rgb);
    }

    #[test]
    fn test_map_color_space_unknown() {
        let space = map_color_space(ff_sys::AVColorSpace_AVCOL_SPC_UNSPECIFIED);
        assert_eq!(space, ColorSpace::Unknown);
    }

    // ========================================================================
    // Color Range Mapping Tests
    // ========================================================================

    #[test]
    fn test_map_color_range_limited() {
        let range = map_color_range(ff_sys::AVColorRange_AVCOL_RANGE_MPEG);
        assert_eq!(range, ColorRange::Limited);
    }

    #[test]
    fn test_map_color_range_full() {
        let range = map_color_range(ff_sys::AVColorRange_AVCOL_RANGE_JPEG);
        assert_eq!(range, ColorRange::Full);
    }

    #[test]
    fn test_map_color_range_unknown() {
        let range = map_color_range(ff_sys::AVColorRange_AVCOL_RANGE_UNSPECIFIED);
        assert_eq!(range, ColorRange::Unknown);
    }

    // ========================================================================
    // Color Primaries Mapping Tests
    // ========================================================================

    #[test]
    fn test_map_color_primaries_bt709() {
        let primaries = map_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT709);
        assert_eq!(primaries, ColorPrimaries::Bt709);
    }

    #[test]
    fn test_map_color_primaries_sd() {
        let primaries = map_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT470BG);
        assert_eq!(primaries, ColorPrimaries::Bt470bg);

        let primaries = map_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE170M);
        assert_eq!(primaries, ColorPrimaries::Smpte170m);
    }

    #[test]
    fn test_map_color_primaries_bt2020() {
        let primaries = map_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020);
        assert_eq!(primaries, ColorPrimaries::Bt2020);
    }

    #[test]
    fn test_map_color_primaries_unknown() {
        let primaries = map_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_UNSPECIFIED);
        assert_eq!(primaries, ColorPrimaries::Unknown);
    }

    // ========================================================================
    // Audio Codec Mapping Tests
    // ========================================================================

    #[test]
    fn test_map_audio_codec_aac() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_AAC);
        assert_eq!(codec, AudioCodec::Aac);
    }

    #[test]
    fn test_map_audio_codec_mp3() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_MP3);
        assert_eq!(codec, AudioCodec::Mp3);
    }

    #[test]
    fn test_map_audio_codec_opus() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_OPUS);
        assert_eq!(codec, AudioCodec::Opus);
    }

    #[test]
    fn test_map_audio_codec_flac() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_FLAC);
        assert_eq!(codec, AudioCodec::Flac);
    }

    #[test]
    fn test_map_audio_codec_vorbis() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_VORBIS);
        assert_eq!(codec, AudioCodec::Vorbis);
    }

    #[test]
    fn test_map_audio_codec_ac3() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_AC3);
        assert_eq!(codec, AudioCodec::Ac3);
    }

    #[test]
    fn test_map_audio_codec_eac3() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_EAC3);
        assert_eq!(codec, AudioCodec::Eac3);
    }

    #[test]
    fn test_map_audio_codec_dts() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_DTS);
        assert_eq!(codec, AudioCodec::Dts);
    }

    #[test]
    fn test_map_audio_codec_alac() {
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_ALAC);
        assert_eq!(codec, AudioCodec::Alac);
    }

    #[test]
    fn test_map_audio_codec_pcm() {
        // Test various PCM formats
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_PCM_S16LE);
        assert_eq!(codec, AudioCodec::Pcm);

        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_PCM_F32LE);
        assert_eq!(codec, AudioCodec::Pcm);

        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_PCM_U8);
        assert_eq!(codec, AudioCodec::Pcm);
    }

    #[test]
    fn test_map_audio_codec_unknown() {
        // Use a codec ID that's not explicitly mapped
        let codec = map_audio_codec(ff_sys::AVCodecID_AV_CODEC_ID_WMAV2);
        assert_eq!(codec, AudioCodec::Unknown);
    }

    // ========================================================================
    // Sample Format Mapping Tests
    // ========================================================================

    #[test]
    fn test_map_sample_format_u8() {
        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8 as i32);
        assert_eq!(format, SampleFormat::U8);
    }

    #[test]
    fn test_map_sample_format_i16() {
        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16 as i32);
        assert_eq!(format, SampleFormat::I16);
    }

    #[test]
    fn test_map_sample_format_i32() {
        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32 as i32);
        assert_eq!(format, SampleFormat::I32);
    }

    #[test]
    fn test_map_sample_format_f32() {
        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT as i32);
        assert_eq!(format, SampleFormat::F32);
    }

    #[test]
    fn test_map_sample_format_f64() {
        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBL as i32);
        assert_eq!(format, SampleFormat::F64);
    }

    #[test]
    fn test_map_sample_format_planar() {
        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8P as i32);
        assert_eq!(format, SampleFormat::U8p);

        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16P as i32);
        assert_eq!(format, SampleFormat::I16p);

        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32P as i32);
        assert_eq!(format, SampleFormat::I32p);

        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLTP as i32);
        assert_eq!(format, SampleFormat::F32p);

        let format = map_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBLP as i32);
        assert_eq!(format, SampleFormat::F64p);
    }

    #[test]
    fn test_map_sample_format_unknown() {
        // Use a format value that's not explicitly mapped
        let format = map_sample_format(999);
        assert!(matches!(format, SampleFormat::Other(_)));
    }

    // ========================================================================
    // Bitrate Calculation Tests
    // ========================================================================

    #[test]
    fn test_bitrate_fallback_calculation() {
        // Test the fallback bitrate calculation logic:
        // bitrate = file_size (bytes) * 8 (bits/byte) / duration (seconds)
        //
        // Example: 10 MB file, 10 second duration
        // Expected: 10_000_000 bytes * 8 / 10 seconds = 8_000_000 bps
        let file_size: u64 = 10_000_000;
        let duration = Duration::from_secs(10);
        let duration_secs = duration.as_secs_f64();

        let calculated_bitrate = (file_size as f64 * 8.0 / duration_secs) as u64;
        assert_eq!(calculated_bitrate, 8_000_000);
    }

    #[test]
    fn test_bitrate_fallback_with_subsecond_duration() {
        // Test with sub-second duration
        // 1 MB file, 0.5 second duration
        // Expected: 1_000_000 * 8 / 0.5 = 16_000_000 bps
        let file_size: u64 = 1_000_000;
        let duration = Duration::from_millis(500);
        let duration_secs = duration.as_secs_f64();

        let calculated_bitrate = (file_size as f64 * 8.0 / duration_secs) as u64;
        assert_eq!(calculated_bitrate, 16_000_000);
    }

    #[test]
    fn test_bitrate_zero_duration() {
        // When duration is zero, we cannot calculate bitrate
        let duration = Duration::ZERO;
        let duration_secs = duration.as_secs_f64();

        // Should not divide when duration is zero
        assert!(duration_secs == 0.0);
    }

    #[test]
    fn test_bitrate_zero_file_size() {
        // When file size is zero, bitrate should also be zero
        let file_size: u64 = 0;
        let duration = Duration::from_secs(10);
        let duration_secs = duration.as_secs_f64();

        if duration_secs > 0.0 && file_size > 0 {
            let calculated_bitrate = (file_size as f64 * 8.0 / duration_secs) as u64;
            assert_eq!(calculated_bitrate, 0);
        } else {
            // file_size is 0, so we should not have calculated a bitrate
            assert_eq!(file_size, 0);
        }
    }

    #[test]
    fn test_bitrate_typical_video_file() {
        // Test with typical video file parameters:
        // 100 MB file, 5 minute duration
        // Expected: 100_000_000 * 8 / 300 = 2_666_666 bps (~2.67 Mbps)
        let file_size: u64 = 100_000_000;
        let duration = Duration::from_secs(300); // 5 minutes
        let duration_secs = duration.as_secs_f64();

        let calculated_bitrate = (file_size as f64 * 8.0 / duration_secs) as u64;
        assert_eq!(calculated_bitrate, 2_666_666);
    }

    #[test]
    fn test_bitrate_high_quality_video() {
        // Test with high-quality video parameters:
        // 5 GB file, 2 hour duration
        // Expected: 5_000_000_000 * 8 / 7200 = 5_555_555 bps (~5.6 Mbps)
        let file_size: u64 = 5_000_000_000;
        let duration = Duration::from_secs(7200); // 2 hours
        let duration_secs = duration.as_secs_f64();

        let calculated_bitrate = (file_size as f64 * 8.0 / duration_secs) as u64;
        assert_eq!(calculated_bitrate, 5_555_555);
    }
}
