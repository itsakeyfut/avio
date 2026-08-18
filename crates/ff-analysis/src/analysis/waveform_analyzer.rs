//! Waveform amplitude analysis for audio files.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ff_format::SampleFormat;

use ff_decode::AudioDecoder;

use crate::AnalysisError;

/// A single waveform measurement over a configurable time interval.
///
/// Both amplitude values are expressed in dBFS (decibels relative to full
/// scale). `0.0` dBFS means the signal reached maximum amplitude; values
/// approach [`f32::NEG_INFINITY`] for silence.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformSample {
    /// Start of the time interval this sample covers.
    pub timestamp: Duration,
    /// Peak amplitude in dBFS (`max(|s|)` over all samples in the interval).
    /// [`f32::NEG_INFINITY`] when the interval contains only silence.
    pub peak_db: f32,
    /// RMS amplitude in dBFS (`sqrt(mean(s²))` over all samples).
    /// [`f32::NEG_INFINITY`] when the interval contains only silence.
    pub rms_db: f32,
}

/// Computes peak and RMS amplitude per time interval for an audio file.
///
/// Decodes audio via [`AudioDecoder`] (requesting packed `f32` output so that
/// per-sample arithmetic needs no format dispatch) and computes, for each
/// configurable interval, the peak and RMS amplitudes in dBFS.  The resulting
/// [`Vec<WaveformSample>`] is designed for waveform display rendering.
///
/// # Examples
///
/// ```ignore
/// use ff_analysis::WaveformAnalyzer;
/// use std::time::Duration;
///
/// let samples = WaveformAnalyzer::new("audio.mp3")
///     .interval(Duration::from_millis(50))
///     .run()?;
///
/// for s in &samples {
///     println!("{:?}: peak={:.1} dBFS  rms={:.1} dBFS",
///              s.timestamp, s.peak_db, s.rms_db);
/// }
/// ```
pub struct WaveformAnalyzer {
    input: PathBuf,
    interval: Duration,
}

impl WaveformAnalyzer {
    /// Creates a new analyzer for the given audio file.
    ///
    /// The default sampling interval is 100 ms.  Call
    /// [`interval`](Self::interval) to override it.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            interval: Duration::from_millis(100),
        }
    }

    /// Sets the sampling interval.
    ///
    /// Peak and RMS are computed independently for each interval of this
    /// length.  Passing [`Duration::ZERO`] causes [`run`](Self::run) to
    /// return [`AnalysisError::Failed`].
    ///
    /// Default: 100 ms.
    #[must_use]
    pub fn interval(mut self, d: Duration) -> Self {
        self.interval = d;
        self
    }

    /// Runs the waveform analysis and returns one [`WaveformSample`] per interval.
    ///
    /// The timestamp of each sample is the **start** of its interval.  Audio
    /// is decoded as packed `f32` samples; the decoder performs any necessary
    /// format conversion automatically.
    ///
    /// # Errors
    ///
    /// - [`AnalysisError::Failed`] — interval is [`Duration::ZERO`].
    /// - [`ff_decode::DecodeError::FileNotFound`] — input path does not exist.
    /// - Any other [`ff_decode::DecodeError`] propagated from [`AudioDecoder`],
    ///   wrapped in [`AnalysisError::Decode`].
    pub fn run(self) -> Result<Vec<WaveformSample>, AnalysisError> {
        if self.interval.is_zero() {
            return Err(AnalysisError::Failed {
                reason: "interval must be non-zero".to_string(),
            });
        }

        let mut decoder = AudioDecoder::open(&self.input)
            .output_format(SampleFormat::F32)
            .build()?;

        let mut results: Vec<WaveformSample> = Vec::new();
        let mut interval_start = Duration::ZERO;
        let mut bucket: Vec<f32> = Vec::new();

        while let Some(frame) = decoder.decode_one()? {
            let frame_start = frame.timestamp().as_duration();

            // Flush all completed intervals that end before this frame begins.
            while frame_start >= interval_start + self.interval {
                if bucket.is_empty() {
                    results.push(WaveformSample {
                        timestamp: interval_start,
                        peak_db: f32::NEG_INFINITY,
                        rms_db: f32::NEG_INFINITY,
                    });
                } else {
                    results.push(waveform_sample_from_bucket(interval_start, &bucket));
                    bucket.clear();
                }
                interval_start += self.interval;
            }

            if let Some(samples) = frame.as_f32() {
                bucket.extend_from_slice(samples);
            }
        }

        // Flush the final partial interval.
        if !bucket.is_empty() {
            results.push(waveform_sample_from_bucket(interval_start, &bucket));
        }

        log::debug!("waveform analysis complete samples={}", results.len());
        Ok(results)
    }
}

/// Builds a [`WaveformSample`] from the raw `f32` PCM values accumulated for
/// one interval.
#[allow(clippy::cast_precision_loss)] // sample count fits comfortably in f32
pub(super) fn waveform_sample_from_bucket(timestamp: Duration, samples: &[f32]) -> WaveformSample {
    let peak = samples
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max);

    let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let rms = mean_sq.sqrt();

    WaveformSample {
        timestamp,
        peak_db: amplitude_to_db(peak),
        rms_db: amplitude_to_db(rms),
    }
}

/// Converts a linear amplitude (0.0–1.0) to dBFS.
///
/// Zero and negative amplitudes map to [`f32::NEG_INFINITY`].
pub(super) fn amplitude_to_db(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amplitude.log10()
    }
}

#[cfg(test)]
mod tests {
    use ff_decode::DecodeError;

    use super::*;

    #[test]
    fn amplitude_to_db_zero_should_be_neg_infinity() {
        assert_eq!(amplitude_to_db(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn amplitude_to_db_full_scale_should_be_zero_db() {
        let db = amplitude_to_db(1.0);
        assert!(
            (db - 0.0).abs() < 1e-5,
            "expected ~0 dBFS for full-scale amplitude, got {db}"
        );
    }

    #[test]
    fn amplitude_to_db_half_amplitude_should_be_about_minus_6db() {
        let db = amplitude_to_db(0.5);
        assert!(
            (db - (-6.020_6)).abs() < 0.01,
            "expected ~-6 dBFS for 0.5 amplitude, got {db}"
        );
    }

    #[test]
    fn waveform_analyzer_zero_interval_should_return_analysis_failed() {
        let result = WaveformAnalyzer::new("irrelevant.mp3")
            .interval(Duration::ZERO)
            .run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed, got {result:?}"
        );
    }

    #[test]
    fn waveform_analyzer_nonexistent_file_should_return_file_not_found() {
        let result = WaveformAnalyzer::new("does_not_exist_12345.mp3").run();
        assert!(
            matches!(
                result,
                Err(AnalysisError::Decode(DecodeError::FileNotFound { .. }))
            ),
            "expected Decode(FileNotFound), got {result:?}"
        );
    }

    #[test]
    fn waveform_analyzer_silence_should_have_low_amplitude() {
        let silent: Vec<f32> = vec![0.0; 4800];
        let sample = waveform_sample_from_bucket(Duration::ZERO, &silent);
        assert!(
            sample.peak_db.is_infinite() && sample.peak_db.is_sign_negative(),
            "expected -infinity peak_db for all-zero samples, got {}",
            sample.peak_db
        );
        assert!(
            sample.rms_db.is_infinite() && sample.rms_db.is_sign_negative(),
            "expected -infinity rms_db for all-zero samples, got {}",
            sample.rms_db
        );
    }
}
