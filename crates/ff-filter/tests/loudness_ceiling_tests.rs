//! `LoudnessNormalize`'s `true_peak_db` ceiling must bound the output (#1822).
//!
//! The ceiling was accepted, validated and then never read: `-1`, `-12` and
//! `-20 dBTP` all produced one identical peak, so a `-20` request was exceeded by
//! about 15 dB. Pass 1 already measured the peak with `ebur128=peak=true` and
//! discarded it; the gain is now bounded by what the ceiling allows.
//!
//! Needs `ebur128` and `volume`, so these skip where the build has neither.

#![allow(clippy::unwrap_used)]

use ff_filter::{FilterGraph, FilterStep};
use ff_format::{AudioFrame, SampleFormat, Timestamp};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
const FRAME_SAMPLES: usize = 1024;
/// Half scale: -6.02 dBFS, comfortably above every ceiling under test.
const AMPLITUDE: f32 = 0.5;

/// A stereo packed F32 frame of a 440 Hz tone at half scale.
fn tone_frame(index: usize) -> AudioFrame {
    let mut buf = vec![0u8; FRAME_SAMPLES * CHANNELS as usize * 4];
    for (i, chunk) in buf.chunks_exact_mut(4 * CHANNELS as usize).enumerate() {
        let t = (index * FRAME_SAMPLES + i) as f64 / f64::from(SAMPLE_RATE);
        let v = (AMPLITUDE as f64 * (t * 440.0 * std::f64::consts::TAU).sin()) as f32;
        for ch in chunk.chunks_exact_mut(4) {
            ch.copy_from_slice(&v.to_le_bytes());
        }
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

/// The linear peak of a run at the given ceiling, or `None` where this build
/// cannot run the analysis.
///
/// One second of tone: EBU R128 integrates over 400 ms blocks, so a shorter
/// programme measures as silence and the gain would come out meaningless.
fn peak_at_ceiling(true_peak_db: f32) -> Option<f32> {
    let mut graph = match FilterGraph::builder()
        .add_step(FilterStep::LoudnessNormalize {
            target_lufs: -6.0,
            true_peak_db,
            lra: 7.0,
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: the graph could not be built here: {e}");
            return None;
        }
    };

    for i in 0..47 {
        graph.push_audio(0, &tone_frame(i)).unwrap();
    }
    graph.flush_audio();

    let mut peak = 0.0f32;
    let mut frames = 0usize;
    loop {
        match graph.pull_audio() {
            Ok(Some(frame)) => {
                frames += 1;
                for chunk in frame.planes()[0].chunks_exact(4) {
                    let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    peak = peak.max(v.abs());
                }
            }
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: this build cannot run the analysis: {e}");
                return None;
            }
        }
    }

    if frames == 0 {
        println!("Skipping: no frames came back, so the build lacks ebur128 or volume");
        return None;
    }
    Some(peak)
}

fn db(linear: f32) -> f32 {
    if linear > 0.0 {
        20.0 * linear.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Acceptance criterion 2: several ceilings against the same source, the measured
/// peaks differ, and each sits at or below what was requested.
#[test]
fn different_true_peak_ceilings_should_produce_different_and_respected_peaks() {
    const CEILINGS: [f32; 3] = [-1.0, -12.0, -20.0];

    let mut measured = Vec::new();
    for ceiling in CEILINGS {
        let Some(peak) = peak_at_ceiling(ceiling) else {
            return;
        };
        measured.push((ceiling, peak, db(peak)));
    }

    for &(ceiling, peak, peak_db) in &measured {
        assert!(
            peak_db <= ceiling + 0.5,
            "a {ceiling} dBTP ceiling must bound the output, got {peak_db:.2} dBTP \
             (linear {peak:.4})"
        );
        // Not merely below the ceiling. A conversion that erred conservatively
        // would over-attenuate and still satisfy "at or below", so where the
        // ceiling binds the output must sit close to it as well. The source is
        // -6.7 LUFS against a -6.0 target, so the loudness term asks for almost no
        // gain and the ceiling decides for anything under about -7.
        if ceiling < -7.0 {
            assert!(
                peak_db >= ceiling - 1.5,
                "a {ceiling} dBTP ceiling should land near its ceiling rather than \
                 far below it, got {peak_db:.2} dBTP"
            );
        }
    }

    // The defect produced one identical peak for every ceiling, so difference is
    // the property that distinguishes a working ceiling from an ignored one.
    for window in measured.windows(2) {
        let (lo_ceiling, _, lo_db) = window[0];
        let (hi_ceiling, _, hi_db) = window[1];
        assert!(
            (lo_db - hi_db).abs() > 1.0,
            "ceilings {lo_ceiling} and {hi_ceiling} dBTP produced the same peak \
             ({lo_db:.2} vs {hi_db:.2} dBTP), so the ceiling is being ignored"
        );
    }
}

/// A ceiling far above the signal must not pull the output down: the loudness
/// target decides there, which is why the `-1` case sits below its ceiling rather
/// than at it.
#[test]
fn a_ceiling_above_the_signal_should_leave_the_loudness_target_in_charge() {
    let Some(peak) = peak_at_ceiling(-0.1) else {
        return;
    };
    let Some(tight) = peak_at_ceiling(-20.0) else {
        return;
    };
    assert!(
        peak > tight,
        "a -0.1 dBTP ceiling should not bind where -20 does: {peak:.4} vs {tight:.4}"
    );
}
