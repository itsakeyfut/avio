//! Constructors, metadata accessors, and trait impls for [`AudioFrame`].

use std::fmt;
use std::time::Duration;

use crate::error::FrameError;
use crate::{SampleFormat, Timestamp};

use super::AudioFrame;

impl AudioFrame {
    /// Creates a new audio frame with the specified parameters.
    ///
    /// # Arguments
    ///
    /// * `planes` - Audio sample data (1 plane for packed, channels for planar)
    /// * `samples` - Number of samples per channel
    /// * `channels` - Number of audio channels
    /// * `sample_rate` - Sample rate in Hz
    /// * `format` - Sample format
    /// * `timestamp` - Presentation timestamp
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::InvalidPlaneCount`] if the number of planes doesn't
    /// match the format (1 for packed, channels for planar).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat, Timestamp};
    ///
    /// // Create a mono F32 frame with 1024 samples
    /// let samples = 1024;
    /// let bytes_per_sample = 4; // F32
    /// let data = vec![0u8; samples * bytes_per_sample];
    ///
    /// let frame = AudioFrame::new(
    ///     vec![data],
    ///     samples,
    ///     1,
    ///     48000,
    ///     SampleFormat::F32,
    ///     Timestamp::default(),
    /// ).unwrap();
    ///
    /// assert_eq!(frame.samples(), 1024);
    /// assert_eq!(frame.channels(), 1);
    /// ```
    pub fn new(
        planes: Vec<Vec<u8>>,
        samples: usize,
        channels: u32,
        sample_rate: u32,
        format: SampleFormat,
        timestamp: Timestamp,
    ) -> Result<Self, FrameError> {
        let expected_planes = if format.is_planar() {
            channels as usize
        } else {
            1
        };

        if planes.len() != expected_planes {
            return Err(FrameError::InvalidPlaneCount {
                expected: expected_planes,
                actual: planes.len(),
            });
        }

        // A plane short of its declared sample count overruns itself once something
        // downstream reads what the frame says it holds, which is how a packed plane
        // copied one channel short reached `swr_convert` and read past its end
        // (#1812, #1849). `Other` is exempt because its `bytes_per_sample` is a
        // guess, and `new` accepts it where `empty` does not.
        if !matches!(format, SampleFormat::Other(_)) {
            // Saturating rather than checked: a geometry that overflows `usize`
            // cannot be satisfied by any plane either way, and saturating keeps
            // the comparison below as the single place that reports a length, so
            // the error carries a measured `actual` instead of a placeholder.
            let per_plane_samples = if format.is_planar() {
                samples
            } else {
                samples.saturating_mul(channels as usize)
            };
            let expected_len = per_plane_samples.saturating_mul(format.bytes_per_sample());
            if let Some(plane) = planes.iter().find(|p| p.len() < expected_len) {
                return Err(FrameError::InvalidDataSize {
                    expected: expected_len,
                    actual: plane.len(),
                });
            }
        }

        Ok(Self {
            planes,
            samples,
            channels,
            sample_rate,
            format,
            timestamp,
        })
    }

    /// Creates an empty audio frame with the specified parameters.
    ///
    /// The frame will have properly sized planes filled with zeros.
    ///
    /// # Arguments
    ///
    /// * `samples` - Number of samples per channel
    /// * `channels` - Number of audio channels
    /// * `sample_rate` - Sample rate in Hz
    /// * `format` - Sample format
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::UnsupportedSampleFormat`] if the format is
    /// [`SampleFormat::Other`], as the memory layout cannot be determined.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// // Create a stereo I16 frame
    /// let frame = AudioFrame::empty(1024, 2, 44100, SampleFormat::I16).unwrap();
    /// assert_eq!(frame.samples(), 1024);
    /// assert_eq!(frame.channels(), 2);
    /// assert_eq!(frame.num_planes(), 1); // Packed format
    /// ```
    pub fn empty(
        samples: usize,
        channels: u32,
        sample_rate: u32,
        format: SampleFormat,
    ) -> Result<Self, FrameError> {
        if matches!(format, SampleFormat::Other(_)) {
            return Err(FrameError::UnsupportedSampleFormat(format));
        }

        let planes = Self::allocate_planes(samples, channels, format);

        Ok(Self {
            planes,
            samples,
            channels,
            sample_rate,
            format,
            timestamp: Timestamp::default(),
        })
    }

    /// Creates a silent audio frame with 1024 zero-filled samples.
    ///
    /// `pts_ms` is the presentation timestamp in milliseconds.
    /// Both planar formats (one plane per channel) and packed formats (single
    /// interleaved plane) are supported.
    #[doc(hidden)]
    #[must_use]
    pub fn new_silent(sample_rate: u32, channels: u32, format: SampleFormat, pts_ms: i64) -> Self {
        let samples = 1024usize;
        let bps = format.bytes_per_sample();
        let planes = if format.is_planar() {
            (0..channels as usize)
                .map(|_| vec![0u8; samples * bps])
                .collect()
        } else {
            vec![vec![0u8; samples * channels as usize * bps]]
        };
        let timestamp = Timestamp::from_millis(pts_ms, crate::Rational::new(1, 1000));
        Self {
            planes,
            samples,
            channels,
            sample_rate,
            format,
            timestamp,
        }
    }

    /// Allocates planes for the given parameters.
    pub(super) fn allocate_planes(
        samples: usize,
        channels: u32,
        format: SampleFormat,
    ) -> Vec<Vec<u8>> {
        let bytes_per_sample = format.bytes_per_sample();

        if format.is_planar() {
            // Planar: one plane per channel
            let plane_size = samples * bytes_per_sample;
            (0..channels).map(|_| vec![0u8; plane_size]).collect()
        } else {
            // Packed: single plane with interleaved samples
            let total_size = samples * channels as usize * bytes_per_sample;
            vec![vec![0u8; total_size]]
        }
    }

    // ==========================================================================
    // Metadata Accessors
    // ==========================================================================

    /// Returns the number of samples per channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.samples(), 1024);
    /// ```
    #[must_use]
    #[inline]
    pub const fn samples(&self) -> usize {
        self.samples
    }

    /// Returns the number of audio channels.
    ///
    /// The return type is `u32` to match `FFmpeg`'s `AVFrame::ch_layout.nb_channels`
    /// and professional audio APIs (Core Audio, WASAPI, JACK, Dolby Atmos).
    ///
    /// # Integration
    ///
    /// Playback libraries such as `rodio` and `cpal` accept channel counts as
    /// `u16`. Cast with `frame.channels() as u16`; the truncation is always safe
    /// because no real-world format exceeds `u16::MAX` (65 535) channels.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.channels(), 2);
    /// ```
    #[must_use]
    #[inline]
    pub const fn channels(&self) -> u32 {
        self.channels
    }

    /// Returns the sample rate in Hz.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.sample_rate(), 48000);
    /// ```
    #[must_use]
    #[inline]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the sample format.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.format(), SampleFormat::F32);
    /// assert!(frame.format().is_float());
    /// ```
    #[must_use]
    #[inline]
    pub const fn format(&self) -> SampleFormat {
        self.format
    }

    /// Returns the presentation timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat, Timestamp, Rational};
    ///
    /// let ts = Timestamp::new(48000, Rational::new(1, 48000));
    /// let mut frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// frame.set_timestamp(ts);
    /// assert_eq!(frame.timestamp(), ts);
    /// ```
    #[must_use]
    #[inline]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Sets the presentation timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat, Timestamp, Rational};
    ///
    /// let mut frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// let ts = Timestamp::new(48000, Rational::new(1, 48000));
    /// frame.set_timestamp(ts);
    /// assert_eq!(frame.timestamp(), ts);
    /// ```
    #[inline]
    pub fn set_timestamp(&mut self, timestamp: Timestamp) {
        self.timestamp = timestamp;
    }

    /// Returns the duration of this audio frame.
    ///
    /// The duration is calculated as `samples / sample_rate`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{AudioFrame, SampleFormat};
    ///
    /// // 1024 samples at 48kHz = ~21.33ms
    /// let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
    /// let duration = frame.duration();
    /// assert!((duration.as_secs_f64() - 0.02133).abs() < 0.001);
    ///
    /// // 48000 samples at 48kHz = 1 second
    /// let frame = AudioFrame::empty(48000, 2, 48000, SampleFormat::F32).unwrap();
    /// assert_eq!(frame.duration().as_secs(), 1);
    /// ```
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Audio frame sample counts are well within f64's precision
    pub fn duration(&self) -> Duration {
        if self.sample_rate == 0 {
            log::warn!(
                "duration unavailable, sample_rate is 0, returning zero \
                 samples={} fallback=Duration::ZERO",
                self.samples
            );
            return Duration::ZERO;
        }
        let secs = self.samples as f64 / f64::from(self.sample_rate);
        Duration::from_secs_f64(secs)
    }
}

impl fmt::Debug for AudioFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioFrame")
            .field("samples", &self.samples)
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("format", &self.format)
            .field("timestamp", &self.timestamp)
            .field("num_planes", &self.planes.len())
            .field(
                "plane_sizes",
                &self.planes.iter().map(Vec::len).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl fmt::Display for AudioFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration_ms = self.duration().as_secs_f64() * 1000.0;
        write!(
            f,
            "AudioFrame({} samples, {}ch, {}Hz, {} @ {}, {:.2}ms)",
            self.samples, self.channels, self.sample_rate, self.format, self.timestamp, duration_ms
        )
    }
}

impl Default for AudioFrame {
    /// Returns a default empty mono F32 frame with 0 samples.
    fn default() -> Self {
        Self {
            planes: vec![vec![]],
            samples: 0,
            channels: 1,
            sample_rate: 48000,
            format: SampleFormat::F32,
            timestamp: Timestamp::default(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::redundant_closure_for_method_calls)]
mod tests {
    use super::*;
    use crate::Rational;

    #[test]
    fn test_new_packed_f32() {
        let samples = 1024;
        let channels = 2u32;
        let bytes_per_sample = 4;
        let data = vec![0u8; samples * channels as usize * bytes_per_sample];

        let frame = AudioFrame::new(
            vec![data],
            samples,
            channels,
            48000,
            SampleFormat::F32,
            Timestamp::default(),
        )
        .unwrap();

        assert_eq!(frame.samples(), 1024);
        assert_eq!(frame.channels(), 2);
        assert_eq!(frame.sample_rate(), 48000);
        assert_eq!(frame.format(), SampleFormat::F32);
        assert_eq!(frame.num_planes(), 1);
    }

    #[test]
    fn test_new_planar_f32p() {
        let samples = 1024;
        let channels = 2u32;
        let bytes_per_sample = 4;
        let plane_size = samples * bytes_per_sample;

        let planes = vec![vec![0u8; plane_size], vec![0u8; plane_size]];

        let frame = AudioFrame::new(
            planes,
            samples,
            channels,
            48000,
            SampleFormat::F32p,
            Timestamp::default(),
        )
        .unwrap();

        assert_eq!(frame.samples(), 1024);
        assert_eq!(frame.channels(), 2);
        assert_eq!(frame.format(), SampleFormat::F32p);
        assert_eq!(frame.num_planes(), 2);
    }

    #[test]
    fn new_should_reject_a_packed_plane_missing_its_channel_factor() {
        // The defect behind #1812 and #1849: a packed plane sized as though it held
        // one channel. 4096 stereo F32 samples need 4096 * 2 * 4 bytes.
        let result = AudioFrame::new(
            vec![vec![0u8; 4096 * 4]],
            4096,
            2,
            48_000,
            SampleFormat::F32,
            Timestamp::default(),
        );

        assert_eq!(
            result.unwrap_err(),
            FrameError::InvalidDataSize {
                expected: 4096 * 2 * 4,
                actual: 4096 * 4
            },
            "a packed plane one channel short must be rejected at construction"
        );
    }

    #[test]
    fn new_should_accept_a_correctly_sized_packed_plane() {
        let frame = AudioFrame::new(
            vec![vec![0u8; 4096 * 2 * 4]],
            4096,
            2,
            48_000,
            SampleFormat::F32,
            Timestamp::default(),
        )
        .expect("a correctly sized packed plane must be accepted");

        assert_eq!(frame.samples(), 4096);
        assert_eq!(frame.num_planes(), 1);
    }

    #[test]
    fn new_should_reject_a_planar_plane_shorter_than_its_sample_count() {
        // Planar planes hold one channel each, so the channel factor must NOT apply.
        let result = AudioFrame::new(
            vec![vec![0u8; 1024 * 4], vec![0u8; 512 * 4]],
            1024,
            2,
            48_000,
            SampleFormat::F32p,
            Timestamp::default(),
        );

        assert_eq!(
            result.unwrap_err(),
            FrameError::InvalidDataSize {
                expected: 1024 * 4,
                actual: 512 * 4
            },
            "every planar plane must cover the declared sample count"
        );
    }

    #[test]
    fn new_should_skip_the_length_check_for_an_unknown_format() {
        // `bytes_per_sample` is a guess for `Other`, so a length check on it would
        // reject frames it cannot measure. `new` accepts `Other` where `empty` does not.
        let frame = AudioFrame::new(
            vec![vec![0u8; 8]],
            4096,
            2,
            48_000,
            SampleFormat::Other(99),
            Timestamp::default(),
        )
        .expect("an unknown format must not be length checked");

        assert_eq!(frame.format(), SampleFormat::Other(99));
    }

    #[test]
    fn test_new_invalid_plane_count_packed() {
        // Packed format should have 1 plane, but we provide 2
        let result = AudioFrame::new(
            vec![vec![0u8; 100], vec![0u8; 100]],
            100,
            2,
            48000,
            SampleFormat::F32,
            Timestamp::default(),
        );

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            FrameError::InvalidPlaneCount {
                expected: 1,
                actual: 2
            }
        );
    }

    #[test]
    fn test_new_invalid_plane_count_planar() {
        // Planar format with 2 channels should have 2 planes, but we provide 1
        let result = AudioFrame::new(
            vec![vec![0u8; 100]],
            100,
            2,
            48000,
            SampleFormat::F32p,
            Timestamp::default(),
        );

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            FrameError::InvalidPlaneCount {
                expected: 2,
                actual: 1
            }
        );
    }

    #[test]
    fn test_empty_packed() {
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();

        assert_eq!(frame.samples(), 1024);
        assert_eq!(frame.channels(), 2);
        assert_eq!(frame.sample_rate(), 48000);
        assert_eq!(frame.format(), SampleFormat::F32);
        assert_eq!(frame.num_planes(), 1);
        assert_eq!(frame.total_size(), 1024 * 2 * 4);
    }

    #[test]
    fn test_empty_planar() {
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32p).unwrap();

        assert_eq!(frame.num_planes(), 2);
        assert_eq!(frame.plane(0).map(|p| p.len()), Some(1024 * 4));
        assert_eq!(frame.plane(1).map(|p| p.len()), Some(1024 * 4));
        assert_eq!(frame.total_size(), 1024 * 2 * 4);
    }

    #[test]
    fn test_empty_i16() {
        let frame = AudioFrame::empty(1024, 2, 44100, SampleFormat::I16).unwrap();

        assert_eq!(frame.bytes_per_sample(), 2);
        assert_eq!(frame.total_size(), 1024 * 2 * 2);
    }

    #[test]
    fn test_empty_other_format_error() {
        let result = AudioFrame::empty(1024, 2, 48000, SampleFormat::Other(999));

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            FrameError::UnsupportedSampleFormat(SampleFormat::Other(999))
        );
    }

    #[test]
    fn test_default() {
        let frame = AudioFrame::default();

        assert_eq!(frame.samples(), 0);
        assert_eq!(frame.channels(), 1);
        assert_eq!(frame.sample_rate(), 48000);
        assert_eq!(frame.format(), SampleFormat::F32);
    }

    #[test]
    fn test_duration() {
        // 1024 samples at 48kHz = 0.02133... seconds
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
        let duration = frame.duration();
        assert!((duration.as_secs_f64() - 0.021_333_333).abs() < 0.000_001);

        // 48000 samples at 48kHz = 1 second
        let frame = AudioFrame::empty(48000, 2, 48000, SampleFormat::F32).unwrap();
        assert_eq!(frame.duration().as_secs(), 1);
    }

    #[test]
    fn test_duration_zero_sample_rate() {
        let frame = AudioFrame::new(
            vec![vec![]],
            0,
            1,
            0,
            SampleFormat::F32,
            Timestamp::default(),
        )
        .unwrap();

        assert_eq!(frame.duration(), Duration::ZERO);
    }

    #[test]
    fn test_set_timestamp() {
        let mut frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
        let ts = Timestamp::new(48000, Rational::new(1, 48000));

        frame.set_timestamp(ts);
        assert_eq!(frame.timestamp(), ts);
    }

    #[test]
    fn test_clone() {
        let mut original = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
        original.set_timestamp(Timestamp::new(1000, Rational::new(1, 1000)));

        // Modify some data
        if let Some(data) = original.plane_mut(0) {
            data[0] = 42;
        }

        let cloned = original.clone();

        // Verify metadata matches
        assert_eq!(cloned.samples(), original.samples());
        assert_eq!(cloned.channels(), original.channels());
        assert_eq!(cloned.sample_rate(), original.sample_rate());
        assert_eq!(cloned.format(), original.format());
        assert_eq!(cloned.timestamp(), original.timestamp());

        // Verify data was cloned
        assert_eq!(cloned.plane(0).unwrap()[0], 42);

        // Verify it's a deep clone
        let mut cloned = cloned;
        if let Some(data) = cloned.plane_mut(0) {
            data[0] = 99;
        }
        assert_eq!(original.plane(0).unwrap()[0], 42);
        assert_eq!(cloned.plane(0).unwrap()[0], 99);
    }

    #[test]
    fn audio_frame_clone_should_have_identical_data() {
        let samples = 512;
        let channels = 2u32;
        let bytes_per_sample = 4; // F32
        // Packed stereo interleaves both channels into one plane, so the buffer is
        // `samples * channels * bytes_per_sample`.
        let plane_data = vec![7u8; samples * channels as usize * bytes_per_sample];
        let ts = Timestamp::new(500, Rational::new(1, 1000));

        let original = AudioFrame::new(
            vec![plane_data.clone()],
            samples,
            channels,
            44100,
            SampleFormat::F32,
            ts,
        )
        .unwrap();

        let clone = original.clone();

        assert_eq!(clone.samples(), original.samples());
        assert_eq!(clone.channels(), original.channels());
        assert_eq!(clone.sample_rate(), original.sample_rate());
        assert_eq!(clone.format(), original.format());
        assert_eq!(clone.timestamp(), original.timestamp());
        assert_eq!(clone.num_planes(), original.num_planes());
        assert_eq!(clone.plane(0), original.plane(0));
    }

    #[test]
    fn test_debug() {
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
        let debug = format!("{frame:?}");
        assert!(debug.contains("AudioFrame"));
        assert!(debug.contains("1024"));
        assert!(debug.contains("48000"));
        assert!(debug.contains("F32"));
    }

    #[test]
    fn test_display() {
        let frame = AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap();
        let display = format!("{frame}");
        assert!(display.contains("1024 samples"));
        assert!(display.contains("2ch"));
        assert!(display.contains("48000Hz"));
        assert!(display.contains("flt")); // F32 displays as "flt"
    }
}
