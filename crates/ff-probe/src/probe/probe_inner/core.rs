//! Core format-level extraction: format name, duration, and container bitrate.

use std::time::Duration;

/// `AV_TIME_BASE` constant from `FFmpeg` (microseconds per second).
const AV_TIME_BASE: i64 = 1_000_000;

/// Extracts format information from the demux context.
pub(super) fn extract_format_info(
    ctx: &ff_sys::InputFormatContext,
) -> (String, Option<String>, Duration) {
    let format = ctx
        .iformat_name()
        .unwrap_or_else(|| String::from("unknown"));
    let format_long_name = ctx.iformat_long_name();
    let duration = extract_duration(ctx.duration());

    (format, format_long_name, duration)
}

/// Converts a container duration in `AV_TIME_BASE` units (microseconds) to a
/// [`Duration`], returning `Duration::ZERO` when it is unavailable or invalid.
fn extract_duration(duration_us: i64) -> Duration {
    // duration_us == 0: Container does not provide duration info (e.g., live streams)
    // duration_us < 0: AV_NOPTS_VALUE (typically i64::MIN), indicating unknown duration
    if duration_us <= 0 {
        return Duration::ZERO;
    }

    // Convert from microseconds to Duration
    // duration is in AV_TIME_BASE units (1/1000000 seconds)
    // Safe cast: we verified duration_us > 0 above
    #[expect(clippy::cast_sign_loss, reason = "verified duration_us > 0")]
    let secs = (duration_us / AV_TIME_BASE) as u64;
    #[expect(clippy::cast_sign_loss, reason = "verified duration_us > 0")]
    let micros = (duration_us % AV_TIME_BASE) as u32;

    Duration::new(secs, micros * 1000)
}

/// Calculates the overall bitrate for a media file.
///
/// This function first tries to get the bitrate directly from the `AVFormatContext`.
/// If the bitrate is not available (i.e., 0 or negative), it falls back to calculating
/// the bitrate from the file size and duration: `bitrate = file_size * 8 / duration`.
///
/// # Arguments
///
/// * `ctx` - The demux context to extract bitrate from
/// * `file_size` - The file size in bytes
/// * `duration` - The duration of the media
///
/// # Returns
///
/// Returns `Some(bitrate)` in bits per second, or `None` if neither method can determine
/// the bitrate (e.g., if duration is zero).
pub(super) fn calculate_container_bitrate(
    ctx: &ff_sys::InputFormatContext,
    file_size: u64,
    duration: std::time::Duration,
) -> Option<u64> {
    let bitrate = ctx.bit_rate();

    // If bitrate is available from FFmpeg, use it directly
    if bitrate > 0 {
        #[expect(clippy::cast_sign_loss, reason = "verified bitrate > 0")]
        return Some(bitrate as u64);
    }

    // Fallback: calculate from file size and duration
    // bitrate (bps) = file_size (bytes) * 8 (bits/byte) / duration (seconds)
    let duration_secs = duration.as_secs_f64();
    if duration_secs > 0.0 && file_size > 0 {
        // Note: Precision loss from u64->f64 is acceptable here because:
        // 1. For files up to 9 PB, f64 provides sufficient precision
        // 2. The result is used for display/metadata purposes, not exact calculations
        #[expect(
            clippy::cast_precision_loss,
            reason = "precision loss acceptable for file size; f64 handles up to 9 PB"
        )]
        let file_size_f64 = file_size as f64;

        #[expect(
            clippy::cast_possible_truncation,
            reason = "bitrate values are bounded by practical file sizes"
        )]
        #[expect(
            clippy::cast_sign_loss,
            reason = "result is always positive since both operands are positive"
        )]
        let calculated_bitrate = (file_size_f64 * 8.0 / duration_secs) as u64;
        Some(calculated_bitrate)
    } else {
        None
    }
}
