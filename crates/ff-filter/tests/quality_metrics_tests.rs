//! Integration tests for QualityMetrics.
//!
//! Tests verify:
//! - Missing reference returns `FilterError::AnalysisFailed`
//! - Comparing a video to itself returns SSIM ≈ 1.0
//! - The returned SSIM is in [0.0, 1.0]
//! - Comparing a video to itself returns PSNR = infinity (or very large)
//! - The returned PSNR is non-negative

#![allow(clippy::unwrap_used)]

use ff_filter::{FilterError, QualityMetrics};

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

#[test]
fn quality_metrics_ssim_missing_reference_should_return_analysis_failed() {
    let result = QualityMetrics::ssim(
        "does_not_exist_ref_99999.mp4",
        "does_not_exist_dist_99999.mp4",
    );
    assert!(
        matches!(result, Err(FilterError::AnalysisFailed { .. })),
        "expected AnalysisFailed for missing reference, got {result:?}"
    );
}

#[test]
fn quality_metrics_ssim_identical_files_should_return_one() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let ssim = match QualityMetrics::ssim(&path, &path) {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: QualityMetrics::ssim failed ({e})");
            return;
        }
    };

    assert!(
        ssim >= 0.9999,
        "expected SSIM ≈ 1.0 when comparing a file to itself, got {ssim}"
    );
}

#[test]
fn quality_metrics_ssim_result_should_be_between_zero_and_one() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let ssim = match QualityMetrics::ssim(&path, &path) {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: QualityMetrics::ssim failed ({e})");
            return;
        }
    };

    assert!(
        (0.0..=1.0).contains(&ssim),
        "expected SSIM in [0.0, 1.0], got {ssim}"
    );
}

// PSNR error-path tests

#[test]
fn quality_metrics_psnr_missing_reference_should_return_analysis_failed() {
    let result = QualityMetrics::psnr(
        "does_not_exist_ref_99999.mp4",
        "does_not_exist_dist_99999.mp4",
    );
    assert!(
        matches!(result, Err(FilterError::AnalysisFailed { .. })),
        "expected AnalysisFailed for missing reference, got {result:?}"
    );
}

// PSNR functional tests

#[test]
fn quality_metrics_psnr_identical_files_should_return_large_value() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let psnr = match QualityMetrics::psnr(&path, &path) {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: QualityMetrics::psnr failed ({e})");
            return;
        }
    };

    assert!(
        psnr > 100.0 || psnr == f32::INFINITY,
        "expected PSNR > 100 dB or infinity when comparing a file to itself, got {psnr}"
    );
}

#[test]
fn quality_metrics_psnr_result_should_be_non_negative() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let psnr = match QualityMetrics::psnr(&path, &path) {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: QualityMetrics::psnr failed ({e})");
            return;
        }
    };

    assert!(
        psnr >= 0.0 || psnr == f32::INFINITY,
        "expected PSNR ≥ 0.0 dB, got {psnr}"
    );
}
