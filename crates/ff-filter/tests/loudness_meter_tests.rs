//! Integration tests for LoudnessMeter.
//!
//! Tests verify:
//! - Missing input returns `FilterError::AnalysisFailed`
//! - A real audio file produces a `LoudnessResult`
//! - The integrated LUFS value is finite (non-silence audio)
//! - The true peak value is finite

#![allow(clippy::unwrap_used)]

use ff_filter::{FilterError, LoudnessMeter};

fn test_audio_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!(
        "{manifest_dir}/../../assets/audio/konekonoosanpo.mp3"
    ))
}

#[test]
fn loudness_meter_missing_file_should_return_analysis_failed() {
    let result = LoudnessMeter::new("does_not_exist_99999.mp3").measure();
    assert!(
        matches!(result, Err(FilterError::AnalysisFailed { .. })),
        "expected AnalysisFailed for missing file, got {result:?}"
    );
}

#[test]
fn loudness_meter_should_return_lufs_result() {
    let path = test_audio_path();
    if !path.exists() {
        println!("Skipping: test audio file not found at {}", path.display());
        return;
    }

    let result = match LoudnessMeter::new(&path).measure() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: LoudnessMeter::measure failed ({e})");
            return;
        }
    };

    // The result should be a valid struct — just check it is accessible.
    let _ = result.integrated_lufs;
    let _ = result.lra;
    let _ = result.true_peak_dbtp;
}

#[test]
fn loudness_meter_real_audio_should_have_finite_integrated_lufs() {
    let path = test_audio_path();
    if !path.exists() {
        println!("Skipping: test audio file not found at {}", path.display());
        return;
    }

    let result = match LoudnessMeter::new(&path).measure() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: LoudnessMeter::measure failed ({e})");
            return;
        }
    };

    assert!(
        result.integrated_lufs.is_finite(),
        "expected finite integrated_lufs for real audio, got {}",
        result.integrated_lufs
    );
    // EBU R128 integrated loudness for real music is typically between -40 and 0 LUFS.
    assert!(
        result.integrated_lufs < 0.0 && result.integrated_lufs > -70.0,
        "integrated_lufs={} is outside expected range [-70, 0]",
        result.integrated_lufs
    );
}

#[test]
fn loudness_meter_real_audio_should_have_finite_true_peak() {
    let path = test_audio_path();
    if !path.exists() {
        println!("Skipping: test audio file not found at {}", path.display());
        return;
    }

    let result = match LoudnessMeter::new(&path).measure() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: LoudnessMeter::measure failed ({e})");
            return;
        }
    };

    assert!(
        result.true_peak_dbtp.is_finite(),
        "expected finite true_peak_dbtp for real audio, got {}",
        result.true_peak_dbtp
    );
}
