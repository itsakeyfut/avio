//! A two-pass audio step must not measure before its input has ended (#1821).
//!
//! `LoudnessNormalize` and `NormalizePeak` buffer every pushed frame and analyse
//! the whole programme at once. `pull_audio` used to run that analysis on its
//! first call, so a caller that pulled after each push measured the first frame
//! alone: EBU R128 integrates over 400 ms blocks, reported its silence floor of
//! -70 LUFS for a 21 ms frame, and the resulting `target - measured` gain was
//! about +56 dB. Frames pushed afterwards were never processed.
//!
//! These tests need no filters, because the gate acts before any graph is built,
//! so they run on CI's Linux FFmpeg as well.

#![allow(clippy::unwrap_used)]

use ff_filter::{FilterGraph, FilterStep};
use ff_format::{AudioFrame, SampleFormat, Timestamp};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
const FRAME_SAMPLES: usize = 1024;

/// A stereo packed F32 frame at a constant amplitude.
fn frame(amplitude: f32) -> AudioFrame {
    let bytes = amplitude.to_le_bytes();
    let mut buf = vec![0u8; FRAME_SAMPLES * CHANNELS as usize * 4];
    for chunk in buf.chunks_exact_mut(4) {
        chunk.copy_from_slice(&bytes);
    }
    AudioFrame::new(
        vec![buf],
        FRAME_SAMPLES,
        CHANNELS,
        SAMPLE_RATE,
        SampleFormat::F32,
        Timestamp::default(),
    )
    .unwrap()
}

fn graph_with(step: FilterStep) -> Option<FilterGraph> {
    match FilterGraph::builder().add_step(step).build() {
        Ok(g) => Some(g),
        Err(e) => {
            println!("Skipping: the graph could not be built here: {e}");
            None
        }
    }
}

fn loudness_step() -> FilterStep {
    FilterStep::LoudnessNormalize {
        target_lufs: -14.0,
        true_peak_db: -1.0,
        lra: 7.0,
    }
}

fn peak_step() -> FilterStep {
    FilterStep::NormalizePeak { target_db: -1.0 }
}

/// The defect in one assertion: pulling between pushes must yield nothing, so the
/// measurement cannot run on a prefix of the input.
#[test]
fn loudness_normalize_should_yield_nothing_before_the_input_ends() {
    let Some(mut graph) = graph_with(loudness_step()) else {
        return;
    };

    for i in 0..8 {
        graph.push_audio(0, &frame(0.5)).unwrap();
        assert!(
            graph.pull_audio().unwrap().is_none(),
            "pull {i} produced a frame before flush_audio; the measurement would \
             have run on {} frame(s) instead of the whole programme",
            i + 1
        );
    }
}

#[test]
fn normalize_peak_should_yield_nothing_before_the_input_ends() {
    let Some(mut graph) = graph_with(peak_step()) else {
        return;
    };

    for i in 0..8 {
        graph.push_audio(0, &frame(0.5)).unwrap();
        assert!(
            graph.pull_audio().unwrap().is_none(),
            "pull {i} produced a frame before flush_audio"
        );
    }
}

/// Pulling early must not consume the input either: the frames are still there to
/// be analysed once EOF arrives. Needs the `ebur128` and `volume` filters, so it
/// skips where the build has none.
#[test]
fn loudness_normalize_should_still_deliver_every_frame_after_a_flush() {
    const PUSHED: usize = 8;
    let Some(mut graph) = graph_with(loudness_step()) else {
        return;
    };

    for i in 0..PUSHED {
        graph.push_audio(0, &frame(0.5)).unwrap();
        // Asserted here too, not only in the test above: a frame emitted now is
        // one the drain below cannot find, which would otherwise reach the
        // "no frames, so no filters" skip and hide the defect.
        assert!(
            graph.pull_audio().unwrap().is_none(),
            "pull {i} produced a frame before flush_audio"
        );
    }

    graph.flush_audio();
    let mut drained = 0usize;
    loop {
        match graph.pull_audio() {
            Ok(Some(_)) => drained += 1,
            Ok(None) => break,
            Err(e) => {
                println!("Skipping the drain: this build cannot run the analysis: {e}");
                return;
            }
        }
    }

    if drained == 0 {
        println!("Skipping: the build produced no frames, so it lacks ebur128 or volume");
        return;
    }
    assert_eq!(
        drained, PUSHED,
        "every pushed frame must survive the early pulls and emerge after the flush"
    );
}
