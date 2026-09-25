//! `Clip::with_pitch` must shift the pitch on a timeline (#1817).
//!
//! `FilterStep::PitchShift` is compound: `asetrate` moves the pitch and the
//! duration together, and an `atempo` chain restores the duration. That
//! expansion lived only in the single-source builder, so on a timeline the
//! multi-track composition builder created a bare `asetrate` and every render
//! failed with `failed to apply effect ... filter=asetrate`.
//!
//! The assertions are on the measured frequency ratio against `2^(n/12)` rather
//! than on the render returning `Ok`, because a dropped step also returns `Ok`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

mod fixtures;

use std::path::PathBuf;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_filter::FilterError;
use fixtures::{
    FileGuard, dominant_hz, make_source_file, measure_audio, test_output_path, write_tone_wav,
};

/// The tone the fixtures carry, and the window used for every measurement.
const SOURCE_HZ: f64 = 440.0;
const WINDOW_START: f64 = 0.4;
const WINDOW_SECS: f64 = 0.25;
const TONE_SECS: f64 = 2.0;
/// The rate the mix runs at, which the timeline uses by default.
const MIX_RATE: u32 = 48_000;

/// Renders a 2 s tone with `semitones` of pitch shift.
///
/// `None` means this `FFmpeg` build could not take part: no encoder for the
/// video source, or no filters to build the graph with, which is CI's Linux
/// `FFmpeg` (`--disable-everything`). Any other failure panics, so the
/// `filter=asetrate` failure this test exists for cannot be mistaken for a
/// missing filter.
fn render_pitched(tag: &str, semitones: f64, source_rate: u32) -> Option<PathBuf> {
    let video = test_output_path(&format!("pitch_video_{tag}.mp4"));
    let _gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, 30.0, 60, 80, 90, 120)?;

    let tone = test_output_path(&format!("pitch_tone_{tag}.wav"));
    let _gt = FileGuard::new(tone.clone());
    write_tone_wav(&tone, source_rate, 2, 16, TONE_SECS);

    let out = test_output_path(&format!("pitch_out_{tag}.mp4"));
    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&video)])
        .audio_track(vec![Clip::new(&tone).with_pitch(semitones)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    match timeline.render(&out, EncoderConfig::builder().build()) {
        Ok(()) => Some(out),
        // The gate turns on the reason, not the variant. The defect these tests
        // guard against reports `CompositionFailed` too, with
        // "failed to apply effect ... filter=asetrate", so skipping on the
        // variant alone would skip on the very failure being tested.
        Err(TimelineError::Filter(FilterError::CompositionFailed { ref reason }))
            if reason.contains("filter not found") =>
        {
            println!("Skipping: this build lacks a filter the graph needs: {reason}");
            None
        }
        Err(TimelineError::Filter(FilterError::BuildFailed)) => {
            println!("Skipping: the graph could not be built here");
            None
        }
        Err(e) => panic!("render failed: {e}"),
    }
}

/// Measures the untouched source first. A reading far from 440 Hz means the
/// measurement is broken, which must not be read as a pitch result.
fn calibrate(tag: &str, source_rate: u32) -> Option<f64> {
    let tone = test_output_path(&format!("pitch_cal_{tag}.wav"));
    let _g = FileGuard::new(tone.clone());
    write_tone_wav(&tone, source_rate, 2, 16, TONE_SECS);
    let hz = dominant_hz(&tone, WINDOW_START, WINDOW_SECS)?;
    assert!(
        (hz - SOURCE_HZ).abs() < 15.0,
        "the measurement is unreliable here: a {SOURCE_HZ} Hz source read {hz} Hz"
    );
    Some(hz)
}

fn assert_shift(tag: &str, semitones: f64, source_rate: u32) {
    let Some(source_hz) = calibrate(tag, source_rate) else {
        println!("Skipping: cannot measure the source here");
        return;
    };
    let Some(out) = render_pitched(tag, semitones, source_rate) else {
        return;
    };
    let _g = FileGuard::new(out.clone());

    let Some(measured) = dominant_hz(&out, WINDOW_START, WINDOW_SECS) else {
        println!("Skipping: cannot decode the rendered audio here");
        return;
    };
    let expected_ratio = 2f64.powf(semitones / 12.0);
    let measured_ratio = measured / source_hz;
    assert!(
        (measured_ratio - expected_ratio).abs() < 0.05,
        "{semitones:+} semitones should give a ratio of {expected_ratio:.3}, \
         measured {measured_ratio:.3} ({measured:.0} Hz from {source_hz:.0} Hz)"
    );
}

#[test]
fn an_octave_up_should_double_the_frequency() {
    assert_shift("up12", 12.0, MIX_RATE);
}

#[test]
fn a_fifth_up_should_raise_the_frequency_by_the_expected_ratio() {
    assert_shift("up5", 5.0, MIX_RATE);
}

#[test]
fn a_fourth_down_should_lower_the_frequency_by_the_expected_ratio() {
    assert_shift("down5", -5.0, MIX_RATE);
}

/// The other half of the criterion: `asetrate` alone would shorten the clip by
/// the same factor it raises the pitch, and the `atempo` chain is what puts the
/// duration back. A frequency check alone would pass on a clip half as long.
#[test]
fn a_pitch_shift_should_not_change_the_clip_duration() {
    let Some(out) = render_pitched("duration", 12.0, MIX_RATE) else {
        return;
    };
    let _g = FileGuard::new(out.clone());

    let Some((_peak, _rms, secs)) = measure_audio(&out) else {
        println!("Skipping: cannot decode the rendered audio here");
        return;
    };
    assert!(
        (secs - TONE_SECS).abs() < 0.35,
        "an octave up must not shorten the clip: expected about {TONE_SECS} s, got {secs} s"
    );
}

/// `asetrate` replaces the rate it finds rather than scaling it, so the chain has
/// to actually be at the mix rate before one is computed from the mix rate. A
/// 44.1 kHz source in a 48 kHz mix is the case that catches a missing
/// normalisation: it shifted by an extra 48000/44100, about 1.4 semitones on top
/// of the octave, and every test at the mix rate stayed green.
#[test]
fn a_source_below_the_mix_rate_should_still_shift_by_the_requested_interval() {
    assert_shift("rate44k", 12.0, 44_100);
}
