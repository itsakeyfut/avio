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
/// The timeline rate every render here uses. Held once because the expected frame
/// counts are computed from it as well as the timeline being built with it.
const FPS: f64 = 30.0;
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

/// What a rendered video-only timeline actually holds.
///
/// Neither quantity can be read from a duration. The canvas is generated for a whole
/// number of frames; a layer fills a whole number of them; and when the two are derived
/// by different roundings the canvas gets a slot no layer reaches, which renders as
/// background (#1862). Counting the frames is the only way to see any of it (RK-031).
struct Rendered {
    /// How many frames the canvas was generated for.
    frames: usize,
    /// How many of them carry picture.
    lit: usize,
    /// The first and last frame carrying picture. **Where** the picture sits is the
    /// property this issue is about, and a count cannot see it: a layer placed one slot
    /// early keeps its count while leaving the final frame black, which is the same
    /// "the two rules disagree" defect in the other direction.
    first_lit: usize,
    last_lit: usize,
}

/// Renders a video-only timeline of the given clips.
fn render_clips(tag: &str, clips: Vec<Clip>) -> Option<Rendered> {
    let out = test_output_path(&format!("retime_f_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(FPS)
        .video_track(clips)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };
    measure(timeline, &out)?;
    let (_fps, luma) = video_luma_per_frame(&out)?;
    let lit: Vec<usize> = luma
        .iter()
        .enumerate()
        .filter(|&(_, &v)| v > LUMA_FLOOR)
        .map(|(i, _)| i)
        .collect();
    if lit.is_empty() {
        panic!(
            "the render carried no picture at all: {} frames",
            luma.len()
        );
    }
    Some(Rendered {
        frames: luma.len(),
        lit: lit.len(),
        first_lit: lit[0],
        last_lit: lit[lit.len() - 1],
    })
}

/// One clip, which is all it takes to reproduce #1862.
fn render_frames(tag: &str, in_pt: f64, out_pt: f64, offset: f64, speed: f64) -> Option<Rendered> {
    let (video, _gv, _tone, _gt) = sources(tag)?;
    render_clips(
        tag,
        vec![
            Clip::new(&video)
                .trim(s(in_pt), s(out_pt))
                .offset(s(offset))
                .with_speed(speed),
        ],
    )
}

/// An offset between two frames still has to land on one, and the composition's length
/// has to agree about which. When the two were derived separately the canvas ended a slot
/// past the layer and that slot rendered as background: one black frame at the end of a
/// retimed clip, with the programme keeping its length so nothing reported it (#1862).
///
/// Both conditions are needed, so unity speed is covered as the control: at 1.0 no offset
/// reproduced it.
#[test]
fn a_retimed_clip_at_an_offgrid_offset_should_keep_its_last_frame() {
    // 30 fps, so an offset is on the grid when `offset * 30` is a whole number. The
    // fractional part has to be sampled on both sides of a half frame: rounding the
    // summed end agrees with rounding the parts whenever it is at or above 0.5, so a
    // case below 0.5 is the only one that tells the two apart.
    for (tag, offset, speed) in [
        ("grid1x", 1.0, 1.0),                  // 30.00 frames
        ("quarter1x", 1.0 + 1.0 / 120.0, 1.0), // 30.25, under half
        ("half1x", 1.0 + 1.0 / 60.0, 1.0),     // 30.50
        ("odd1x", 1.125, 1.0),                 // 33.75
        ("grid2x", 1.0, SPEED),
        ("quarter2x", 1.0 + 1.0 / 120.0, SPEED), // 30.25, under half
        ("threeq2x", 1.0 + 1.0 / 40.0, SPEED),   // 30.75
        ("half2x", 1.0 + 1.0 / 60.0, SPEED),     // 30.50
        ("odd2x", 1.125, SPEED),                 // 33.75
        ("odd2xb", 1.25, SPEED),                 // 37.50
    ] {
        let Some(r) = render_frames(tag, 0.0, SOURCE_SECS, offset, speed) else {
            return;
        };
        let expected_lit = (SOURCE_SECS / speed * FPS).ceil() as usize;
        assert_eq!(
            r.lit, expected_lit,
            "a {speed}x clip at {offset}s must keep every frame it contributes"
        );
        // The criterion is "every frame it covers, including the last", so the last
        // frame is asserted directly. The counts above cannot see it: a layer one slot
        // early keeps them both while the final frame renders as background.
        assert_eq!(
            r.last_lit,
            r.frames - 1,
            "the last frame must carry picture: {speed}x at {offset}s left it black"
        );
        let lead_in = (offset * FPS).round() as usize;
        assert_eq!(
            r.first_lit, lead_in,
            "and the picture must start on the frame the offset lands on"
        );
        assert_eq!(
            r.frames,
            lead_in + expected_lit,
            "the canvas must end where the clip does: got {} frames for {} lit",
            r.frames,
            r.lit
        );
    }
}

/// The regression the obvious fix would cause. A clip's contribution is not always a
/// whole number of frames, and one that is 13.5 or 7.5 frames long legitimately occupies
/// 14 or 8. Rounding the composition's end down would take one of those away, which is
/// why the rounding is applied to the offset and the content separately rather than to
/// their sum.
#[test]
fn a_clip_contributing_a_fraction_of_a_frame_should_keep_all_of_them() {
    for (tag, out_pt, expected_lit) in [
        ("frac135", 0.9, 14),      // 0.45s at 2x -> 13.5 frames
        ("frac75", 0.5, 8),        // 0.25s at 2x -> 7.5 frames
        ("frac03", 1.0 / 45.0, 1), // a third of a frame still occupies one
    ] {
        let Some(r) = render_frames(tag, 0.0, out_pt, 1.0, SPEED) else {
            return;
        };
        assert_eq!(
            r.lit,
            expected_lit,
            "a clip contributing {} frames must occupy {expected_lit}",
            out_pt / SPEED * FPS
        );
        assert_eq!(
            r.frames,
            30 + expected_lit,
            "and the canvas must hold exactly those, after the lead-in"
        );
        assert_eq!(
            r.last_lit,
            r.frames - 1,
            "with the picture running to the final frame"
        );
    }
}

/// Clips that tile the timeline are the shape the GPU route accepts, and the rounding is
/// applied per clip, so the sum of the parts is not the rounding of the sum: two clips of
/// 13.5 frames each occupy 14 + 14 = 28 frames, where rounding their combined 27 frames
/// would give 27 and take one away. Both halves of a razored retimed clip have this shape,
/// so it is the case the fix has to get right for an editor, and a single clip cannot see
/// it.
#[test]
fn tiled_retimed_clips_should_each_keep_their_fractional_frame() {
    let Some((video, _gv, _tone, _gt)) = sources("tiled") else {
        return;
    };
    // 0.9s of source at 2x is 0.45s on the timeline, which is 13.5 frames: each clip
    // needs 14, and the second starts where the first ended.
    let half = |in_pt: f64, offset: f64| {
        Clip::new(&video)
            .trim(s(in_pt), s(in_pt + 0.9))
            .offset(s(offset))
            .with_speed(SPEED)
    };
    let Some(r) = render_clips("tiled", vec![half(0.0, 0.0), half(1.0, 0.45)]) else {
        return;
    };
    assert_eq!(
        r.frames, 28,
        "each clip occupies the frame its fraction needs: 14 + 14, not 27"
    );
    assert_eq!(
        r.last_lit,
        r.frames - 1,
        "and the second clip must reach the final frame"
    );
    assert_eq!(r.first_lit, 0, "the first clip starts the programme");
    assert_eq!(
        r.lit, r.frames,
        "no slot between or after them may render as background"
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
    // the span black. The final frame is included: it used to be excluded because a
    // retimed clip lost it whatever the placement did, which is #1862 and is fixed.
    let Some((fps, luma)) = video_luma_per_frame(&out_split) else {
        println!("Skipping: cannot decode the rendered video here");
        return;
    };
    let first = ((1.0 + 0.02) * fps).ceil() as usize;
    let last =
        (((1.0 + CONTENT_SECS - 0.02) * fps).floor() as usize).min(luma.len().saturating_sub(1));
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
