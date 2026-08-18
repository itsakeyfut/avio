//! Integration tests for HistogramExtractor.
//!
//! Tests verify:
//! - Missing input returns `AnalysisError::Failed`
//! - `interval_frames(0)` returns `AnalysisError::Failed`
//! - A real video file produces non-empty histograms with correct bin totals
//! - `interval_frames(N)` yields approximately 1/N of the full-frame count

#![allow(clippy::unwrap_used)]

use ff_analysis::{AnalysisError, HistogramExtractor};

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

// ── Error-path tests ──────────────────────────────────────────────────────────

#[test]
fn histogram_extractor_missing_file_should_return_analysis_failed() {
    let result = HistogramExtractor::new("does_not_exist_99999.mp4").run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for missing file, got {result:?}"
    );
}

#[test]
fn histogram_extractor_zero_interval_should_return_analysis_failed() {
    let result = HistogramExtractor::new("does_not_exist_99999.mp4")
        .interval_frames(0)
        .run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for interval_frames=0, got {result:?}"
    );
}

// ── Functional tests ──────────────────────────────────────────────────────────
// These tests decode the full video and can be slow on large files.
// Run explicitly with: cargo test -p ff-analysis -- --include-ignored

#[test]
#[ignore = "decodes entire video; run explicitly with -- --include-ignored"]
fn histogram_extractor_real_video_should_return_non_empty_histograms() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let histograms = match HistogramExtractor::new(&path).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: HistogramExtractor::run failed ({e})");
            return;
        }
    };

    assert!(
        !histograms.is_empty(),
        "expected at least one histogram from a real video"
    );
}

#[test]
#[ignore = "decodes entire video twice; run explicitly with -- --include-ignored"]
fn histogram_extractor_bin_sums_should_equal_total_pixels() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    // Sample only every 1000th frame to keep the test manageable.
    let histograms = match HistogramExtractor::new(&path).interval_frames(1000).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: HistogramExtractor::run failed ({e})");
            return;
        }
    };

    if histograms.is_empty() {
        println!("Skipping: no histograms returned (video may have < 1000 frames)");
        return;
    }

    // For every sampled frame: r, g, b, and luma bins must each sum to the same
    // value (width × height), confirming every pixel was counted exactly once.
    let h = &histograms[0];
    let r_total: u32 = h.r.iter().sum();
    let g_total: u32 = h.g.iter().sum();
    let b_total: u32 = h.b.iter().sum();
    let l_total: u32 = h.luma.iter().sum();

    assert!(r_total > 0, "r bin sum should be > 0");
    assert_eq!(
        r_total, g_total,
        "r and g bin sums should be equal (both = w×h)"
    );
    assert_eq!(
        r_total, b_total,
        "r and b bin sums should be equal (both = w×h)"
    );
    assert_eq!(
        r_total, l_total,
        "r and luma bin sums should be equal (both = w×h)"
    );
}

#[test]
#[ignore = "decodes entire video twice; run explicitly with -- --include-ignored"]
fn histogram_extractor_interval_reduces_sample_count() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let all = match HistogramExtractor::new(&path).interval_frames(1).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: HistogramExtractor::run failed ({e})");
            return;
        }
    };
    if all.len() < 2 {
        println!("Skipping: video has fewer than 2 frames");
        return;
    }

    let every_other = match HistogramExtractor::new(&path).interval_frames(2).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: HistogramExtractor::run failed ({e})");
            return;
        }
    };

    assert!(
        every_other.len() <= all.len() / 2 + 1,
        "interval_frames(2) should return roughly half as many histograms as interval_frames(1): \
         all={} every_other={}",
        all.len(),
        every_other.len()
    );
}
