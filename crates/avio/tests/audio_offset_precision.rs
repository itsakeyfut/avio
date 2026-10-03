//! A clip's audio must start on the sample its `offset` names, not on the nearest
//! millisecond (#1915).
//!
//! The export lost the sub-millisecond part of `Clip::offset` twice. The model
//! truncated it (`as_millis()`, against `ATrim`'s `as_secs_f64()` a dozen lines
//! above), and the `adelay` argument was a decimal millisecond count, which
//! `af_adelay.c` reads through a C `float` and so cannot resolve to a sample in
//! material of any length. The fix carries full precision through the model and
//! converts to samples where the rate is known, emitting `delays=NNNS`.
//!
//! **The preview is the baseline, not a second opinion.** `ff-preview` never converts
//! an offset to milliseconds: it carries `Duration`s from `SceneAudioPlacement` all
//! the way through the runner's comparisons. So the two paths disagreed because the
//! export rounded, and the first test here asserts the preview's side through
//! `to_scene`, which needs no `FFmpeg` and therefore runs everywhere.
//!
//! **The export measurement is calibrated.** A 440 Hz tone rises from zero, so onset
//! detection lands a few samples after the true start. Rendering the same timeline at
//! `offset = 0` and at the offset under test and taking the difference cancels that
//! rise; an absolute measurement would charge it to the placement. The source is a
//! hand-written PCM WAV so no lossy codec's pre-echo sits in front of the onset, and
//! the output is FLAC for the same reason.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::path::PathBuf;
use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_filter::FilterError;
use ff_format::AudioCodec;
use fixtures::{FileGuard, first_sound_sample, make_source_file, test_output_path, write_tone_wav};

/// 500.5 ms. Not a whole number of milliseconds, so `as_millis()` truncated it to
/// 500 ms: a 0.5 ms error, which is 24 samples at 48 kHz. At 48 kHz the sample count
/// is 24024 exactly, so the expectation carries no rounding of its own.
const OFFSET_NANOS: u64 = 500_500_000;
/// The rate every render here mixes to, and therefore the rate of the measurement.
/// `MultiTrackAudioMixer` is constructed with it in `Timeline::render`.
const OUTPUT_RATE: u32 = 48_000;
/// Long enough that the offset clip is well inside the programme.
const TONE_SECS: f64 = 1.0;
const FPS: f64 = 30.0;
/// Well above the noise a lossless path leaves in a silent lead-in, well below the
/// tone's half scale.
const SOUND_FLOOR: f64 = 0.05;

fn offset() -> Duration {
    Duration::from_nanos(OFFSET_NANOS)
}

/// The sample the offset names at the output rate: 24024 at 48 kHz.
fn expected_samples() -> u64 {
    (offset().as_secs_f64() * f64::from(OUTPUT_RATE)).round() as u64
}

/// A video source to hold the programme, and a tone at `rate` to hear the start of.
///
/// The video track exists for the reason `retimed_offset_tests.rs` gives: without one
/// long enough, the composition ends before the offset audio does and the onset is
/// cut off rather than measured. `make_source_file` writes silent audio, so the tone
/// is its own clip on its own track.
fn sources(tag: &str, rate: u32) -> Option<(PathBuf, FileGuard, PathBuf, FileGuard)> {
    let video = test_output_path(&format!("aoff_src_{tag}.mp4"));
    let gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, FPS, 90, 70, 90, 110)?;

    let tone = test_output_path(&format!("aoff_tone_{tag}.wav"));
    let gt = FileGuard::new(tone.clone());
    write_tone_wav(&tone, rate, 2, 16, TONE_SECS);

    Some((video, gv, tone, gt))
}

/// Renders the tone at `off` and returns the onset sample index in the output.
///
/// `None` means this build cannot run the measurement, and the caller skips. The gate
/// turns on the **reason** rather than the variant: a misplaced clip still renders
/// `Ok`, so skipping on `CompositionFailed` alone would also skip the failure this
/// test exists to catch (RK-002).
fn onset(tag: &str, rate: u32, off: Duration) -> Option<u64> {
    let (video, _gv, tone, _gt) = sources(tag, rate)?;
    let out = test_output_path(&format!("aoff_out_{tag}.mkv"));
    let _go = FileGuard::new(out.clone());

    let timeline = match Timeline::builder()
        .canvas(160, 120)
        .frame_rate(FPS)
        .video_track(vec![
            Clip::new(&video).trim(Duration::ZERO, Duration::from_secs(3)),
        ])
        .audio_track(vec![Clip::new(&tone).offset(off)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    // FLAC so the onset is not blurred by a lossy codec's pre-echo, whose position
    // depends on the frame grid and so would differ between the two renders rather
    // than cancelling. Forced onto the CPU route because that is where the audio
    // graph this exercises is built, said out loud rather than relied on (RK-030).
    let cfg = EncoderConfig::builder()
        .audio_codec(AudioCodec::Flac)
        .build();
    match timeline.render_forcing_cpu(&out, cfg) {
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
        Some((sample, measured_rate)) => {
            assert_eq!(
                measured_rate, OUTPUT_RATE,
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

/// The difference between the offset render and the `offset = 0` control, which is the
/// placement with the tone's own rise time removed.
fn placed_samples(tag: &str, rate: u32) -> Option<u64> {
    let control = onset(&format!("{tag}_ctl"), rate, Duration::ZERO)?;
    let placed = onset(&format!("{tag}_off"), rate, offset())?;
    assert!(
        placed >= control,
        "an offset clip cannot start before the same clip at offset zero: {placed} < {control}"
    );
    Some(placed - control)
}

/// The preview's side, and the reference the export is measured against. Asserted
/// through `to_scene`, so it needs no `FFmpeg` and runs on a build that cannot render.
///
/// Gated on the feature rather than through `required-features` on the whole target,
/// because the export measurements below need no preview and a target-level gate would
/// stop them building at all on a default-feature check.
#[cfg(feature = "preview")]
#[test]
fn the_preview_should_place_a_sub_millisecond_offset_exactly() {
    let timeline = Timeline::builder()
        .canvas(160, 120)
        .frame_rate(FPS)
        .audio_track(vec![Clip::new("nonexistent.wav").offset(offset())])
        .build()
        .expect("a timeline whose clip is never opened still builds");

    let scene = timeline.to_scene();
    let placement = scene
        .audio_tracks
        .first()
        .and_then(|track| track.placements.first())
        .expect("one audio track holding one placement");
    assert_eq!(
        placement.offset,
        offset(),
        "the preview carries the authored offset verbatim, to the nanosecond"
    );
}

/// Criterion: the export places the clip on the sample its offset names. The source is
/// already at the mix rate, so nothing resamples and one sample is the honest
/// tolerance: the only residue is the onset detector landing on the same relative
/// sample in both renders.
#[test]
fn the_export_should_place_a_clip_on_the_sample_its_offset_names() {
    let Some(placed) = placed_samples("native", OUTPUT_RATE) else {
        return;
    };
    let expected = expected_samples();
    let drift = placed.abs_diff(expected);
    assert!(
        drift <= 1,
        "the clip should start {expected} samples in, measured {placed} ({drift} off). \
         Truncating the offset to whole milliseconds would read about {} samples.",
        (offset().as_secs_f64() * 1000.0).trunc() / 1000.0 * f64::from(OUTPUT_RATE)
    );
}

/// The same measurement with a source whose rate is not the mix rate.
///
/// **This test is what lets the builder convert at the mix rate without resampling
/// first.** The sample count the graph emits is computed from the mix rate, while
/// nothing in the chain has converted the source yet: the engine's track spec carries
/// the target rate, so the mixer's conditional `aresample` never fires and `amovie`
/// emits 44.1 kHz into the effects chain. It is nonetheless correct, because `adelay`
/// reads `delays` in `config_input`, after format negotiation, and negotiation resolves
/// that link to the rate the sink demands.
///
/// An explicit normaliser before `adelay` would make this independent of negotiation,
/// and was tried: with it removed the clip still lands within one sample here, so the
/// node was dead weight and no mutation could distinguish it. This assertion is the
/// guarantee instead, at the same one-sample tolerance as the native-rate case. If
/// negotiation ever stops placing the conversion upstream, the delay stretches by
/// 48/44.1 and this fails by about 2100 samples.
#[test]
fn the_export_should_place_a_clip_on_the_right_sample_at_44_1_khz() {
    let Some(placed) = placed_samples("resampled", 44_100) else {
        return;
    };
    let expected = expected_samples();
    let drift = placed.abs_diff(expected);
    assert!(
        drift <= 1,
        "the clip should start {expected} samples in, measured {placed} ({drift} off). \
         A delay converted at a rate the chain does not have would read about {} samples.",
        (expected as f64 * f64::from(OUTPUT_RATE) / 44_100.0).round()
    );
}
