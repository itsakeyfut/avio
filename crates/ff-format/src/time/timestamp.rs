//! [`Timestamp`] type for representing media timestamps.

// These casts are intentional for media timestamp arithmetic.
// The values involved (PTS, time bases, frame rates) are well within
// the safe ranges for these conversions in practical video/audio scenarios.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Sub};
use std::time::Duration;

use super::Rational;

/// A timestamp representing a point in time within a media stream.
///
/// Timestamps are represented as a presentation timestamp (PTS) value
/// combined with a time base that defines the unit of measurement.
///
/// # Time Base
///
/// The time base is a rational number that represents the duration of
/// one timestamp unit. For example:
/// - `1/90000`: Each PTS unit is 1/90000 of a second (MPEG-TS)
/// - `1/1000`: Each PTS unit is 1 millisecond
/// - `1/48000`: Each PTS unit is one audio sample at 48kHz
///
/// # Examples
///
/// ```
/// use ff_format::{Rational, Timestamp};
/// use std::time::Duration;
///
/// // Create a timestamp at 1 second using 90kHz time base
/// let time_base = Rational::new(1, 90000);
/// let ts = Timestamp::new(90000, time_base);
///
/// assert!((ts.as_secs_f64() - 1.0).abs() < 0.0001);
/// assert_eq!(ts.as_millis(), 1000);
///
/// // Convert from Duration
/// let ts2 = Timestamp::from_duration(Duration::from_secs(1), time_base);
/// assert_eq!(ts2.pts(), 90000);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Timestamp {
    pts: i64,
    time_base: Rational,
}

impl Timestamp {
    /// Creates a new timestamp with the given PTS value and time base.
    ///
    /// # Arguments
    ///
    /// * `pts` - The presentation timestamp value
    /// * `time_base` - The time base (duration of one PTS unit)
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let time_base = Rational::new(1, 1000);  // milliseconds
    /// let ts = Timestamp::new(500, time_base);  // 500ms
    /// assert_eq!(ts.as_millis(), 500);
    /// ```
    #[must_use]
    pub const fn new(pts: i64, time_base: Rational) -> Self {
        Self { pts, time_base }
    }

    /// Creates a timestamp representing zero (0 PTS).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let time_base = Rational::new(1, 90000);
    /// let zero = Timestamp::zero(time_base);
    /// assert_eq!(zero.pts(), 0);
    /// assert_eq!(zero.as_secs_f64(), 0.0);
    /// ```
    #[must_use]
    pub const fn zero(time_base: Rational) -> Self {
        Self { pts: 0, time_base }
    }

    /// Creates a timestamp from a Duration value.
    ///
    /// # Arguments
    ///
    /// * `duration` - The duration to convert
    /// * `time_base` - The target time base for the resulting timestamp
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    /// use std::time::Duration;
    ///
    /// let time_base = Rational::new(1, 90000);
    /// let ts = Timestamp::from_duration(Duration::from_millis(1000), time_base);
    /// assert_eq!(ts.pts(), 90000);
    /// ```
    #[must_use]
    pub fn from_duration(duration: Duration, time_base: Rational) -> Self {
        let secs = duration.as_secs_f64();
        let pts = (secs / time_base.as_f64()).round() as i64;
        Self { pts, time_base }
    }

    /// Creates a timestamp from a seconds value.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let time_base = Rational::new(1, 1000);
    /// let ts = Timestamp::from_secs_f64(1.5, time_base);
    /// assert_eq!(ts.pts(), 1500);
    /// ```
    #[must_use]
    pub fn from_secs_f64(secs: f64, time_base: Rational) -> Self {
        let pts = (secs / time_base.as_f64()).round() as i64;
        Self { pts, time_base }
    }

    /// Creates a timestamp from milliseconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let time_base = Rational::new(1, 90000);
    /// let ts = Timestamp::from_millis(1000, time_base);
    /// assert_eq!(ts.pts(), 90000);
    /// ```
    #[must_use]
    pub fn from_millis(millis: i64, time_base: Rational) -> Self {
        let secs = millis as f64 / 1000.0;
        Self::from_secs_f64(secs, time_base)
    }

    /// Returns the presentation timestamp value.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let ts = Timestamp::new(12345, Rational::new(1, 90000));
    /// assert_eq!(ts.pts(), 12345);
    /// ```
    #[must_use]
    #[inline]
    pub const fn pts(&self) -> i64 {
        self.pts
    }

    /// Returns the time base.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let time_base = Rational::new(1, 90000);
    /// let ts = Timestamp::new(100, time_base);
    /// assert_eq!(ts.time_base(), time_base);
    /// ```
    #[must_use]
    #[inline]
    pub const fn time_base(&self) -> Rational {
        self.time_base
    }

    /// Converts the timestamp to a Duration.
    ///
    /// A timestamp `Duration` cannot represent is clamped to zero. That covers a
    /// negative value, which a codec delay such as Opus's pre-skip or AAC's
    /// priming expresses normally, a non-finite one, which [`Rational::as_f64`]
    /// yields for a zero denominator, and one too large for `Duration`.
    ///
    /// The negative case is silent, being ordinary media on a path that runs once
    /// per frame; the other two log a warning, because they mean the container's
    /// own time base is unusable.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    /// use std::time::Duration;
    ///
    /// let ts = Timestamp::new(90000, Rational::new(1, 90000));
    /// let duration = ts.as_duration();
    /// assert_eq!(duration, Duration::from_secs(1));
    /// ```
    #[must_use]
    pub fn as_duration(&self) -> Duration {
        let secs = self.as_secs_f64();
        // One fallible conversion covers every value `Duration` rejects:
        // negative, non-finite and overflowing. A hand-written sign test does
        // not, because `secs < 0.0` is false for both `NaN` and `+inf`.
        if let Ok(duration) = Duration::try_from_secs_f64(secs) {
            return duration;
        }
        // Decoders call this once per frame, so a negative value stays silent:
        // it is a codec delay such as Opus's pre-skip, which is ordinary media
        // (ADR-0017) and not worth a line. A non-finite or overflowing value
        // means the container's own time base is unusable, which is.
        if !secs.is_finite() || secs > 0.0 {
            log::warn!(
                "timestamp is not representable as a duration, clamping to zero \
                 secs={secs} pts={pts} time_base={num}/{den} fallback=Duration::ZERO",
                pts = self.pts,
                num = self.time_base.num(),
                den = self.time_base.den()
            );
        }
        Duration::ZERO
    }

    /// Converts the timestamp to seconds as a floating-point value.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let ts = Timestamp::new(45000, Rational::new(1, 90000));
    /// assert!((ts.as_secs_f64() - 0.5).abs() < 0.0001);
    /// ```
    #[must_use]
    #[inline]
    pub fn as_secs_f64(&self) -> f64 {
        self.pts as f64 * self.time_base.as_f64()
    }

    /// Converts the timestamp to milliseconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let ts = Timestamp::new(90000, Rational::new(1, 90000));
    /// assert_eq!(ts.as_millis(), 1000);
    /// ```
    #[must_use]
    #[inline]
    pub fn as_millis(&self) -> i64 {
        (self.as_secs_f64() * 1000.0).round() as i64
    }

    /// Converts the timestamp to microseconds.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let ts = Timestamp::new(90, Rational::new(1, 90000));
    /// assert_eq!(ts.as_micros(), 1000);  // 90/90000 = 0.001 sec = 1000 microseconds
    /// ```
    #[must_use]
    #[inline]
    pub fn as_micros(&self) -> i64 {
        (self.as_secs_f64() * 1_000_000.0).round() as i64
    }

    /// Converts the timestamp to a frame number at the given frame rate.
    ///
    /// # Arguments
    ///
    /// * `fps` - The frame rate (frames per second)
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let ts = Timestamp::new(90000, Rational::new(1, 90000));  // 1 second
    /// assert_eq!(ts.as_frame_number(30.0), 30);  // 30 fps
    /// assert_eq!(ts.as_frame_number(60.0), 60);  // 60 fps
    /// ```
    #[must_use]
    #[inline]
    pub fn as_frame_number(&self, fps: f64) -> u64 {
        let secs = self.as_secs_f64();
        if secs < 0.0 {
            log::warn!(
                "timestamp is negative, returning frame 0 \
                 secs={secs} fps={fps} fallback=0"
            );
            0
        } else {
            (secs * fps).round() as u64
        }
    }

    /// Converts the timestamp to a frame number using a rational frame rate.
    ///
    /// # Arguments
    ///
    /// * `fps` - The frame rate as a rational number
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let ts = Timestamp::new(90000, Rational::new(1, 90000));  // 1 second
    /// let fps = Rational::new(30000, 1001);  // 29.97 fps
    /// let frame = ts.as_frame_number_rational(fps);
    /// assert!(frame == 29 || frame == 30);  // Should be approximately 30
    /// ```
    #[must_use]
    pub fn as_frame_number_rational(&self, fps: Rational) -> u64 {
        self.as_frame_number(fps.as_f64())
    }

    /// Rescales this timestamp to a different time base.
    ///
    /// # Arguments
    ///
    /// * `new_time_base` - The target time base
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let ts = Timestamp::new(1000, Rational::new(1, 1000));  // 1 second
    /// let rescaled = ts.rescale(Rational::new(1, 90000));
    /// assert_eq!(rescaled.pts(), 90000);
    /// ```
    #[must_use]
    pub fn rescale(&self, new_time_base: Rational) -> Self {
        let secs = self.as_secs_f64();
        Self::from_secs_f64(secs, new_time_base)
    }

    /// Returns true if this timestamp is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let zero = Timestamp::zero(Rational::new(1, 90000));
    /// assert!(zero.is_zero());
    ///
    /// let non_zero = Timestamp::new(100, Rational::new(1, 90000));
    /// assert!(!non_zero.is_zero());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_zero(&self) -> bool {
        self.pts == 0
    }

    /// Returns true if this timestamp is negative.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timestamp};
    ///
    /// let negative = Timestamp::new(-100, Rational::new(1, 90000));
    /// assert!(negative.is_negative());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_negative(&self) -> bool {
        self.pts < 0
    }

    /// Returns a sentinel `Timestamp` representing "no PTS available".
    ///
    /// This mirrors `FFmpeg`'s `AV_NOPTS_VALUE` (`INT64_MIN`). Use [`is_valid`](Self::is_valid)
    /// to check before calling any conversion method.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Timestamp;
    ///
    /// let ts = Timestamp::invalid();
    /// assert!(!ts.is_valid());
    /// ```
    #[must_use]
    pub const fn invalid() -> Self {
        Self {
            pts: i64::MIN,
            time_base: Rational::new(1, 1),
        }
    }

    /// Returns `true` if this timestamp represents a real PTS value.
    ///
    /// Returns `false` when the timestamp was constructed via [`invalid`](Self::invalid),
    /// which corresponds to `FFmpeg`'s `AV_NOPTS_VALUE`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Timestamp, Rational};
    ///
    /// let valid = Timestamp::new(1000, Rational::new(1, 48000));
    /// assert!(valid.is_valid());
    ///
    /// let invalid = Timestamp::invalid();
    /// assert!(!invalid.is_valid());
    /// ```
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.pts != i64::MIN
    }
}

impl Default for Timestamp {
    /// Returns a default timestamp (0 with 1/90000 time base).
    fn default() -> Self {
        Self::new(0, Rational::new(1, 90000))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.as_secs_f64();
        let hours = (secs / 3600.0).floor() as u32;
        let mins = ((secs % 3600.0) / 60.0).floor() as u32;
        let secs_remainder = secs % 60.0;
        write!(f, "{hours:02}:{mins:02}:{secs_remainder:06.3}")
    }
}

impl PartialEq for Timestamp {
    fn eq(&self, other: &Self) -> bool {
        // Compare by converting to common representation (seconds)
        (self.as_secs_f64() - other.as_secs_f64()).abs() < 1e-9
    }
}

impl Eq for Timestamp {}

impl PartialOrd for Timestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_secs_f64()
            .partial_cmp(&other.as_secs_f64())
            .unwrap_or_else(|| {
                log::warn!(
                    "NaN timestamp comparison, treating as equal \
                     self_pts={} other_pts={} fallback=Ordering::Equal",
                    self.pts,
                    other.pts
                );
                Ordering::Equal
            })
    }
}

impl Add for Timestamp {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        let secs = self.as_secs_f64() + rhs.as_secs_f64();
        Self::from_secs_f64(secs, self.time_base)
    }
}

impl Sub for Timestamp {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        let secs = self.as_secs_f64() - rhs.as_secs_f64();
        Self::from_secs_f64(secs, self.time_base)
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        let secs = self.as_secs_f64() + rhs.as_secs_f64();
        Self::from_secs_f64(secs, self.time_base)
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self::Output {
        let secs = self.as_secs_f64() - rhs.as_secs_f64();
        Self::from_secs_f64(secs, self.time_base)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::redundant_closure_for_method_calls
)]
mod tests {
    use super::*;

    /// Helper for approximate float comparison in tests
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    mod timestamp_tests {
        use super::*;

        fn time_base_90k() -> Rational {
            Rational::new(1, 90000)
        }

        fn time_base_1k() -> Rational {
            Rational::new(1, 1000)
        }

        #[test]
        fn test_new() {
            let ts = Timestamp::new(90000, time_base_90k());
            assert_eq!(ts.pts(), 90000);
            assert_eq!(ts.time_base(), time_base_90k());
        }

        #[test]
        fn test_zero() {
            let ts = Timestamp::zero(time_base_90k());
            assert_eq!(ts.pts(), 0);
            assert!(ts.is_zero());
            assert!(approx_eq(ts.as_secs_f64(), 0.0));
        }

        #[test]
        fn test_from_duration() {
            let ts = Timestamp::from_duration(Duration::from_secs(1), time_base_90k());
            assert_eq!(ts.pts(), 90000);

            let ts = Timestamp::from_duration(Duration::from_millis(500), time_base_90k());
            assert_eq!(ts.pts(), 45000);
        }

        #[test]
        fn test_from_secs_f64() {
            let ts = Timestamp::from_secs_f64(1.5, time_base_1k());
            assert_eq!(ts.pts(), 1500);
        }

        #[test]
        fn test_from_millis() {
            let ts = Timestamp::from_millis(1000, time_base_90k());
            assert_eq!(ts.pts(), 90000);

            let ts = Timestamp::from_millis(500, time_base_1k());
            assert_eq!(ts.pts(), 500);
        }

        #[test]
        fn test_as_duration() {
            let ts = Timestamp::new(90000, time_base_90k());
            let duration = ts.as_duration();
            assert_eq!(duration, Duration::from_secs(1));

            // Negative timestamp clamps to zero
            let ts = Timestamp::new(-100, time_base_90k());
            assert_eq!(ts.as_duration(), Duration::ZERO);
        }

        #[test]
        fn as_duration_should_clamp_a_negative_codec_delay_to_zero() {
            // Opus states its pre-skip as a negative first timestamp, so this is
            // ordinary media rather than corruption.
            let ts = Timestamp::new(-7, time_base_1k());
            assert_eq!(ts.as_duration(), Duration::ZERO);
        }

        #[test]
        fn as_duration_should_clamp_an_infinite_time_base_to_zero() {
            // `Rational::as_f64` answers INFINITY for a positive numerator over a
            // zero denominator, which no sign test catches.
            let ts = Timestamp::new(1, Rational::new(1, 0));
            assert!(ts.as_secs_f64().is_infinite());
            assert_eq!(ts.as_duration(), Duration::ZERO);
        }

        #[test]
        fn as_duration_should_clamp_a_nan_time_base_to_zero() {
            // 0/0 answers NaN, for which `secs < 0.0` is also false.
            let ts = Timestamp::new(1, Rational::new(0, 0));
            assert!(ts.as_secs_f64().is_nan());
            assert_eq!(ts.as_duration(), Duration::ZERO);
        }

        #[test]
        fn as_duration_should_clamp_seconds_beyond_duration_to_zero() {
            let ts = Timestamp::new(i64::MAX, Rational::new(i32::MAX, 1));
            assert!(ts.as_secs_f64() > Duration::MAX.as_secs_f64());
            assert_eq!(ts.as_duration(), Duration::ZERO);
        }

        #[test]
        fn test_as_secs_f64() {
            let ts = Timestamp::new(45000, time_base_90k());
            assert!((ts.as_secs_f64() - 0.5).abs() < 0.0001);
        }

        #[test]
        fn test_as_millis() {
            let ts = Timestamp::new(90000, time_base_90k());
            assert_eq!(ts.as_millis(), 1000);

            let ts = Timestamp::new(45000, time_base_90k());
            assert_eq!(ts.as_millis(), 500);
        }

        #[test]
        fn test_as_micros() {
            let ts = Timestamp::new(90, time_base_90k());
            assert_eq!(ts.as_micros(), 1000); // 90/90000 = 0.001 sec = 1000 us
        }

        #[test]
        fn test_as_frame_number() {
            let ts = Timestamp::new(90000, time_base_90k()); // 1 second
            assert_eq!(ts.as_frame_number(30.0), 30);
            assert_eq!(ts.as_frame_number(60.0), 60);
            assert_eq!(ts.as_frame_number(24.0), 24);

            // Negative timestamp
            let ts = Timestamp::new(-90000, time_base_90k());
            assert_eq!(ts.as_frame_number(30.0), 0);
        }

        #[test]
        fn test_as_frame_number_rational() {
            let ts = Timestamp::new(90000, time_base_90k()); // 1 second
            let fps = Rational::new(30, 1);
            assert_eq!(ts.as_frame_number_rational(fps), 30);
        }

        #[test]
        fn test_rescale() {
            let ts = Timestamp::new(1000, time_base_1k()); // 1 second
            let rescaled = ts.rescale(time_base_90k());
            assert_eq!(rescaled.pts(), 90000);
        }

        #[test]
        fn test_is_zero() {
            assert!(Timestamp::zero(time_base_90k()).is_zero());
            assert!(!Timestamp::new(1, time_base_90k()).is_zero());
        }

        #[test]
        fn test_is_negative() {
            assert!(Timestamp::new(-100, time_base_90k()).is_negative());
            assert!(!Timestamp::new(100, time_base_90k()).is_negative());
            assert!(!Timestamp::new(0, time_base_90k()).is_negative());
        }

        #[test]
        fn test_display() {
            // 1 hour, 2 minutes, 3.456 seconds
            let secs = 3600.0 + 120.0 + 3.456;
            let ts = Timestamp::from_secs_f64(secs, time_base_90k());
            let display = format!("{ts}");
            assert!(display.starts_with("01:02:03"));
        }

        #[test]
        fn test_eq() {
            let ts1 = Timestamp::new(90000, time_base_90k());
            let ts2 = Timestamp::new(1000, time_base_1k());
            assert_eq!(ts1, ts2); // Both are 1 second
        }

        #[test]
        fn test_ord() {
            let ts1 = Timestamp::new(45000, time_base_90k()); // 0.5 sec
            let ts2 = Timestamp::new(90000, time_base_90k()); // 1.0 sec
            assert!(ts1 < ts2);
            assert!(ts2 > ts1);
        }

        #[test]
        fn test_add() {
            let ts1 = Timestamp::new(45000, time_base_90k());
            let ts2 = Timestamp::new(45000, time_base_90k());
            let sum = ts1 + ts2;
            assert_eq!(sum.pts(), 90000);
        }

        #[test]
        fn test_sub() {
            let ts1 = Timestamp::new(90000, time_base_90k());
            let ts2 = Timestamp::new(45000, time_base_90k());
            let diff = ts1 - ts2;
            assert_eq!(diff.pts(), 45000);
        }

        #[test]
        fn test_add_duration() {
            let ts = Timestamp::new(45000, time_base_90k());
            let result = ts + Duration::from_millis(500);
            assert_eq!(result.pts(), 90000);
        }

        #[test]
        fn test_sub_duration() {
            let ts = Timestamp::new(90000, time_base_90k());
            let result = ts - Duration::from_millis(500);
            assert_eq!(result.pts(), 45000);
        }

        #[test]
        fn test_default() {
            let ts = Timestamp::default();
            assert_eq!(ts.pts(), 0);
            assert_eq!(ts.time_base(), Rational::new(1, 90000));
        }

        #[test]
        fn test_video_timestamps() {
            // Common video time base: 1/90000 (MPEG-TS)
            let time_base = Rational::new(1, 90000);

            // At 30 fps, each frame is 3000 PTS units
            let frame_duration_pts = 90000 / 30;
            assert_eq!(frame_duration_pts, 3000);

            // Frame 0
            let frame0 = Timestamp::new(0, time_base);
            assert_eq!(frame0.as_frame_number(30.0), 0);

            // Frame 30 (1 second)
            let frame30 = Timestamp::new(90000, time_base);
            assert_eq!(frame30.as_frame_number(30.0), 30);
        }

        #[test]
        fn test_audio_timestamps() {
            // Audio at 48kHz - each sample is 1/48000 seconds
            let time_base = Rational::new(1, 48000);

            // 1024 samples (common audio frame size)
            let ts = Timestamp::new(1024, time_base);
            let ms = ts.as_secs_f64() * 1000.0;
            assert!((ms - 21.333).abs() < 0.01); // ~21.33 ms
        }

        #[test]
        fn invalid_timestamp_is_not_valid() {
            let ts = Timestamp::invalid();
            assert!(!ts.is_valid());
        }

        #[test]
        fn zero_timestamp_is_valid() {
            let ts = Timestamp::zero(Rational::new(1, 48000));
            assert!(ts.is_valid());
        }

        #[test]
        fn real_timestamp_is_valid() {
            let ts = Timestamp::new(1000, Rational::new(1, 48000));
            assert!(ts.is_valid());
        }

        #[test]
        fn default_timestamp_is_valid() {
            // Timestamp::default() has pts=0 (not the sentinel)
            let ts = Timestamp::default();
            assert!(ts.is_valid());
        }
    }
}
