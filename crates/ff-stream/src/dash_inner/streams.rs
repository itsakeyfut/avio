//! Video/audio stream helpers: encoder selection, AAC encoder open, FPS detection.

use ff_sys::AVFormatContext;

use crate::error::StreamError;

use super::ffmpeg_err;
use super::ffmpeg_err_msg;

// ============================================================================
// Helper: select best available H.264 encoder
// ============================================================================

pub(super) unsafe fn select_h264_encoder() -> Option<ff_sys::Codec> {
    let candidates = [
        "h264_nvenc",
        "h264_qsv",
        "h264_amf",
        "h264_videotoolbox",
        "libx264",
        "mpeg4",
    ];
    for name in candidates {
        if let Some(codec) = ff_sys::Codec::find_encoder_by_name(name) {
            log::info!("dash selected video encoder encoder={name}");
            return Some(codec);
        }
    }
    None
}

// ============================================================================
// Helper: open AAC encoder
// ============================================================================

pub(super) unsafe fn open_aac_encoder(
    sample_rate: i32,
    nb_channels: i32,
) -> Result<ff_sys::CodecContext, StreamError> {
    let codec = ff_sys::Codec::find_encoder_by_name("aac")
        .or_else(|| ff_sys::Codec::find_encoder_by_name("libfdk_aac"))
        .ok_or_else(|| ffmpeg_err_msg("no AAC encoder available"))?;

    let mut enc = ff_sys::CodecContext::new(Some(codec)).map_err(|e| ffmpeg_err(e.code()))?;

    enc.set_sample_rate(sample_rate);
    enc.set_sample_fmt(ff_sys::swresample::sample_format::FLTP);
    enc.set_bit_rate(192_000);
    enc.set_time_base(ff_sys::AVRational {
        num: 1,
        den: sample_rate,
    });
    enc.set_ch_layout_default(nb_channels);

    // On open failure `enc` drops (Drop = avcodec_free_context), so no manual free.
    enc.open(codec, std::ptr::null_mut())
        .map_err(|e| ffmpeg_err(e.code()))?;

    log::info!("dash aac encoder opened sample_rate={sample_rate} channels={nb_channels}");
    Ok(enc)
}

// ============================================================================
// FPS detection
// ============================================================================

#[allow(clippy::cast_precision_loss)]
pub(super) unsafe fn detect_fps(
    stream: *mut ff_sys::AVStream,
    fmt_ctx: *mut AVFormatContext,
) -> f64 {
    const MIN_FPS: f64 = 1.0;
    const MAX_FPS: f64 = 240.0;

    let try_rational = |num: i32, den: i32| -> Option<f64> {
        if den <= 0 || num <= 0 {
            return None;
        }
        let fps = num as f64 / den as f64;
        if (MIN_FPS..=MAX_FPS).contains(&fps) {
            Some(fps)
        } else {
            None
        }
    };

    // 1. avg_frame_rate — reliable for most containers
    let avg = (*stream).avg_frame_rate;
    if let Some(fps) = try_rational(avg.num, avg.den) {
        return fps;
    }

    // 2. r_frame_rate — constant-framerate indicator
    let rfr = (*stream).r_frame_rate;
    if let Some(fps) = try_rational(rfr.num, rfr.den) {
        return fps;
    }

    // 3. Derive from nb_frames and total duration (robust for MPEG-4 Part 2)
    let nb = (*stream).nb_frames;
    let dur = (*fmt_ctx).duration; // in AV_TIME_BASE (1 000 000) microseconds
    if nb > 0 && dur > 0 {
        let fps = nb as f64 / (dur as f64 / 1_000_000.0);
        if (MIN_FPS..=MAX_FPS).contains(&fps) {
            return fps;
        }
    }

    25.0 // sane default
}
