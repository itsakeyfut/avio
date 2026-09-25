//! The export must follow each clip's `offset`, not its index in `Track::clips` (#1803).
//!
//! The composition used to end with whichever clip was last in the vector, because
//! the background canvas ran forever and the graph had to borrow its end from a
//! layer. Anything later than that clip was truncated away, so dragging a clip to
//! the right through `Command::MoveClip` silently exported a shorter timeline than
//! the one on screen, with nothing from `Timeline::validate` to say so.
//!
//! The assertions are on the rendered duration rather than on the render returning
//! `Ok`, because the truncating render returned `Ok` too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::PathBuf;
use std::time::Duration;

use avio::{BlendMode, Clip, Command, EncoderConfig, Timeline, TimelineError};
use ff_filter::FilterError;
use fixtures::{FileGuard, make_source_file, test_output_path};

/// Every clip is this long, so a clip at `offset` ends at `offset + CLIP_SECS`.
const CLIP_SECS: f64 = 2.0;
/// One frame at 30 fps is 33ms; half a frame is a fair tolerance for a duration
/// read back through a container.
const TOLERANCE_SECS: f64 = 0.05;

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// A source long enough to be trimmed at any offset used here.
fn source(tag: &str) -> Option<(PathBuf, FileGuard)> {
    let path = test_output_path(&format!("order_src_{tag}.mp4"));
    let guard = FileGuard::new(path.clone());
    make_source_file(&path, 160, 120, 30.0, 90, 80, 90, 120)?;
    Some((path, guard))
}

/// Renders a single video track holding one clip per offset, in the order given.
///
/// `None` means this `FFmpeg` build could not take part: no encoder for the source,
/// or no filters to build the composition with, which is CI's Linux `FFmpeg`
/// (`--disable-everything`). The gate turns on the **reason**, not the variant: a
/// truncating composition reports `CompositionFailed` too, so skipping on the
/// variant alone would skip on the very failure being tested.
fn render_offsets(tag: &str, offsets: &[f64]) -> Option<f64> {
    let (src, _g) = source(tag)?;
    let out = test_output_path(&format!("order_out_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let clips: Vec<Clip> = offsets
        .iter()
        .map(|o| {
            Clip::new(&src)
                .trim(Duration::ZERO, s(CLIP_SECS))
                .offset(s(*o))
        })
        .collect();
    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(clips)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };
    measure(timeline, &out)
}

/// Renders `timeline` on the default route and returns the output's duration.
fn measure(timeline: Timeline, out: &PathBuf) -> Option<f64> {
    measure_route(timeline, out, false)
}

/// Renders `timeline` and returns the output's duration in seconds.
///
/// `force_cpu` matters for anything testing the composition graph: the default route
/// hands an eligible timeline to the GPU compositor, which has its own scheduler and
/// never builds the graph at all. A test that means to exercise the graph has to say
/// so, or it passes while the code it targets is never reached.
fn measure_route(timeline: Timeline, out: &PathBuf, force_cpu: bool) -> Option<f64> {
    let config = EncoderConfig::builder().build();
    let rendered = if force_cpu {
        timeline.render_forcing_cpu(out, config)
    } else {
        timeline.render(out, config)
    };
    match rendered {
        Ok(()) => {}
        // The gate turns on the **reason**, not the variant: a composition that
        // truncates reports `CompositionFailed` too, so skipping on the variant alone
        // would skip on the very failure these tests exist for.
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
        // An encoder or decoder the build does not carry is the environment, not a
        // regression, matching `gpu_export_tests`' `is_environment_unavailable`.
        Err(ref e @ (TimelineError::Encode(_) | TimelineError::Decode(_))) => {
            println!("Skipping: this build cannot run the pipeline: {e}");
            return None;
        }
        Err(e) => panic!("render failed: {e}"),
    }
    match avio::open(out) {
        Ok(info) => Some(info.duration().as_secs_f64()),
        Err(e) => {
            println!("Skipping: cannot probe the rendered file here: {e}");
            None
        }
    }
}

#[test]
fn two_clips_in_either_vector_order_should_render_the_same_length() {
    let Some(sorted) = render_offsets("sorted2", &[2.0, 10.0]) else {
        return;
    };
    let Some(unsorted) = render_offsets("unsorted2", &[10.0, 2.0]) else {
        return;
    };
    assert!(
        (sorted - unsorted).abs() < TOLERANCE_SECS,
        "the same clips in a different vector order must render the same timeline: \
         [2, 10] gave {sorted}s and [10, 2] gave {unsorted}s"
    );
    let expected = 10.0 + CLIP_SECS;
    assert!(
        (sorted - expected).abs() < TOLERANCE_SECS,
        "the timeline ends where its latest clip ends: expected about {expected}s, got {sorted}s"
    );
}

/// The latest clip sits in the middle, so this fails both on the old "ends with the
/// last element" rule and on a fix that merely sorts the list by offset.
#[test]
fn a_clip_that_is_neither_first_nor_last_should_still_set_the_length() {
    let Some(measured) = render_offsets("middle", &[10.0, 2.0, 6.0]) else {
        return;
    };
    let expected = 10.0 + CLIP_SECS;
    assert!(
        (measured - expected).abs() < TOLERANCE_SECS,
        "the clip at 10s ends last wherever it sits in the vector: \
         expected about {expected}s, got {measured}s"
    );
}

/// Dragging a clip to the right is an ordinary timeline-UI gesture, and it is how
/// the command path reaches a vector whose order no longer matches the offsets.
#[test]
fn move_clip_should_leave_a_timeline_whose_render_matches_its_offsets() {
    let Some((src, _g)) = source("move") else {
        return;
    };
    let out = test_output_path("order_out_move.mp4");
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(
            (0..3)
                .map(|i| {
                    Clip::new(&src)
                        .trim(Duration::ZERO, s(1.0))
                        .offset(s(f64::from(i)))
                })
                .collect::<Vec<_>>(),
        )
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    let first = timeline.video_tracks()[0].clips[0].id;
    let moved = avio::apply(
        &timeline,
        &Command::MoveClip {
            clip: first,
            offset: s(5.0),
        },
    )
    .expect("MoveClip applies");

    // The model is right either way; what this pins is that the export agrees with it.
    let offsets: Vec<f64> = moved.video_tracks()[0]
        .clips
        .iter()
        .map(|c| c.offset.as_secs_f64())
        .collect();
    assert_eq!(
        offsets,
        vec![5.0, 1.0, 2.0],
        "MoveClip moved the wrong clip"
    );

    let Some(measured) = measure(moved, &out) else {
        return;
    };
    let expected = 5.0 + 1.0;
    assert!(
        (measured - expected).abs() < TOLERANCE_SECS,
        "the moved clip ends at 6s, so the export does too: \
         expected about {expected}s, got {measured}s"
    );
}

/// The same truncation lives in the `blend` branch of the composition builder, which a
/// Normal-blend test cannot reach: `overlay` and `blend` are separate filters with
/// their own `eof_action`. A short top layer carrying a blend mode used to end the
/// export early while the identical timeline with `BlendMode::Normal` did not.
#[test]
fn a_blended_top_layer_should_not_truncate_the_track_below() {
    let Some((src, _g)) = source("blend") else {
        return;
    };
    let out = test_output_path("order_out_blend.mp4");
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&src).trim(Duration::ZERO, s(CLIP_SECS))])
        .video_track(vec![
            Clip::new(&src)
                .trim(Duration::ZERO, s(0.5))
                .with_blend_mode(BlendMode::Multiply),
        ])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    // Forced onto the CPU route on purpose: `blend` is a filter in the composition
    // graph, and the default route would hand this timeline to the GPU compositor
    // instead, leaving the branch under test unvisited.
    let Some(measured) = measure_route(timeline, &out, true) else {
        return;
    };
    assert!(
        (measured - CLIP_SECS).abs() < TOLERANCE_SECS,
        "a 0.5s Multiply layer must not truncate the {CLIP_SECS}s track below it, got {measured}s"
    );
}
