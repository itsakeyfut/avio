//! Video stream extraction.

use std::ffi::CStr;
use std::time::Duration;

use ff_format::Rational;
use ff_format::stream::VideoStreamInfo;

use super::mapping::{
    map_color_primaries, map_color_range, map_color_space, map_pixel_format, map_video_codec,
};

/// Extracts all video streams from the demux context.
///
/// Iterates every stream and extracts detailed information for each video stream.
pub(super) fn extract_video_streams(ctx: &ff_sys::InputFormatContext) -> Vec<VideoStreamInfo> {
    ctx.streams()
        .filter(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO)
        .map(extract_single_video_stream)
        .collect()
}

/// Extracts information from a single video stream handle.
fn extract_single_video_stream(stream: ff_sys::StreamRef<'_>) -> VideoStreamInfo {
    let codecpar = stream.codecpar();

    // Extract codec info
    let codec_id = codecpar.codec_id();
    let codec = map_video_codec(codec_id);
    let codec_name = extract_codec_name(codec_id);

    // Extract dimensions
    #[expect(clippy::cast_sign_loss, reason = "width/height are always positive")]
    let width = codecpar.width() as u32;
    #[expect(clippy::cast_sign_loss, reason = "width/height are always positive")]
    let height = codecpar.height() as u32;

    // Extract pixel format
    let pixel_format = map_pixel_format(codecpar.format());

    // Extract frame rate
    let frame_rate = extract_frame_rate(stream);

    // Extract bitrate
    let bitrate = extract_stream_bitrate(codecpar);

    // Extract color information
    let color_space = map_color_space(codecpar.color_space());
    let color_range = map_color_range(codecpar.color_range());
    let color_primaries = map_color_primaries(codecpar.color_primaries());

    // Extract duration if available
    let duration = extract_stream_duration(stream);

    // Extract frame count if available
    let frame_count = extract_frame_count(stream);

    // Build the VideoStreamInfo
    #[expect(clippy::cast_sign_loss, reason = "stream index is always non-negative")]
    let index = stream.index() as u32;
    let mut builder = VideoStreamInfo::builder()
        .index(index)
        .codec(codec)
        .codec_name(codec_name)
        .width(width)
        .height(height)
        .pixel_format(pixel_format)
        .frame_rate(frame_rate)
        .color_space(color_space)
        .color_range(color_range)
        .color_primaries(color_primaries);

    if let Some(d) = duration {
        builder = builder.duration(d);
    }

    if let Some(b) = bitrate {
        builder = builder.bitrate(b);
    }

    if let Some(c) = frame_count {
        builder = builder.frame_count(c);
    }

    builder.build()
}

/// Extracts the codec name from an `AVCodecID`.
pub(super) fn extract_codec_name(codec_id: ff_sys::AVCodecID) -> String {
    // SAFETY: avcodec_get_name is safe for any codec ID value and returns a valid
    //         C string (or null, which we guard).
    let name_ptr = unsafe { ff_sys::avcodec_get_name(codec_id) };

    if name_ptr.is_null() {
        return String::from("unknown");
    }

    // SAFETY: avcodec_get_name returns a valid, NUL-terminated C string.
    unsafe { CStr::from_ptr(name_ptr).to_string_lossy().into_owned() }
}

/// Extracts the frame rate from a stream handle.
///
/// Tries the real frame rate (`r_frame_rate`), falling back to the average frame
/// rate (`avg_frame_rate`), and finally to a default of 30/1.
fn extract_frame_rate(stream: ff_sys::StreamRef<'_>) -> Rational {
    // Try r_frame_rate first (real frame rate, most accurate for video)
    let r_frame_rate = stream.r_frame_rate();
    if r_frame_rate.den > 0 && r_frame_rate.num > 0 {
        return Rational::new(r_frame_rate.num, r_frame_rate.den);
    }

    // Fall back to avg_frame_rate
    let avg_frame_rate = stream.avg_frame_rate();
    if avg_frame_rate.den > 0 && avg_frame_rate.num > 0 {
        return Rational::new(avg_frame_rate.num, avg_frame_rate.den);
    }

    // Default to 30 fps
    log::warn!(
        "frame_rate unavailable, falling back to 30fps \
         r_frame_rate={}/{} avg_frame_rate={}/{} fallback=30/1",
        r_frame_rate.num,
        r_frame_rate.den,
        avg_frame_rate.num,
        avg_frame_rate.den
    );
    Rational::new(30, 1)
}

/// Extracts the bitrate from codec parameters.
///
/// Returns `None` if the bitrate is not available or is zero.
pub(super) fn extract_stream_bitrate(codecpar: ff_sys::CodecParameters<'_>) -> Option<u64> {
    let bitrate = codecpar.bit_rate();

    if bitrate > 0 {
        #[expect(clippy::cast_sign_loss, reason = "verified bitrate > 0")]
        Some(bitrate as u64)
    } else {
        None
    }
}

/// Extracts the duration from a stream handle.
///
/// Returns `None` if the duration is not available.
pub(super) fn extract_stream_duration(stream: ff_sys::StreamRef<'_>) -> Option<Duration> {
    let duration_pts = stream.duration();

    // AV_NOPTS_VALUE indicates unknown duration
    if duration_pts <= 0 {
        return None;
    }

    // Get stream time base
    let time_base = stream.time_base();
    if time_base.den == 0 {
        return None;
    }

    // Convert to seconds: pts * num / den
    // Note: i64 to f64 cast may lose precision for very large values,
    // but this is acceptable for media timestamps which are bounded
    #[expect(clippy::cast_precision_loss, reason = "media timestamps are bounded")]
    let secs = (duration_pts as f64) * f64::from(time_base.num) / f64::from(time_base.den);

    if secs > 0.0 {
        Some(Duration::from_secs_f64(secs))
    } else {
        None
    }
}

/// Extracts the frame count from a stream handle.
///
/// Returns `None` if the frame count is not available.
fn extract_frame_count(stream: ff_sys::StreamRef<'_>) -> Option<u64> {
    let nb_frames = stream.nb_frames();

    if nb_frames > 0 {
        #[expect(clippy::cast_sign_loss, reason = "verified nb_frames > 0")]
        Some(nb_frames as u64)
    } else {
        None
    }
}
