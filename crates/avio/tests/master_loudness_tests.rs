//! Regression: `LoudnessNormalize` must reach its target on the master bus, not
//! only on a track's effect chain (#1821).
//!
//! The two routes differ only in call order. A track chain pushes every frame,
//! flushes, then drains; the master bus pulled after each push. A two-pass step
//! buffers its input and used to run both passes on the first pull, so the master
//! bus measured one 21 ms frame. EBU R128 integrates over 400 ms blocks, reported
//! its -70 LUFS silence floor, and the resulting gain was about +56 dB: the
//! output peaked at 340 against a full scale of 1.0.
//!
//! The assertions are on measured loudness rather than peak alone, because the
//! criterion is that the requested target is reached.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

mod fixtures;

use avio::{Clip, EncoderConfig, FilterStep, Timeline, TimelineError, Track};
use ff_filter::FilterError;
use ff_filter::analysis::LoudnessMeter;
use fixtures::{FileGuard, make_source_file, measure_audio, test_output_path, write_tone_wav};

const TARGET_LUFS: f32 = -14.0;

fn loudness_step() -> FilterStep {
    FilterStep::LoudnessNormalize {
        target_lufs: TARGET_LUFS,
        true_peak_db: -1.0,
        lra: 7.0,
    }
}

/// Renders a 2 s tone with the step on the requested route.
///
/// `None` means this `FFmpeg` build could not take part: no encoder for the
/// video source, or no filters to build the graph with, which is CI's Linux
/// `FFmpeg` (`--disable-everything`). Any other failure panics.
fn render(tag: &str, master: bool) -> Option<std::path::PathBuf> {
    let video = test_output_path(&format!("ln_video_{tag}.mp4"));
    let _gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, 30.0, 60, 80, 90, 120)?;

    let tone = test_output_path(&format!("ln_tone_{tag}.wav"));
    let _gt = FileGuard::new(tone.clone());
    write_tone_wav(&tone, 48_000, 2, 16, 2.0);

    let out = test_output_path(&format!("ln_out_{tag}.mp4"));

    let builder = Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&video)]);
    let audio = vec![Clip::new(&tone)];
    let built = if master {
        builder
            .audio_track(audio)
            .audio_filter(vec![loudness_step()])
            .build()
    } else {
        builder
            .audio_track_with(Track::new(audio).audio_effects(vec![loudness_step()]))
            .build()
    };
    let timeline = match built {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    match timeline.render(&out, EncoderConfig::builder().build()) {
        Ok(()) => Some(out),
        Err(TimelineError::Filter(
            FilterError::BuildFailed | FilterError::CompositionFailed { .. },
        )) => {
            println!("Skipping: the graph needs filters this build lacks");
            None
        }
        Err(e) => panic!("render failed for a reason other than a missing filter: {e}"),
    }
}

/// Asserts the output reaches the requested loudness and stays within full scale.
fn assert_normalized(out: &std::path::Path, route: &str) {
    let (peak, _rms, secs) = measure_audio(out).expect("the rendered audio must decode");
    assert!(
        peak <= 1.0,
        "{route}: the output must stay within full scale, got peak={peak}"
    );

    // What the defect destroyed was the input reaching the step at all: it
    // measured one frame of the roughly ninety pushed. Loudness alone would pass
    // on a route that dropped half the programme and normalised the rest, and the
    // container's own duration would not notice either, since it follows the
    // 2 s video track.
    assert!(
        (secs - 2.0).abs() < 0.35,
        "{route}: every frame must survive the two-pass step, got {secs} s of decoded audio"
    );

    let measured = match LoudnessMeter::new(out).measure() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping the loudness check: this build cannot measure it: {e}");
            return;
        }
    };
    let delta = (measured.integrated_lufs - TARGET_LUFS).abs();
    assert!(
        delta <= 1.5,
        "{route}: expected about {TARGET_LUFS} LUFS, measured {} LUFS (delta {delta})",
        measured.integrated_lufs
    );
}

#[test]
fn loudness_normalize_on_the_master_bus_should_reach_the_target() {
    let Some(out) = render("master", true) else {
        return;
    };
    let _g = FileGuard::new(out.clone());
    assert_normalized(&out, "master bus");
}

#[test]
fn loudness_normalize_on_a_track_chain_should_reach_the_target() {
    let Some(out) = render("track", false) else {
        return;
    };
    let _g = FileGuard::new(out.clone());
    assert_normalized(&out, "track chain");
}
