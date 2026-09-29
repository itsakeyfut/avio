//! A locked track renders exactly as an unlocked one (#1805).
//!
//! The lock is an authoring constraint: `apply` refuses edits aimed at the track, and
//! the derivation ignores the flag entirely (ADR-0021). The unit tests cover the
//! refusals; this one covers the other half, because "the derivation ignores it" is a
//! claim about the rendered file rather than about the model.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError, Track};
use ff_filter::FilterError;
use fixtures::{FileGuard, make_source_file, test_output_path, video_luma_per_frame};

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// Renders a one-clip timeline whose track carries `lock`, returning the frame luma
/// read back from the file, or `None` where this build cannot run the pipeline.
fn render_with_lock(src: &std::path::Path, lock: bool, tag: &str) -> Option<(f64, Vec<f64>)> {
    let out = test_output_path(&format!("locked_track_{tag}.mp4"));
    let _g = FileGuard::new(out.clone());
    let timeline = Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track_with(
            Track::new(vec![Clip::new(src).trim(Duration::ZERO, s(1.0))]).locked(lock),
        )
        .build()
        .ok()?;
    match timeline.render(&out, EncoderConfig::builder().build()) {
        Ok(()) => {}
        // The gate turns on the **reason**, not the variant. A composition that
        // dropped the locked track reports `CompositionFailed` too, so skipping on
        // the variant alone would skip on the very failure this test exists for
        // (measured: the mutation that makes the derivation read the lock produces
        // `composition failed: no layers`, and a variant-wide gate passed it).
        Err(TimelineError::Filter(FilterError::CompositionFailed { ref reason }))
            if reason.contains("filter not found") =>
        {
            println!("Skipping: this build lacks a filter the composition needs: {reason}");
            return None;
        }
        Err(TimelineError::Filter(FilterError::BuildFailed)) => {
            println!("Skipping: the graph could not be built here");
            return None;
        }
        Err(ref e @ (TimelineError::Encode(_) | TimelineError::Decode(_))) => {
            println!("Skipping: this build cannot run the pipeline: {e}");
            return None;
        }
        Err(e) => panic!("render failed with lock={lock}: {e}"),
    }
    video_luma_per_frame(&out)
}

#[test]
fn a_locked_track_should_render_the_same_as_an_unlocked_one() {
    let src = test_output_path("locked_track_src.mp4");
    let _g = FileGuard::new(src.clone());
    if make_source_file(&src, 160, 120, 30.0, 45, 90, 100, 140).is_none() {
        return; // no encoder here
    }

    let Some((fps_locked, locked)) = render_with_lock(&src, true, "locked") else {
        return;
    };
    let Some((fps_free, free)) = render_with_lock(&src, false, "free") else {
        return;
    };

    assert!(
        (fps_locked - fps_free).abs() < f64::EPSILON,
        "same frame rate"
    );
    assert_eq!(
        locked.len(),
        free.len(),
        "a locked track must not change how many frames are written"
    );
    for (i, (a, b)) in locked.iter().zip(free.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1.0,
            "frame {i} differs with the lock set: {a} vs {b}"
        );
    }
}
