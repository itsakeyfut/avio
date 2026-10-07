//! A clip moved to a frame starts on that frame in the export (#1827).
//!
//! The point of frame addressing is that a host can say "frame 1234" and get frame 1234,
//! so the only check worth having decodes the render and counts frames. A duration is not
//! evidence: a clip one frame late produces an output of the same length.
//!
//! **The measurement is calibrated.** The same timeline is rendered with the clip at frame
//! 0 and at the frame under test, and the difference in onset is the placement. That
//! cancels anything the source or the encoder contributes to the first visible frame, which
//! an absolute count would charge to the edit.
//!
//! The rate is a broadcast one, because that is where a placement has something to get
//! wrong: at 29.97 a frame is 33.3667 ms and neither a whole millisecond nor a whole
//! number of nanoseconds.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::PathBuf;
use std::time::Duration;

use avio::{Clip, Command, EncoderConfig, Timeline, TimelineError, apply};
use ff_filter::FilterError;
use ff_format::Rational;
use fixtures::{FileGuard, first_visible_frame, make_source_file, test_output_path};

/// 29.97. A frame is 33.3667 ms here, so an off-by-one is not hidden by any rounding that
/// happens to agree.
fn ntsc() -> Rational {
    Rational::new(30_000, 1001)
}

/// The fixture's own luma is 90 against a black canvas, so anything in between finds the
/// frame where the clip's picture begins.
const LUMA_FLOOR: f64 = 45.0;
/// Long enough that the programme still holds the clip after it is moved.
const SOURCE_FRAMES: usize = 120;

/// Renders a one-clip timeline with the clip starting on `frame`, and returns the index of
/// the first frame its picture appears on.
///
/// `None` means this build cannot run the measurement, and the caller skips. The gate turns
/// on the **reason** rather than the variant: a misplaced clip still renders `Ok`, so
/// skipping on `CompositionFailed` alone would also skip the failure this test exists to
/// catch. This build may have no filters at all, which is why the gate exists.
fn onset(tag: &str, frame: u64) -> Option<usize> {
    let src = test_output_path(&format!("fae_src_{tag}.mp4"));
    let _gs = FileGuard::new(src.clone());
    make_source_file(&src, 160, 120, ntsc().as_f64(), SOURCE_FRAMES, 90, 100, 110)?;

    let out = test_output_path(&format!("fae_out_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(ntsc())
        .video_track(vec![
            Clip::new(&src).trim(Duration::ZERO, Duration::from_secs(1)),
        ])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    let id = timeline.video_tracks()[0].clips[0].id;
    let moved = apply(&timeline, &Command::MoveClipToFrame { clip: id, frame })
        .expect("the clip exists, so the move is accepted");

    // Forced onto the CPU route so the measurement is of one compositor rather than
    // whichever one this machine happens to pick: `render` takes the GPU path when an
    // adapter is available, and a placement test should not depend on that.
    match moved.render_forcing_cpu(&out, EncoderConfig::builder().build()) {
        Ok(()) => {}
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
        Err(e) => panic!("render failed: {e}"),
    }

    match first_visible_frame(&out, LUMA_FLOOR) {
        Some(index) => Some(index),
        None => {
            println!("Skipping: cannot decode the rendered video here");
            None
        }
    }
}

/// The placement with whatever the source and encoder contribute removed.
fn placed(tag: &str, frame: u64) -> Option<usize> {
    let control = onset(&format!("{tag}_ctl"), 0)?;
    let moved = onset(&format!("{tag}_mv"), frame)?;
    assert!(
        moved >= control,
        "a clip moved forward cannot start before the same clip at frame 0: {moved} < {control}"
    );
    Some(moved - control)
}

#[test]
fn a_clip_moved_to_a_frame_should_start_on_that_frame_in_the_export() {
    let frame = 7u64;
    let Some(placed) = placed("seven", frame) else {
        return;
    };
    assert_eq!(
        placed, frame as usize,
        "the clip was moved to frame {frame} and the export starts it {placed} frames in"
    );
}

/// A second value, far enough in that an error proportional to the position would show up
/// where a constant one would not.
#[test]
fn the_frame_a_clip_is_moved_to_should_hold_further_into_the_timeline() {
    let frame = 61u64;
    let Some(placed) = placed("sixtyone", frame) else {
        return;
    };
    assert_eq!(placed, frame as usize);
}

/// The command and the read path have to agree without rendering anything, which is the
/// half of the promise a build with no FFmpeg can still check.
#[test]
fn move_clip_to_frame_should_agree_with_frame_at_without_rendering() {
    let timeline = Timeline::builder()
        .canvas(160, 120)
        .frame_rate(ntsc())
        .video_track(vec![Clip::new(PathBuf::from("never-opened.mp4"))])
        .build()
        .expect("a timeline whose clip is never opened still builds");
    let id = timeline.video_tracks()[0].clips[0].id;

    for frame in [1u64, 7, 61, 1800, 107_892] {
        let moved = apply(&timeline, &Command::MoveClipToFrame { clip: id, frame }).unwrap();
        assert_eq!(
            moved.frame_at(moved.video_tracks()[0].clips[0].offset),
            frame,
            "frame {frame} must read back as itself"
        );
    }
}
