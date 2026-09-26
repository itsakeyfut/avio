//! A still image on the timeline is held for its clip's length (#1802).
//!
//! `ClipSource::File` documents that a clip may be backed by an image, and a still is
//! placed constantly in an edit: a logo, a title card, a cut-in. A file-backed still used
//! to contribute exactly one frame of picture. An image source yields one frame and then
//! signals EOF, and since #1803 a layer that has ended stops contributing, so the rest of
//! the clip rendered as background: 90 output frames, one of them with a picture in it.
//!
//! The file that came out was the right length and carried a video stream, so nothing
//! reported it, which is why the assertions here are on the **frames** rather than on the
//! render returning `Ok` or on the duration (RK-031).
//!
//! The three outcomes this replaces were split by route, not by format: the CPU route gave
//! one frame of picture, the GPU route none at all, and the GPU route plus a JPEG failed
//! outright on an exact seek to the in-point of a single-frame source. Both routes are
//! therefore measured, and the still is declined by `gpu_export` so both end up on the CPU
//! composition. The decline itself is asserted in
//! `gpu_export::tests::eligible_track_should_decline_a_still`, because `render` falls back
//! silently and a rendered file cannot tell the two routes apart.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::PathBuf;
use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_filter::FilterError;
use fixtures::{FileGuard, assets_dir, test_output_path, video_luma_per_frame};

/// The clip's length on the timeline, and the rate it is rendered at, so the expected
/// frame count is the product of the two.
const CLIP_SECS: f64 = 3.0;
const FPS: f64 = 30.0;
/// The fixtures are a bright drawing on white; the canvas behind them is black. Anything
/// in between separates "the picture is here" from "the background is showing".
const LUMA_FLOOR: f64 = 40.0;

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// The committed image fixtures: one lossless, one lossy, which is what the issue's
/// third criterion asks for. They differ in demuxer as well as in codec (`png_pipe`
/// against `image2`), and that difference is what made the first attempt at this fix
/// work for one and hang for the other.
fn image_fixtures() -> [(&'static str, PathBuf); 2] {
    [
        ("png", assets_dir().join("img/hello-triangle.png")),
        ("jpg", assets_dir().join("img/hello-triangle.jpg")),
    ]
}

/// Renders one still on one video track and returns its per-frame luma.
///
/// `force_cpu` names the route **asked for**, not the one that runs: a still is declined
/// by `gpu_export`, so the default route falls back to the CPU composition. Asking for
/// both is still worth it, because it is the default route that a caller uses and the
/// route that used to produce nothing at all.
fn render_still(tag: &str, src: &PathBuf, force_cpu: bool) -> Option<Vec<f64>> {
    let out = test_output_path(&format!("still_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(640, 360)
        .frame_rate(FPS)
        .video_track(vec![Clip::new(src).trim(Duration::ZERO, s(CLIP_SECS))])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    let config = EncoderConfig::builder().build();
    let rendered = if force_cpu {
        timeline.render_forcing_cpu(&out, config)
    } else {
        timeline.render(&out, config)
    };
    match rendered {
        Ok(()) => {}
        // The gate turns on the **reason**, not the variant: a composition that drops the
        // still still returns `Ok`, and a build with no filters reports `CompositionFailed`
        // for a reason these tests are not about, so skipping on the variant alone would
        // hide the defect too (RK-002).
        Err(TimelineError::Filter(FilterError::CompositionFailed { ref reason }))
            if reason.contains("filter not found") =>
        {
            println!("Skipping: this build lacks a filter the still needs: {reason}");
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
        Err(e) => panic!("render failed: {e}"),
    }

    match video_luma_per_frame(&out) {
        Some((_fps, luma)) => Some(luma),
        None => {
            println!("Skipping: cannot decode the rendered video here");
            None
        }
    }
}

/// Criterion 1 and 3. Every frame the clip covers carries picture, for a lossless and a
/// lossy format, on both routes.
#[test]
fn a_still_image_should_render_for_its_clips_duration() {
    let expected = (CLIP_SECS * FPS).round() as usize;
    for (format, src) in image_fixtures() {
        if !src.exists() {
            println!("Skipping: fixture not found at {}", src.display());
            return;
        }
        for (route, force_cpu) in [("forced-CPU", true), ("default", false)] {
            let tag = format!("{format}_{}", if force_cpu { "cpu" } else { "default" });
            let Some(luma) = render_still(&tag, &src, force_cpu) else {
                return;
            };
            assert_eq!(
                luma.len(),
                expected,
                "a {CLIP_SECS}s still must render {expected} frames on the {route} route"
            );
            // Not a count of lit frames: *which* frames carry picture is the whole of
            // this defect, and the one that used to survive was the first.
            let dark: Vec<usize> = luma
                .iter()
                .enumerate()
                .filter(|&(_, &v)| v <= LUMA_FLOOR)
                .map(|(i, _)| i)
                .collect();
            assert!(
                dark.is_empty(),
                "every frame of a held still must carry picture on the {route} route; \
                 {format} left {} of {expected} showing the canvas, starting at {:?}",
                dark.len(),
                dark.first()
            );
        }
    }
}

/// An **unbounded** still, which is the shape that hangs if a held frame is allowed to
/// outlive the thing that ends the export.
///
/// `Clip::new(img)` with no `trim` has no `out_point`, and a still's probed length is
/// zero, so `composition_end` establishes no length at all: the canvas runs forever. If
/// the still were held as well, nothing would end the graph and the render would never
/// return. Measured before this test existed: 60 seconds with no output, against a 33ms
/// single-frame file from the same timeline before #1802 was touched.
///
/// The assertion is deliberately weak about *what* comes out (an unbounded still has no
/// right length to assert) and strict about the render **returning at all**, which is the
/// property that broke. A regression here reports as a `timeout` row from
/// `cargo xtask test` rather than a failure, which is the only way a hang can surface.
#[test]
fn an_unbounded_still_should_not_hang_the_export() {
    let src = assets_dir().join("img/hello-triangle.png");
    if !src.exists() {
        println!("Skipping: fixture not found at {}", src.display());
        return;
    }
    let out = test_output_path("still_unbounded.mp4");
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(640, 360)
        .frame_rate(FPS)
        // No trim: the clip has no out-point, so the composition has no length either.
        .video_track(vec![Clip::new(&src)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };
    match timeline.render_forcing_cpu(&out, EncoderConfig::builder().build()) {
        Ok(()) => {}
        Err(TimelineError::Filter(FilterError::CompositionFailed { ref reason }))
            if reason.contains("filter not found") =>
        {
            println!("Skipping: this build lacks a filter the still needs: {reason}");
            return;
        }
        Err(TimelineError::Filter(FilterError::BuildFailed)) => {
            println!("Skipping: the graph could not be built here");
            return;
        }
        Err(ref e @ (TimelineError::Encode(_) | TimelineError::Decode(_))) => {
            println!("Skipping: this build cannot run the pipeline: {e}");
            return;
        }
        Err(e) => panic!("render failed: {e}"),
    }
    let Some(luma) = video_luma_per_frame(&out).map(|(_fps, l)| l) else {
        println!("Skipping: cannot decode the rendered video here");
        return;
    };
    assert!(
        !luma.is_empty(),
        "an unbounded still must still produce the frame it has"
    );
    assert!(
        luma[0] > LUMA_FLOOR,
        "and that frame must carry the picture, not the canvas"
    );
}
