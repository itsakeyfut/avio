//! A retimed clip's audio must be as long as its video (#1863).
//!
//! The mixer implements `speed` as `asetrate` + `aresample`, and that chain began with
//! `apad=pad_dur=1`. `asetrate` scales everything upstream of it, including the padded
//! second, so the output ran `(content + 1) / speed` instead of `content / speed`: a one
//! second clip came out four seconds long at 0.5x, and unchanged at 2x, which is the
//! coincidence that made the defect hard to read.
//!
//! The pad existed to give SWR's resampler input to flush at EOF, guarding against NaN
//! at high downsampling ratios. `aeval` still sanitises NaN and Inf downstream, and it
//! is the guard that does not distort timing, so the risk this change takes is that the
//! pad was load-bearing after all. `a_silent_clip_at_a_high_speed_should_stay_finite`
//! is the test that guards it.
//!
//! Lengths are read from the decoded stream, never from the container's duration, which
//! follows whichever stream is longest and so cannot see any of this.
//!
//! The renders are forced onto the CPU route because it is the only route a retimed clip
//! can take: `gpu_export` declines any clip whose speed is not 1.0.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::PathBuf;
use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_filter::FilterError;
use fixtures::{
    FileGuard, make_source_file, measure_audio, test_output_path, video_luma_per_frame,
    write_silence_wav, write_tone_wav,
};

/// One second of source is trimmed out of every fixture, so the expected output is
/// `SOURCE_SECS / speed`.
const SOURCE_SECS: f64 = 1.0;
/// One AAC frame is 1024 samples, about 21 ms at 48 kHz, and the encoder cannot emit
/// less than a whole one. A little over two frames of slack keeps the assertions honest
/// without letting the defect through: the smallest error it produced was 0.5s at 2x,
/// ten times this.
const TOLERANCE_SECS: f64 = 0.05;

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// A video source and a tone long enough to trim a second out of the middle of.
///
/// The tone is its own clip because `make_source_file` writes silent audio, and a length
/// measured off silence would not prove the samples arrived.
fn sources(tag: &str) -> Option<(PathBuf, FileGuard, PathBuf, FileGuard)> {
    let video = test_output_path(&format!("raud_src_{tag}.mp4"));
    let gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, 30.0, 120, 80, 90, 120)?;

    let tone = test_output_path(&format!("raud_tone_{tag}.wav"));
    let gt = FileGuard::new(tone.clone());
    write_tone_wav(&tone, 48_000, 2, 16, 4.0);
    Some((video, gv, tone, gt))
}

/// Renders on the CPU route, or reports why this build cannot take part.
///
/// One place, so the gate's conditions cannot drift apart between the tests below.
fn render_or_skip(timeline: Timeline, out: &PathBuf) -> Option<()> {
    match timeline.render_forcing_cpu(out, EncoderConfig::builder().build()) {
        Ok(()) => Some(()),
        // The gate turns on the **reason**, not the variant: audio of the wrong length
        // still renders `Ok`, and a build with no filters reports `CompositionFailed`
        // for a reason these tests are not about, so skipping on the variant alone
        // would hide real failures too.
        Err(TimelineError::Filter(FilterError::CompositionFailed { ref reason }))
            if reason.contains("filter not found") =>
        {
            println!("Skipping: this build lacks a filter the chain needs: {reason}");
            None
        }
        Err(TimelineError::Filter(FilterError::BuildFailed)) => {
            println!("Skipping: the graph could not be built here");
            None
        }
        Err(ref e @ (TimelineError::Encode(_) | TimelineError::Decode(_))) => {
            println!("Skipping: this build cannot run the pipeline: {e}");
            None
        }
        Err(e) => panic!("render failed: {e}"),
    }
}

/// Renders one retimed audio clip and returns `(peak, decoded seconds)`.
///
/// A video track long enough to outlast the audio is included so the composition does
/// not end before the audio does, which would measure the canvas rather than the clip.
fn render_audio(tag: &str, speed: f64) -> Option<(f64, f64)> {
    let (video, _gv, tone, _gt) = sources(tag)?;
    let out = test_output_path(&format!("raud_out_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&video).trim(Duration::ZERO, s(4.0))])
        .audio_track(vec![
            Clip::new(&tone)
                .trim(s(SOURCE_SECS), s(SOURCE_SECS * 2.0))
                .with_speed(speed),
        ])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    render_or_skip(timeline, &out)?;

    match measure_audio(&out) {
        Some((peak, _rms, secs)) => Some((peak, secs)),
        None => {
            println!("Skipping: cannot decode the rendered audio here");
            None
        }
    }
}

/// Both sides of 1.0, and a speed outside the range a single `atempo` covers, because the
/// error was a division: above 1.0 it left the clip too long, below 1.0 it multiplied.
#[test]
fn a_retimed_clips_audio_should_be_as_long_as_the_speed_says() {
    for (tag, speed) in [
        ("half", 0.5),
        ("onehalf", 1.5),
        ("double", 2.0),
        ("quad", 4.0),
    ] {
        let Some((peak, secs)) = render_audio(tag, speed) else {
            return;
        };
        let expected = SOURCE_SECS / speed;
        assert!(
            (secs - expected).abs() < TOLERANCE_SECS,
            "{SOURCE_SECS}s of source at {speed}x must last {expected}s, got {secs}s"
        );
        assert!(
            peak > 0.1,
            "the measurement must be reading the tone, not silence: peak {peak}"
        );
    }
}

/// The audio is what the video already was: a clip at 2x contributes half its source to
/// the programme. A retimed clip used to desync from itself because only one of them
/// followed the speed.
#[test]
fn a_retimed_clips_audio_and_video_should_cover_the_same_span() {
    let Some((video, _gv, tone, _gt)) = sources("span") else {
        return;
    };
    let out = test_output_path("raud_out_span.mp4");
    let _go = FileGuard::new(out.clone());

    let clip = |path: &PathBuf| {
        Clip::new(path)
            .trim(s(SOURCE_SECS), s(SOURCE_SECS * 2.0))
            .with_speed(2.0)
    };
    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![clip(&video)])
        .audio_track(vec![clip(&tone)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };
    if render_or_skip(timeline, &out).is_none() {
        return;
    }

    // Both spans are counted off their own decoded streams. The container's duration is
    // not usable here: it follows whichever stream is longest, so with audio too long it
    // reports the audio and the test would compare the audio against itself (RK-031).
    let Some((fps, luma)) = video_luma_per_frame(&out) else {
        println!("Skipping: cannot decode the rendered video here");
        return;
    };
    let video_secs = luma.len() as f64 / fps;
    let Some((_peak, _rms, audio_secs)) = measure_audio(&out) else {
        println!("Skipping: cannot decode the rendered audio here");
        return;
    };
    assert!(
        (video_secs - audio_secs).abs() < TOLERANCE_SECS,
        "a 2x clip's video and audio must cover the same span: \
         video {video_secs}s, audio {audio_secs}s"
    );
}

/// The risk the pad removal takes, made into a test.
///
/// The pad was there so SWR had input to flush at EOF; without it a high downsampling
/// ratio was reported to produce NaN, and silent input is the case the sibling `atempo`
/// path calls out as its own NaN trigger.
///
/// The check that sees a NaN is the **rms**, not the peak. `measure_audio` builds its
/// peak with `f64::max`, which ignores NaN and so would return the running maximum
/// unchanged; the rms sums squares, so a single NaN carries through to the result. The
/// peak is asserted as well because it does catch `Inf`.
///
/// **This test could not be made to fail.** The NaN it guards against did not reproduce
/// on this build with the pad or without it, at any of the ratios measured. It is a
/// tripwire for a regression rather than a verified guard, and the mutation test that
/// pins the length assertions says nothing about this one.
#[test]
fn a_silent_clip_at_a_high_speed_should_stay_finite() {
    let Some((video, _gv, _tone, _gt)) = sources("hush") else {
        return;
    };
    let silent = test_output_path("raud_hush.wav");
    let _gs = FileGuard::new(silent.clone());
    write_silence_wav(&silent, 48_000, 4.0);

    let out = test_output_path("raud_out_hush.mp4");
    let _go = FileGuard::new(out.clone());
    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&video).trim(Duration::ZERO, s(1.0))])
        .audio_track(vec![
            Clip::new(&silent)
                .trim(s(SOURCE_SECS), s(SOURCE_SECS * 2.0))
                .with_speed(150.0),
        ])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };
    if render_or_skip(timeline, &out).is_none() {
        return;
    }
    let Some((peak, rms, _secs)) = measure_audio(&out) else {
        println!("Skipping: cannot decode the rendered audio here");
        return;
    };
    assert!(
        peak.is_finite() && rms.is_finite(),
        "no sample may be NaN or Inf after the resampler: peak {peak}, rms {rms}"
    );
}
