//! A clip moved to a beat starts on that beat in the export (#1914).
//!
//! The point of musical time is that a host can say "beat 4" and get beat 4, so the only
//! check worth having decodes the render and counts samples. A duration is not evidence: a
//! clip a beat late produces an output of the same length.
//!
//! **The measurement is on the audio, and calibrated.** The sound is where the promise is
//! exact: a clip's picture is quantised to the output frame grid and can sit up to half a
//! frame from the beat (16.68 ms at 29.97 fps), while its sound is placed within one sample
//! (0.0208 ms at 48 kHz). `Timeline::position_of_beat` documents that split. So this
//! measures the onset sample, and takes the difference against a beat-0 control so the
//! tone's own rise time is not charged to the placement, the way
//! `audio_offset_precision.rs` does.
//!
//! The tempo is 174 BPM, where a beat is 344.827586... ms: neither a whole millisecond nor
//! a whole number of samples, so a placement has something to get wrong.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::PathBuf;
use std::time::Duration;

use avio::{Beats, Clip, Command, EncoderConfig, Rational, Tempo, Timeline, TimelineError, apply};
use ff_filter::FilterError;
use ff_format::AudioCodec;
use fixtures::{FileGuard, first_sound_sample, make_source_file, test_output_path, write_tone_wav};

/// 174 BPM. One beat is 60/174 s = 344.827586... ms.
fn tempo() -> Tempo {
    Tempo::new(Rational::new(174, 1)).expect("174 BPM is a tempo")
}

/// The rate every render here mixes to, and so the unit of the measurement.
const OUTPUT_RATE: u32 = 48_000;
/// Well above the noise a lossless path leaves in a silent lead-in, well below the tone.
const SOUND_FLOOR: f64 = 0.05;
const TONE_SECS: f64 = 1.0;
const FPS: f64 = 30.0;

fn beats(num: i32, den: i32) -> Beats {
    Beats::new(Rational::new(num, den))
}

/// A video source to hold the programme, and a lossless tone to hear the start of.
///
/// The tone is hand-written PCM so no lossy codec's pre-echo sits in front of the onset,
/// and it is its own clip because `make_source_file` writes silent audio.
fn sources(tag: &str) -> Option<(PathBuf, FileGuard, PathBuf, FileGuard)> {
    let video = test_output_path(&format!("bae_src_{tag}.mp4"));
    let gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, FPS, 150, 70, 90, 110)?;

    let tone = test_output_path(&format!("bae_tone_{tag}.wav"));
    let gt = FileGuard::new(tone.clone());
    write_tone_wav(&tone, OUTPUT_RATE, 2, 16, TONE_SECS);

    Some((video, gv, tone, gt))
}

/// Renders the tone placed on `beat` and returns the onset sample index.
///
/// `None` means this build cannot run the measurement, and the caller skips. The gate turns
/// on the **reason** rather than the variant: a misplaced clip still renders `Ok`, so
/// skipping on `CompositionFailed` alone would also skip the failure this test is for. This
/// build may have no filters at all, which is why the gate exists.
fn onset(tag: &str, beat: Beats) -> Option<u64> {
    let (video, _gv, tone, _gt) = sources(tag)?;
    let out = test_output_path(&format!("bae_out_{tag}.mkv"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(Rational::new(30, 1))
        .tempo(tempo())
        .video_track(vec![
            Clip::new(&video).trim(Duration::ZERO, Duration::from_secs(4)),
        ])
        .audio_track(vec![Clip::new(&tone)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    let id = timeline.audio_tracks()[0].clips[0].id;
    let moved = apply(&timeline, &Command::MoveClipToBeat { clip: id, beat })
        .expect("the clip exists and the timeline has a tempo");

    // FLAC so the onset is not blurred by a lossy codec's pre-echo, whose position depends
    // on the frame grid and so would differ between the two renders rather than cancelling.
    // Forced onto the CPU route so the measurement is of one compositor rather than
    // whichever one this machine happens to pick.
    let cfg = EncoderConfig::builder()
        .audio_codec(AudioCodec::Flac)
        .build();
    match moved.render_forcing_cpu(&out, cfg) {
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

    match first_sound_sample(&out, SOUND_FLOOR) {
        Some((sample, rate)) => {
            assert_eq!(
                rate, OUTPUT_RATE,
                "the mix target is {OUTPUT_RATE} Hz, so the measurement's unit is too"
            );
            Some(sample)
        }
        None => {
            println!("Skipping: cannot decode the rendered audio here");
            None
        }
    }
}

/// The placement with the tone's own rise time removed.
fn placed(tag: &str, beat: Beats) -> Option<u64> {
    let control = onset(&format!("{tag}_ctl"), Beats::zero())?;
    let moved = onset(&format!("{tag}_mv"), beat)?;
    assert!(
        moved >= control,
        "a clip moved forward cannot start before the same clip at beat 0: {moved} < {control}"
    );
    Some(moved - control)
}

/// The sample a beat names, from the model's own conversion, so the expectation and the
/// command cannot disagree by construction.
fn expected_sample(beat: Beats) -> u64 {
    let position = tempo().position_of(beat);
    (position.as_secs_f64() * f64::from(OUTPUT_RATE)).round() as u64
}

#[test]
fn a_clip_moved_to_a_beat_should_start_on_that_beat_in_the_export() {
    let beat = beats(4, 1);
    let Some(placed) = placed("four", beat) else {
        return;
    };
    let expected = expected_sample(beat);
    let drift = placed.abs_diff(expected);
    assert!(
        drift <= 1,
        "beat 4 is sample {expected}, measured {placed} ({drift} off)"
    );
}

/// A subdivision rather than a whole beat, which is where a placement that rounded to the
/// beat rather than to the position would show up.
#[test]
fn a_clip_moved_to_a_half_beat_should_start_there() {
    let beat = beats(7, 2);
    let Some(placed) = placed("sevenhalves", beat) else {
        return;
    };
    let expected = expected_sample(beat);
    let drift = placed.abs_diff(expected);
    assert!(
        drift <= 1,
        "beat 3.5 is sample {expected}, measured {placed} ({drift} off)"
    );
}

/// The half of the promise a build with no FFmpeg can still check: the command and the
/// read path agree without rendering anything.
#[test]
fn move_clip_to_beat_should_agree_with_position_of_beat_without_rendering() {
    let timeline = Timeline::builder()
        .canvas(160, 120)
        .frame_rate(Rational::new(30, 1))
        .tempo(tempo())
        .audio_track(vec![Clip::new(PathBuf::from("never-opened.wav"))])
        .build()
        .expect("a timeline whose clip is never opened still builds");
    let id = timeline.audio_tracks()[0].clips[0].id;

    for beat in [beats(1, 1), beats(7, 2), beats(1, 3), beats(1740, 1)] {
        let moved = apply(&timeline, &Command::MoveClipToBeat { clip: id, beat }).unwrap();
        assert_eq!(
            moved.audio_tracks()[0].clips[0].offset,
            timeline.position_of_beat(beat).unwrap(),
            "beat {}/{} must land where the timeline says it does",
            beat.count().num(),
            beat.count().den()
        );
    }
}
