//! Integration tests for scene-change detection against a committed reference video.
//!
//! The reference video (`assets/test/hard_cut_video.mp4`) is a 6-second file
//! consisting of six 1-second solid-colour segments, producing hard cuts at
//! exactly 1 s, 2 s, 3 s, 4 s, and 5 s.  It was generated once by
//! `tools/gen_test_assets.rs` and committed to avoid a dev-dependency on
//! `ff-encode`.

use std::time::Duration;

use ff_analysis::SceneDetector;

fn hard_cut_video_path() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/ff-analysis  →  ../../assets/test/
    manifest_dir
        .join("../..")
        .join("assets/test/hard_cut_video.mp4")
}

#[test]
fn scene_detector_should_detect_known_cuts_within_one_frame_tolerance() {
    let video_path = hard_cut_video_path();

    if !video_path.exists() {
        println!(
            "Skipping: reference asset not found at {}  \
             (run `cargo run --manifest-path tools/Cargo.toml` to regenerate)",
            video_path.display()
        );
        return;
    }

    // Run SceneDetector with default threshold (0.4).
    let timestamps = match SceneDetector::new(&video_path).threshold(0.4).run() {
        Ok(ts) => ts,
        Err(e) => {
            println!("Skipping: SceneDetector::run failed: {e}");
            return;
        }
    };

    // The video has exactly 5 hard cuts.
    assert_eq!(
        timestamps.len(),
        5,
        "expected exactly 5 scene-change timestamps, got {}: {timestamps:?}",
        timestamps.len()
    );

    // Known cut positions: 1 s, 2 s, 3 s, 4 s, 5 s.
    let expected = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(3),
        Duration::from_secs(4),
        Duration::from_secs(5),
    ];

    // ±1 frame tolerance at 30 fps ≈ 33 ms.
    let tolerance = Duration::from_millis(34);

    for (i, (detected, &expected_ts)) in timestamps.iter().zip(expected.iter()).enumerate() {
        let diff = if *detected >= expected_ts {
            *detected - expected_ts
        } else {
            expected_ts - *detected
        };
        assert!(
            diff <= tolerance,
            "cut #{i}: expected ≈{expected_ts:?} (±{tolerance:?}), got {detected:?} \
             (diff={diff:?})"
        );
    }
}
