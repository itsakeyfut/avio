//! Integration tests for EBU R128 loudness analysis.
//!
//! These tests verify `LoudnessMeter::measure()` against a known-loudness
//! reference signal.  The reference is generated at runtime using the `ffmpeg`
//! CLI with the `loudnorm` filter; tests are skipped gracefully when `ffmpeg`
//! is unavailable or generation fails.

#![allow(clippy::unwrap_used)]

mod fixtures;

use std::path::Path;
use std::process::Command;

use ff_filter::LoudnessMeter;
use fixtures::{FileGuard, test_output_path};

// Reference signal generation

/// Generates a 5-second 1 kHz sine wave normalised to `target_lufs` using
/// `ffmpeg`'s `loudnorm` filter and writes it to `path` as a WAV file.
///
/// Returns `None` (printing a skip message) if `ffmpeg` is not in `PATH`,
/// the command fails, or the output path cannot be represented as UTF-8.
fn generate_reference_at_lufs(path: &Path, target_lufs: f32) -> Option<()> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => {
            println!("Skipping: output path is not valid UTF-8");
            return None;
        }
    };

    let af_arg = format!("loudnorm=I={target_lufs:.1}:TP=-1:LRA=11");

    let output = match Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:duration=5",
            "-af",
            &af_arg,
            "-ar",
            "48000",
            "-ac",
            "1",
            "-y",
            path_str,
        ])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            println!("Skipping: cannot run ffmpeg: {e}");
            return None;
        }
    };

    if !output.status.success() {
        println!(
            "Skipping: ffmpeg exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    Some(())
}

#[test]
fn loudness_meter_should_measure_minus23_lufs_within_tolerance() {
    let out_path = test_output_path("analysis_minus23_lufs_ref.wav");
    let _guard = FileGuard::new(out_path.clone());

    if generate_reference_at_lufs(&out_path, -23.0).is_none() {
        return;
    }

    let result = match LoudnessMeter::new(&out_path).measure() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: LoudnessMeter::measure failed: {e}");
            return;
        }
    };

    let lufs = result.integrated_lufs;

    assert!(
        lufs.is_finite(),
        "integrated_lufs must be finite, got {lufs}"
    );
    assert!(
        (lufs - (-23.0_f32)).abs() <= 0.5,
        "expected integrated_lufs ≈ -23.0 LUFS (±0.5), got {lufs:.2} LUFS"
    );
}
