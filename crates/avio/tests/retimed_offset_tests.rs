//! A retimed clip must start at its `offset`, not at `offset / speed` (#1804).
//!
//! `Clip::offset` is a position on the timeline, so a retime must not move it. The
//! export used to emit the placement before the retime in both domains, and
//! `setpts=PTS/factor` (video) and the `atempo` chain (audio) scale every timestamp
//! upstream of themselves, so the placement was scaled with the content. Video and
//! audio were wrong by the same factor, which is why A/V stayed in sync while both sat
//! in the wrong place.
//!
//! At `offset = 0` none of this is visible, which is how it survived until now, so
//! every test here places the clip somewhere other than the start.
//!
//! The renders are forced onto the CPU route because that is the only route a retimed
//! clip can take: `gpu_export` declines any clip whose speed is not 1.0, so `render()`
//! would fall back anyway. Saying so keeps the test honest about what it exercises
//! (RK-030).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::PathBuf;
use std::time::Duration;

use avio::{Clip, Command, EncoderConfig, Timeline, TimelineError};
use ff_filter::FilterError;
use fixtures::{
    FileGuard, first_sound_secs, first_visible_secs, make_source_file, test_output_path,
    video_luma_per_frame, write_tone_wav,
};

/// The clip is trimmed to 1s of source and run at 2x, so it contributes 0.5s.
const SOURCE_SECS: f64 = 1.0;
const SPEED: f64 = 2.0;
const CONTENT_SECS: f64 = SOURCE_SECS / SPEED;
/// One frame at 30 fps is 33ms. The audio start is read from the decoded stream and
/// the video start is derived from a container duration, so a frame either way is the
/// honest tolerance.
const TOLERANCE_SECS: f64 = 0.06;
/// Well above the noise an encoder leaves in a silent lead-in, well below the tone.
const SOUND_FLOOR: f64 = 0.05;
/// The fixture's own luma is 80 against a black canvas, so anything in between finds
/// the frame where the clip's picture begins.
const LUMA_FLOOR: f64 = 40.0;

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// A video source long enough to trim at 1s, and a 440 Hz tone of the same length.
///
/// `make_source_file` writes silent audio, so the tone is a separate clip on its own
/// track: a placement test has to hear where the audio starts.
fn sources(tag: &str) -> Option<(PathBuf, FileGuard, PathBuf, FileGuard)> {
    let video = test_output_path(&format!("retime_src_{tag}.mp4"));
    let gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, 30.0, 90, 80, 90, 120)?;

    let tone = test_output_path(&format!("retime_tone_{tag}.wav"));
    let gt = FileGuard::new(tone.clone());
    write_tone_wav(&tone, 48_000, 2, 16, 3.0);
    Some((video, gv, tone, gt))
}

fn clip_at(path: &PathBuf, offset: f64, speed: f64) -> Clip {
    Clip::new(path)
        .trim(s(SOURCE_SECS), s(SOURCE_SECS * 2.0))
        .offset(s(offset))
        .with_speed(speed)
}

/// Renders a **video-only** timeline and returns the time its picture first appears.
///
/// Not its duration: the composition's background canvas sets that (#1803), so a layer
/// placed at the wrong time still produces a file of exactly the right length, with
/// black where the picture should be. Video-only because a container's duration, and
/// anything derived from it, would otherwise follow whichever stream is longest.
fn render_video(tag: &str, offset: f64, speed: f64) -> Option<f64> {
    let (video, _gv, _tone, _gt) = sources(tag)?;
    let out = test_output_path(&format!("retime_v_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![clip_at(&video, offset, speed)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };
    measure(timeline, &out)?;
    first_visible_secs(&out, LUMA_FLOOR)
}

/// Renders an audio clip and returns the time its tone is first heard.
fn render_audio(tag: &str, offset: f64, speed: f64) -> Option<Option<f64>> {
    let (video, _gv, tone, _gt) = sources(tag)?;
    let out = test_output_path(&format!("retime_a_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        // A video track long enough to hold the whole programme, so the audio is not
        // cut short by the composition ending first.
        .video_track(vec![Clip::new(&video).trim(Duration::ZERO, s(3.0))])
        .audio_track(vec![clip_at(&tone, offset, speed)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };
    measure(timeline, &out)?;
    Some(first_sound_secs(&out, SOUND_FLOOR))
}

/// Renders on the CPU route and returns the output's duration in seconds.
///
/// The gate turns on the **reason**, not the variant: a misplaced clip still renders
/// `Ok`, and a composition that cannot be built reports `CompositionFailed` for a
/// reason this test is not about, so skipping on the variant alone would skip on real
/// failures too (RK-002).
fn measure(timeline: Timeline, out: &PathBuf) -> Option<f64> {
    match timeline.render_forcing_cpu(out, EncoderConfig::builder().build()) {
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
    match avio::open(out) {
        Ok(info) => Some(info.duration().as_secs_f64()),
        Err(e) => {
            println!("Skipping: cannot probe the rendered file here: {e}");
            None
        }
    }
}

/// The issue's own table: a 0.5s clip at three offsets, where the total is the offset
/// plus the content. The `offset = 0` row is the control that used to pass anyway.
#[test]
fn a_retimed_clip_should_start_at_its_offset_not_at_offset_over_speed() {
    for (tag, offset) in [("off0", 0.0), ("off05", 0.5), ("off10", 1.0)] {
        let Some(start) = render_video(tag, offset, SPEED) else {
            return;
        };
        assert!(
            (start - offset).abs() < TOLERANCE_SECS,
            "a {SPEED}x clip authored at {offset}s must appear at {offset}s, got {start}s"
        );
    }
}

/// The audio placement is a separate code path from the video one and was wrong in the
/// same way: `adelay` prepends silence and an `atempo` chain ahead of it compressed
/// that silence too. Both speeds are covered because the error is a division: a speed
/// below 1.0 pushed the clip late rather than early.
#[test]
fn a_retimed_clips_audio_should_start_at_its_offset() {
    for (tag, speed) in [("aud2x", 2.0), ("audhalf", 0.5)] {
        let Some(first_sound) = render_audio(tag, 1.0, speed) else {
            return;
        };
        let Some(start) = first_sound else {
            println!("Skipping: no audio could be decoded from the render here");
            return;
        };
        assert!(
            (start - 1.0).abs() < TOLERANCE_SECS,
            "a {speed}x clip at 1s must be heard at 1s, got {start}s"
        );
    }
}

/// The guard against fixing one domain and not the other. They were wrong by the same
/// factor, so A/V stayed in sync; correcting only one would desync a retimed clip.
#[test]
fn a_retimed_clips_video_and_audio_should_start_together() {
    let Some(video_start) = render_video("syncv", 1.0, SPEED) else {
        return;
    };
    let Some(first_sound) = render_audio("synca", 1.0, SPEED) else {
        return;
    };
    let Some(audio_start) = first_sound else {
        println!("Skipping: no audio could be decoded from the render here");
        return;
    };
    assert!(
        (audio_start - video_start).abs() < TOLERANCE_SECS,
        "video and audio must start together: video at {video_start}s, audio at {audio_start}s"
    );
}

/// Criterion 2. A razor is where this reaches an editor: the right-hand half is created
/// with a non-zero offset, so splitting a retimed clip used to place that half at
/// `at / speed`, overlapping its own left half and leaving a hole after the cut.
///
/// The length is asserted because the criterion asks for it, but the length alone
/// cannot see this any more: the composition's canvas fixes it whatever the layers do
/// (#1803). What sees it is the picture, so the frames across the cut are checked for
/// the black the misplacement would leave.
#[test]
fn splitting_a_retimed_clip_should_not_change_the_programme_length() {
    let Some((video, _gv, tone, _gt)) = sources("split") else {
        return;
    };
    let out_whole = test_output_path("retime_out_whole.mp4");
    let out_split = test_output_path("retime_out_split.mp4");
    let _gw = FileGuard::new(out_whole.clone());
    let _gs = FileGuard::new(out_split.clone());

    let build = || {
        Timeline::builder()
            .canvas(160, 120)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new(&video)
                    .trim(s(SOURCE_SECS), s(SOURCE_SECS * 2.0))
                    .offset(s(1.0))
                    .with_speed(SPEED),
            ])
            .audio_track(vec![
                Clip::new(&tone)
                    .trim(s(SOURCE_SECS), s(SOURCE_SECS * 2.0))
                    .offset(s(1.0))
                    .with_speed(SPEED),
            ])
            .build()
    };
    let Ok(whole) = build() else {
        println!("Skipping: Timeline::builder().build() failed");
        return;
    };
    let Ok(to_split) = build() else {
        return;
    };

    // Cut a quarter of the way into what the clip contributes on the timeline.
    let at = s(1.0 + CONTENT_SECS / 4.0);
    let clip_id = to_split.video_tracks()[0].clips[0].id;
    let split = match avio::apply(&to_split, &Command::SplitClip { clip: clip_id, at }) {
        Ok(t) => t,
        Err(e) => panic!("SplitClip failed: {e}"),
    };
    assert_eq!(
        split.video_tracks()[0].clips.len(),
        2,
        "the split must produce two clips"
    );

    let Some(before) = measure(whole, &out_whole) else {
        return;
    };
    let Some(after) = measure(split, &out_split) else {
        return;
    };
    assert!(
        (before - after).abs() < TOLERANCE_SECS,
        "a razor adds and removes nothing: {before}s before the split, {after}s after"
    );

    // Every frame the clip covers must still carry picture: a half placed at
    // `at / speed` lands early, overlapping its own sibling and leaving the tail of
    // the span black. The final frame is excluded because a separate defect drops it
    // for a retimed clip whatever the placement does (#1862): including it here would
    // make this test fail for a reason it is not about.
    let Some((fps, luma)) = video_luma_per_frame(&out_split) else {
        println!("Skipping: cannot decode the rendered video here");
        return;
    };
    let first = ((1.0 + 0.02) * fps).ceil() as usize;
    let last =
        (((1.0 + CONTENT_SECS - 0.02) * fps).floor() as usize).min(luma.len().saturating_sub(2));
    // An empty range would satisfy the assertion below without looking at anything,
    // so the span being non-empty is asserted first.
    assert!(
        first <= last,
        "the span to check must not be empty: frames {first}..={last} of {}",
        luma.len()
    );
    let dark: Vec<usize> = (first..=last).filter(|&i| luma[i] <= LUMA_FLOOR).collect();
    assert!(
        dark.is_empty(),
        "the split must leave no hole: frames {dark:?} of {} are black inside the clip's span",
        luma.len()
    );
}

/// Criterion 4. The preview never divided the offset, so this is what the export has
/// been brought into line with. Asserted through `to_scene`, which needs no `FFmpeg`
/// and so runs on a build that cannot render at all.
#[test]
fn the_preview_and_the_export_should_agree_on_where_a_retimed_clip_starts() {
    let timeline = Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::new("nonexistent.mp4")
                .trim(s(SOURCE_SECS), s(SOURCE_SECS * 2.0))
                .offset(s(1.0))
                .with_speed(SPEED),
        ])
        .build()
        .expect("a timeline whose clip is never opened still builds");

    let scene = timeline.to_scene();
    let placement = scene
        .video_tracks
        .first()
        .and_then(|layer| layer.placements.first())
        .expect("one video track holding one placement");
    assert_eq!(
        placement.offset,
        s(1.0),
        "the preview places a retimed clip at its authored offset, unscaled"
    );
    assert!(
        (placement.speed - SPEED).abs() < 1e-9,
        "and carries the speed separately, so only the duration is scaled"
    );
}
