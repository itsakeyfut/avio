//! Resolving how long a single stream runs.

use std::time::Duration;

/// How long `stream` runs, reading the stream's own duration and falling back to the
/// container's.
///
/// `container_micros` is `AVFormatContext.duration` in `AV_TIME_BASE` units, which is
/// what [`InputFormatContext::duration`](ff_sys::InputFormatContext::duration) returns.
///
/// # Why the stream's own value is the reading
///
/// A container's duration is its **longest stream**, so for a file whose audio outruns
/// its video it says more than the video holds. That is routine: an AAC encoder emits
/// whole 1024-sample frames, so 15 video frames at 30 fps (0.5s) sit beside 24 audio
/// packets (0.512s) and the container reports 0.512s. Reporting that as the video's
/// duration made a composition's length depend on how long the audio in a source file
/// happened to be (#1861).
///
/// The container's duration remains the right answer for the *file*, which is what
/// `ff-probe` reports through `MediaInfo::duration`. It is the wrong answer for one
/// stream in it.
///
/// # The fallback
///
/// `AVStream.duration` is `AV_NOPTS_VALUE` for a stream whose length the demuxer could
/// not establish, and Matroska in particular leaves it unset. The container's duration
/// is then the only reading available, so it is used, and `None` is returned only when
/// neither is usable. This keeps a file that reported a length before reporting one now.
///
/// The fallback cannot catch a value that is present but wrong, which MPEG-TS and raw
/// streams are known for: there the demuxer may fill the field from a partial scan. The
/// reading is trusted whenever it is positive, so such a file now reports the stream's
/// estimate where it used to report the container's.
pub(crate) fn stream_duration(
    stream: ff_sys::StreamRef<'_>,
    container_micros: i64,
) -> Option<Duration> {
    from_stream(stream).or_else(|| from_micros(container_micros))
}

/// The stream's own duration, converted out of its time base.
///
/// `AV_NOPTS_VALUE` is `i64::MIN`, so a positive test rejects it along with `0` (which
/// carries no more information than "unknown" here).
// A tick count reaches `f64`'s 53-bit mantissa only past 285 years at microsecond
// resolution, so the cast cannot lose anything a media file carries.
#[allow(clippy::cast_precision_loss)]
fn from_stream(stream: ff_sys::StreamRef<'_>) -> Option<Duration> {
    let ticks = stream.duration();
    if ticks <= 0 {
        return None;
    }
    let tb = stream.time_base();
    if tb.num <= 0 || tb.den <= 0 {
        return None;
    }
    seconds_to_duration(ticks as f64 * f64::from(tb.num) / f64::from(tb.den))
}

/// The container's duration, converted out of `AV_TIME_BASE` (microseconds).
// Same bound as `from_stream`: `AV_TIME_BASE` is microseconds, so the mantissa holds
// every duration short of geological.
#[allow(clippy::cast_precision_loss)]
fn from_micros(micros: i64) -> Option<Duration> {
    if micros <= 0 {
        return None;
    }
    seconds_to_duration(micros as f64 / 1_000_000.0)
}

/// `Duration::try_from_secs_f64`, so a reading that cannot be represented reports no
/// duration instead of aborting the process the way the panicking constructor does
/// (ADR-0017).
fn seconds_to_duration(seconds: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_micros_should_convert_av_time_base_units() {
        assert_eq!(from_micros(512_000), Some(Duration::from_millis(512)));
    }

    /// The container is only consulted when the stream has nothing, so a zero or absent
    /// container duration has to stay absent rather than become `Duration::ZERO`.
    #[test]
    fn from_micros_should_reject_a_non_positive_reading() {
        assert_eq!(from_micros(0), None);
        assert_eq!(from_micros(-1), None);
        assert_eq!(from_micros(i64::MIN), None);
    }

    #[test]
    fn seconds_to_duration_should_reject_what_a_duration_cannot_hold() {
        assert_eq!(seconds_to_duration(0.5), Some(Duration::from_millis(500)));
        assert_eq!(seconds_to_duration(f64::NAN), None);
        assert_eq!(seconds_to_duration(f64::INFINITY), None);
        assert_eq!(seconds_to_duration(-1.0), None);
    }
}
