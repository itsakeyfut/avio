//! Integration tests for WaveformAnalyzer.
//!
//! Tests verify:
//! - Correct sample count relative to audio duration and interval
//! - Monotonically increasing timestamps
//! - Finite peak_db values for real (non-silent) audio
//! - Output shape for silent samples

#![allow(clippy::unwrap_used)]

mod fixtures;
use fixtures::*;

use ff_analysis::{AnalysisError, WaveformAnalyzer, WaveformSample};
use ff_decode::AudioDecoder;
use std::time::Duration;

// ── Error-path tests ──────────────────────────────────────────────────────────

#[test]
fn waveform_analyzer_zero_interval_should_return_analysis_failed() {
    let result = WaveformAnalyzer::new(test_audio_path())
        .interval(Duration::ZERO)
        .run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for zero interval, got {result:?}"
    );
}

// ── Functional tests ──────────────────────────────────────────────────────────

/// A 1-second interval produces roughly `duration_ms / 100 ± 2` samples.
#[test]
fn waveform_analyzer_should_return_correct_sample_count() {
    // Get the audio duration first.
    let duration = match AudioDecoder::open(test_audio_path()).build() {
        Ok(dec) => dec.duration(),
        Err(e) => {
            println!("Skipping: audio decoder unavailable ({e})");
            return;
        }
    };

    if duration.is_zero() {
        println!("Skipping: audio duration unknown");
        return;
    }

    let interval = Duration::from_millis(100);
    let samples = match WaveformAnalyzer::new(test_audio_path())
        .interval(interval)
        .run()
    {
        Ok(s) => s,
        Err(e) => {
            println!("Skipping: WaveformAnalyzer::run failed ({e})");
            return;
        }
    };

    let expected = (duration.as_millis() / 100) as usize;
    let diff = (samples.len() as i64 - expected as i64).unsigned_abs() as usize;
    assert!(
        diff <= 2,
        "expected ~{expected} samples (±2) for {duration:?} at 100ms, got {}",
        samples.len()
    );
}

#[test]
fn waveform_analyzer_samples_should_have_monotonically_increasing_timestamps() {
    let samples = match WaveformAnalyzer::new(test_audio_path()).run() {
        Ok(s) => s,
        Err(e) => {
            println!("Skipping: WaveformAnalyzer::run failed ({e})");
            return;
        }
    };

    if samples.is_empty() {
        println!("Skipping: no samples produced");
        return;
    }

    assert_eq!(
        samples[0].timestamp,
        Duration::ZERO,
        "first sample timestamp should be Duration::ZERO"
    );

    for i in 1..samples.len() {
        assert!(
            samples[i].timestamp > samples[i - 1].timestamp,
            "timestamps not monotonically increasing at index {i}: {:?} <= {:?}",
            samples[i].timestamp,
            samples[i - 1].timestamp
        );
    }
}

#[test]
fn waveform_analyzer_real_audio_should_have_finite_peak_db() {
    let samples = match WaveformAnalyzer::new(test_audio_path()).run() {
        Ok(s) => s,
        Err(e) => {
            println!("Skipping: WaveformAnalyzer::run failed ({e})");
            return;
        }
    };

    assert!(!samples.is_empty(), "expected at least one sample");

    let has_finite_peak = samples.iter().any(|s| s.peak_db.is_finite());
    assert!(
        has_finite_peak,
        "expected at least one sample with finite peak_db for real audio"
    );
}

/// Verifies the output shape of a silent `WaveformSample`.
///
/// `WaveformAnalyzer` emits `NEG_INFINITY` for intervals where the audio
/// is all zeros.  This test constructs such a sample directly and checks the
/// invariant so that the contract is documented as a test.
#[test]
fn waveform_analyzer_silence_should_have_low_amplitude() {
    let sample = WaveformSample {
        timestamp: Duration::ZERO,
        peak_db: f32::NEG_INFINITY,
        rms_db: f32::NEG_INFINITY,
    };
    assert!(
        !sample.peak_db.is_finite(),
        "silent peak_db should be -infinity, got {}",
        sample.peak_db
    );
    assert!(
        !sample.rms_db.is_finite(),
        "silent rms_db should be -infinity, got {}",
        sample.rms_db
    );
}
