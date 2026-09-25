//! Regression: `ACompressor` and `ANoiseGate` must render on a track's audio
//! effect chain and as a master audio filter, and must reach the signal (#1818).
//!
//! Both used to fail with `Failed to send audio frame: Invalid argument
//! (code=-22)`, the same error WAV and FLAC sources produced (#1812). The cause
//! was shared: `av_frame_to_audio_frame` copied a packed audio plane without its
//! channel factor, so the frame declared more samples than it carried. These
//! filters output a packed format, which is why an empty effect chain worked and
//! these two did not.
//!
//! Rendering without an error is not enough to keep this fixed: a chain that
//! silently dropped to a passthrough would also return `Ok`. The assertions are
//! on the measured signal.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

mod fixtures;

use avio::{Clip, EncoderConfig, FilterStep, Timeline, TimelineError, Track};
use ff_filter::FilterError;
use fixtures::{FileGuard, make_source_file, measure_audio, test_output_path, write_tone_wav};

/// Where the step is placed in the timeline.
enum Placement {
    /// No effect at all, for the baseline the others are compared against.
    None,
    /// `Track::audio_effects`, the per-track insert.
    Track(FilterStep),
    /// `TimelineBuilder::audio_filter`, the master bus.
    Master(FilterStep),
}

/// Renders a 1 s tone under `placement` and returns the output's (peak, rms).
///
/// `None` means this `FFmpeg` build could not take part: no encoder for the video
/// source, or no filters to build the graph with, which is CI's Linux `FFmpeg`
/// (`--disable-everything`). Any other failure panics, so a genuine render
/// regression cannot be mistaken for an unavailable environment.
fn render_and_measure(tag: &str, placement: Placement) -> Option<(f64, f64)> {
    let video = test_output_path(&format!("afx_video_{tag}.mp4"));
    let _gv = FileGuard::new(video.clone());
    make_source_file(&video, 160, 120, 30.0, 30, 80, 90, 120)?;

    let tone = test_output_path(&format!("afx_tone_{tag}.wav"));
    let _gt = FileGuard::new(tone.clone());
    write_tone_wav(&tone, 48_000, 2, 16, 1.0);

    let out = test_output_path(&format!("afx_out_{tag}.mp4"));
    let _go = FileGuard::new(out.clone());

    let builder = Timeline::builder()
        .canvas(160, 120)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&video)]);
    let audio = vec![Clip::new(&tone)];
    let built = match placement {
        Placement::None => builder.audio_track(audio).build(),
        Placement::Track(step) => builder
            .audio_track_with(Track::new(audio).audio_effects(vec![step]))
            .build(),
        Placement::Master(step) => builder.audio_track(audio).audio_filter(vec![step]).build(),
    };
    let timeline = match built {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return None;
        }
    };

    match timeline.render(&out, EncoderConfig::builder().build()) {
        Ok(()) => {}
        Err(TimelineError::Filter(
            FilterError::BuildFailed | FilterError::CompositionFailed { .. },
        )) => {
            println!("Skipping: the graph needs filters this build lacks");
            return None;
        }
        Err(e) => panic!("render failed for a reason other than a missing filter: {e}"),
    }

    let measured = measure_audio(&out);
    if measured.is_none() {
        println!("Skipping: cannot decode the rendered audio here");
    }
    measured
}

fn compressor() -> FilterStep {
    FilterStep::ACompressor {
        threshold_db: -20.0,
        ratio: 4.0,
        attack_ms: 5.0,
        release_ms: 50.0,
        makeup_db: 0.0,
    }
}

fn gate() -> FilterStep {
    FilterStep::ANoiseGate {
        threshold_db: -40.0,
        attack_ms: 10.0,
        release_ms: 100.0,
    }
}

/// The source tone is a 440 Hz sine at half scale: peak 0.5, RMS 0.354.
/// A render that passes it through unchanged lands near those.
#[test]
fn a_track_with_no_audio_effects_should_pass_the_tone_through() {
    let Some((peak, rms)) = render_and_measure("baseline", Placement::None) else {
        return;
    };
    assert!(
        (0.4..0.7).contains(&peak) && (0.25..0.45).contains(&rms),
        "the baseline should carry the source tone: peak={peak} rms={rms}"
    );
}

/// A compressor above its threshold must pull the peak down and reduce the crest
/// factor, which is what #1818's acceptance criterion asks to observe.
#[test]
fn a_compressor_on_a_track_chain_should_reduce_the_peak() {
    let Some((peak, rms)) = render_and_measure("comp_track", Placement::Track(compressor())) else {
        return;
    };
    assert!(
        peak < 0.45,
        "a compressor with a -20 dB threshold should pull the 0.5 peak down: peak={peak}"
    );
    assert!(
        rms > 0.0,
        "the compressor must not silence the signal: rms={rms}"
    );
}

#[test]
fn a_compressor_as_a_master_filter_should_reduce_the_peak() {
    let Some((peak, rms)) = render_and_measure("comp_master", Placement::Master(compressor()))
    else {
        return;
    };
    assert!(
        peak < 0.45,
        "a compressor on the master bus should pull the 0.5 peak down: peak={peak}"
    );
    assert!(
        rms > 0.0,
        "the compressor must not silence the signal: rms={rms}"
    );
}

/// The tone sits far above a -40 dB gate, so the gate must pass it. This pins the
/// render succeeding and the signal surviving, which is what regressed.
#[test]
fn a_noise_gate_on_a_track_chain_should_pass_a_tone_above_its_threshold() {
    let Some((peak, rms)) = render_and_measure("gate_track", Placement::Track(gate())) else {
        return;
    };
    assert!(
        (0.4..0.7).contains(&peak) && (0.25..0.45).contains(&rms),
        "a -40 dB gate should pass a half-scale tone: peak={peak} rms={rms}"
    );
}

#[test]
fn a_noise_gate_as_a_master_filter_should_pass_a_tone_above_its_threshold() {
    let Some((peak, rms)) = render_and_measure("gate_master", Placement::Master(gate())) else {
        return;
    };
    assert!(
        (0.4..0.7).contains(&peak) && (0.25..0.45).contains(&rms),
        "a -40 dB gate on the master bus should pass a half-scale tone: peak={peak} rms={rms}"
    );
}
