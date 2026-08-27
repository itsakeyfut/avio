//! Video/audio stream helpers: encoder selection, AAC encoder open, FPS detection.

use ff_sys::AVRational;

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
    enc.open_codec(codec).map_err(|e| ffmpeg_err(e.code()))?;

    log::info!("dash aac encoder opened sample_rate={sample_rate} channels={nb_channels}");
    Ok(enc)
}

// ============================================================================
// FPS detection
// ============================================================================

#[allow(clippy::cast_precision_loss)]
pub(super) fn detect_fps(avg: AVRational, rfr: AVRational, nb_frames: i64, duration: i64) -> f64 {
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
    if let Some(fps) = try_rational(avg.num, avg.den) {
        return fps;
    }

    // 2. r_frame_rate — constant-framerate indicator
    if let Some(fps) = try_rational(rfr.num, rfr.den) {
        return fps;
    }

    // 3. Derive from nb_frames and total duration (robust for MPEG-4 Part 2)
    // `duration` is in AV_TIME_BASE (1 000 000) microseconds.
    if nb_frames > 0 && duration > 0 {
        let fps = nb_frames as f64 / (duration as f64 / 1_000_000.0);
        if (MIN_FPS..=MAX_FPS).contains(&fps) {
            return fps;
        }
    }

    25.0 // sane default
}

#[cfg(test)]
mod tests {
    use super::detect_fps;
    use ff_sys::AVRational;

    fn r(num: i32, den: i32) -> AVRational {
        AVRational { num, den }
    }

    #[test]
    fn detect_fps_should_prefer_avg_frame_rate() {
        assert_eq!(detect_fps(r(30, 1), r(60, 1), 0, 0), 30.0);
    }

    #[test]
    fn detect_fps_should_fall_back_to_r_frame_rate_when_avg_out_of_range() {
        // avg = 1_250_000/49 ≈ 25510 fps is outside [1, 240] and is rejected.
        assert_eq!(detect_fps(r(1_250_000, 49), r(24, 1), 0, 0), 24.0);
    }

    #[test]
    fn detect_fps_should_derive_from_nb_frames_and_duration() {
        // 300 frames over 10 s (10_000_000 µs) = 30 fps, distinct from the 25.0 default.
        assert_eq!(detect_fps(r(0, 0), r(0, 0), 300, 10_000_000), 30.0);
    }

    #[test]
    fn detect_fps_should_default_to_25_when_all_sources_unknown() {
        assert_eq!(detect_fps(r(0, 0), r(0, 0), 0, 0), 25.0);
    }
}
