//! `LoudnessNormalize`'s `lra` target must be read and reported (#1854).
//!
//! The parameter was validated, stored and never read: a programme whose loudness
//! range was twice what the caller asked for was normalized and delivered without
//! a word. The range is now compared against the measurement pass 1 already took
//! (`lavfi.r128.LRA`), using the condition FFmpeg's own `loudnorm` uses to decide
//! whether a single gain is legitimate (`af_loudnorm.c:812`).
//!
//! The gain is deliberately unchanged by the outcome, so the report is the only
//! observable: these tests install a capturing logger. That is process-wide, which
//! is why they live in a test binary of their own.
//!
//! Needs `ebur128` and `volume`, so these skip where the build has neither.

#![allow(clippy::unwrap_used)]

use ff_filter::{FilterGraph, FilterStep};
use ff_format::{AudioFrame, SampleFormat, Timestamp};
use std::sync::{Mutex, OnceLock};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
const FRAME_SAMPLES: usize = 1024;
/// 1024 samples at 48 kHz, so 47 frames make just over a second.
const FRAMES_PER_SECOND: usize = 47;
/// Quiet and loud halves, about 34 dB apart. Measured through `LoudnessMeter` as
/// `LRA = 7.8 LU` for a five-second-per-half programme.
const QUIET_AMPLITUDE: f32 = 0.01;
const LOUD_AMPLITUDE: f32 = 0.5;

fn captured() -> &'static Mutex<Vec<String>> {
    static LINES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    LINES.get_or_init(|| Mutex::new(Vec::new()))
}

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        if let Ok(mut lines) = captured().lock() {
            lines.push(format!("{} {}", record.level(), record.args()));
        }
    }
    fn flush(&self) {}
}

/// Installs the capturing logger once and clears what earlier tests recorded.
///
/// Tests in one binary share a process, so the capture is serialised by taking the
/// same lock the logger writes through.
fn start_capture() -> std::sync::MutexGuard<'static, ()> {
    static INIT: OnceLock<()> = OnceLock::new();
    static SERIALISE: Mutex<()> = Mutex::new(());
    static CAPTURE: Capture = Capture;
    INIT.get_or_init(|| {
        // `set_logger` rather than `set_boxed_logger`: the latter needs `log`'s
        // `std` feature, which this workspace does not enable.
        let _ = log::set_logger(&CAPTURE);
        log::set_max_level(log::LevelFilter::Trace);
    });
    let guard = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
    captured().lock().unwrap().clear();
    guard
}

/// A stereo packed F32 frame of a 440 Hz tone at `amplitude`.
fn tone_frame(index: usize, amplitude: f32) -> AudioFrame {
    let mut buf = vec![0u8; FRAME_SAMPLES * CHANNELS as usize * 4];
    for (i, chunk) in buf.chunks_exact_mut(4 * CHANNELS as usize).enumerate() {
        let t = (index * FRAME_SAMPLES + i) as f64 / f64::from(SAMPLE_RATE);
        let v = (f64::from(amplitude) * (t * 440.0 * std::f64::consts::TAU).sin()) as f32;
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

/// Runs a wide-range programme through the step and returns the captured log, or
/// `None` where this build cannot run the analysis.
///
/// Five seconds quiet then five seconds loud: EBU R128 integrates over 400 ms
/// blocks and derives the range from 3 s blocks, so a shorter programme has no
/// range to speak of.
fn log_of_wide_range_run(target_lra: f32) -> Option<Vec<String>> {
    let _serialised = start_capture();

    let mut graph = match FilterGraph::builder()
        .add_step(FilterStep::LoudnessNormalize {
            target_lufs: -23.0,
            true_peak_db: -1.0,
            lra: target_lra,
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: the graph could not be built here: {e}");
            return None;
        }
    };

    let mut index = 0;
    for amplitude in [QUIET_AMPLITUDE, LOUD_AMPLITUDE] {
        for _ in 0..(5 * FRAMES_PER_SECOND) {
            graph.push_audio(0, &tone_frame(index, amplitude)).unwrap();
            index += 1;
        }
    }
    graph.flush_audio();

    let mut frames = 0usize;
    loop {
        match graph.pull_audio() {
            Ok(Some(_)) => frames += 1,
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: pull_audio failed here: {e}");
                return None;
            }
        }
    }
    if frames == 0 {
        // Same shape as `loudness_ceiling_tests.rs`, which CI has proven: on a build
        // with no filters the two-pass step yields nothing, and asserting here would
        // fail for an environmental reason. What stops this from being vacuous is the
        // assertion below, which only passes on a run that really measured a range.
        println!("Skipping: no frames came back, so the build lacks ebur128 or volume");
        return None;
    }

    let lines = captured().lock().unwrap().clone();

    // The report is only meaningful if pass 1 published a range. Without this, a
    // build that measured nothing would read exactly like a range that fits.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("measured_lra=") && !l.contains("measured_lra=none")),
        "pass 1 published no loudness range, so the outcome says nothing: {lines:?}"
    );

    Some(lines)
}

#[test]
fn wide_dynamic_range_should_be_reported_against_a_narrow_target() {
    let Some(lines) = log_of_wide_range_run(3.0) else {
        return;
    };
    let warning = lines
        .iter()
        .find(|l| l.contains("loudness range wider than requested"));
    let warning = warning.unwrap_or_else(|| {
        panic!("a range wider than the target was not reported: {lines:?}");
    });
    assert!(
        warning.starts_with("WARN"),
        "the report should be a warning, got {warning}"
    );
    assert!(
        warning.contains("target_lra=3.0") && warning.contains("measured_lra="),
        "the report should name both values, got {warning}"
    );
    assert!(
        warning.contains("range_unchanged"),
        "the report should say the range was not changed, got {warning}"
    );
}

#[test]
fn a_range_within_the_target_should_not_be_reported() {
    let Some(lines) = log_of_wide_range_run(20.0) else {
        return;
    };
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("loudness range wider than requested")),
        "a range inside the target must not be reported: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("lra_outcome=Within")),
        "the agreement should be recorded: {lines:?}"
    );
}
