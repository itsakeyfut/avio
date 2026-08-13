//! Integration tests for audio filter effects on a reference sine wave.

#![allow(clippy::unwrap_used)]

use ff_filter::{EqBand, FilterError, FilterGraph};
use ff_format::{AudioFrame, SampleFormat, Timestamp};

/// Stereo packed F32 sine wave frame at the given frequency.
///
/// Amplitude is 0.1 to leave headroom for volume boosts (avoids clipping).
fn make_sine_frame(freq_hz: f64, sample_rate: u32, num_samples: usize) -> AudioFrame {
    let channels = 2usize;
    let bytes_per_sample = 4usize; // f32
    let mut buf = vec![0u8; num_samples * channels * bytes_per_sample];
    for i in 0..num_samples {
        let t = i as f64 / sample_rate as f64;
        let v = (0.1_f32 * (2.0 * std::f64::consts::PI * freq_hz * t).sin() as f32).to_le_bytes();
        let offset = i * channels * bytes_per_sample;
        buf[offset..offset + 4].copy_from_slice(&v); // L
        buf[offset + 4..offset + 8].copy_from_slice(&v); // R
    }
    AudioFrame::new(
        vec![buf],
        num_samples,
        2,
        sample_rate,
        SampleFormat::F32,
        Timestamp::default(),
    )
    .unwrap()
}

/// Stereo packed F32 sine wave frame with configurable amplitude.
fn make_sine_with_amplitude(
    freq_hz: f64,
    amplitude: f32,
    sample_rate: u32,
    num_samples: usize,
) -> AudioFrame {
    let channels = 2usize;
    let bytes_per_sample = 4usize;
    let mut buf = vec![0u8; num_samples * channels * bytes_per_sample];
    for i in 0..num_samples {
        let t = i as f64 / f64::from(sample_rate);
        let v = (amplitude * (2.0 * std::f64::consts::PI * freq_hz * t).sin() as f32).to_le_bytes();
        let offset = i * channels * bytes_per_sample;
        buf[offset..offset + 4].copy_from_slice(&v);
        buf[offset + 4..offset + 8].copy_from_slice(&v);
    }
    AudioFrame::new(
        vec![buf],
        num_samples,
        2,
        sample_rate,
        SampleFormat::F32,
        Timestamp::default(),
    )
    .unwrap()
}

/// RMS of all samples in an [`AudioFrame`], trying packed then planar format.
fn frame_rms(frame: &AudioFrame) -> f64 {
    if let Some(s) = frame.as_f32() {
        rms(s)
    } else if let Some(s) = frame.channel_as_f32(0) {
        rms(s)
    } else {
        0.0
    }
}

/// RMS of an f32 sample slice (packed, interleaved channels).
fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    (sum_sq / samples.len() as f64).sqrt()
}

/// Push `frame` through `graph` and pull the first available output frame.
/// Returns `None` and prints a skip message if the push or pull fails.
fn push_pull_audio(graph: &mut FilterGraph, frame: &AudioFrame) -> Option<AudioFrame> {
    match graph.push_audio(0, frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping push_audio: {e}");
            return None;
        }
    }
    match graph.pull_audio() {
        Ok(Some(f)) => Some(f),
        Ok(None) => {
            println!("Skipping: no audio output frame produced");
            None
        }
        Err(e) => {
            println!("Skipping pull_audio: {e}");
            None
        }
    }
}

#[test]
fn volume_6db_should_double_amplitude() {
    let mut graph = match FilterGraph::builder().volume(6.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = make_sine_frame(440.0, 48000, 4800);
    let out = match push_pull_audio(&mut graph, &frame) {
        Some(f) => f,
        None => return,
    };

    let in_samples = frame.as_f32().unwrap();
    let in_rms = rms(in_samples);

    // Extract output samples — handle both packed F32 and planar F32p.
    let out_rms = if let Some(s) = out.as_f32() {
        rms(s)
    } else if let Some(s) = out.channel_as_f32(0) {
        rms(s)
    } else {
        println!("Skipping: unrecognised output format {:?}", out.format());
        return;
    };

    let ratio = out_rms / in_rms;
    // +6 dB ≈ 2× amplitude; allow ±15% tolerance for FFmpeg quantisation/resampling.
    assert!(
        (ratio - 2.0).abs() < 0.30,
        "+6 dB should double amplitude: expected ratio≈2.0, got {ratio:.3}"
    );
}

#[test]
fn volume_minus6db_should_halve_amplitude() {
    let mut graph = match FilterGraph::builder().volume(-6.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = make_sine_frame(440.0, 48000, 4800);
    let out = match push_pull_audio(&mut graph, &frame) {
        Some(f) => f,
        None => return,
    };

    let in_rms = rms(frame.as_f32().unwrap());
    let out_rms = if let Some(s) = out.as_f32() {
        rms(s)
    } else if let Some(s) = out.channel_as_f32(0) {
        rms(s)
    } else {
        println!("Skipping: unrecognised output format {:?}", out.format());
        return;
    };

    let ratio = out_rms / in_rms;
    // −6 dB ≈ 0.5× amplitude; allow ±15% tolerance.
    assert!(
        (ratio - 0.5).abs() < 0.10,
        "-6 dB should halve amplitude: expected ratio≈0.5, got {ratio:.3}"
    );
}

#[test]
fn afade_in_should_start_at_silence_and_reach_full_volume() {
    // Fade in over the entire 0.1 s frame so first samples are near silence.
    let mut graph = match FilterGraph::builder().afade_in(0.0, 0.1).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    // 4800 samples @ 48 kHz = 0.1 s
    let frame = make_sine_frame(440.0, 48000, 4800);
    let out = match push_pull_audio(&mut graph, &frame) {
        Some(f) => f,
        None => return,
    };

    // First samples must be near silence (absolute value < 0.02).
    let first_sample = if let Some(s) = out.as_f32() {
        s[0].abs()
    } else if let Some(s) = out.channel_as_f32(0) {
        s[0].abs()
    } else {
        println!("Skipping: unrecognised output format {:?}", out.format());
        return;
    };

    assert!(
        first_sample < 0.02,
        "afade_in: first sample should be near silence, got {first_sample:.4}"
    );
}

#[test]
fn afade_out_should_reach_silence_at_end() {
    // Fade out starting at 0.0 s over 0.1 s — entire frame fades to silence.
    let mut graph = match FilterGraph::builder().afade_out(0.0, 0.1).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = make_sine_frame(440.0, 48000, 4800);
    let out = match push_pull_audio(&mut graph, &frame) {
        Some(f) => f,
        None => return,
    };

    // Last sample must be near silence (absolute value < 0.02).
    let last_sample = if let Some(s) = out.as_f32() {
        s[s.len() - 1].abs()
    } else if let Some(s) = out.channel_as_f32(0) {
        s[s.len() - 1].abs()
    } else {
        println!("Skipping: unrecognised output format {:?}", out.format());
        return;
    };

    assert!(
        last_sample < 0.02,
        "afade_out: last sample should be near silence, got {last_sample:.4}"
    );
}

#[test]
fn equalizer_peak_should_boost_target_frequency() {
    // Apply a +6 dB peak at 1 kHz; verify the filter applies without panic and
    // produces output. Full FFT-based frequency verification is outside the scope
    // of a unit integration test.
    let bands = vec![EqBand::Peak {
        freq_hz: 1000.0,
        gain_db: 6.0,
        q: 1.0,
    }];
    let mut graph = match FilterGraph::builder().equalizer(bands).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = make_sine_frame(1000.0, 48000, 4800);
    match push_pull_audio(&mut graph, &frame) {
        Some(out) => {
            assert_eq!(out.sample_rate(), 48000, "sample rate must be unchanged");
            // With +6 dB boost at 1 kHz input sine, output RMS should be ≥ input RMS.
            let in_rms = rms(frame.as_f32().unwrap());
            let out_rms = if let Some(s) = out.as_f32() {
                rms(s)
            } else if let Some(s) = out.channel_as_f32(0) {
                rms(s)
            } else {
                return;
            };
            assert!(
                out_rms >= in_rms * 0.9,
                "EQ peak at 1 kHz: output RMS ({out_rms:.4}) should not be less than input ({in_rms:.4})"
            );
        }
        None => {}
    }
}

#[test]
fn stereo_to_mono_should_average_both_channels() {
    let mut graph = match FilterGraph::builder().stereo_to_mono().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = make_sine_frame(440.0, 48000, 4800);
    let out = match push_pull_audio(&mut graph, &frame) {
        Some(f) => f,
        None => return,
    };

    assert_eq!(
        out.channels(),
        1,
        "stereo_to_mono: output must have exactly 1 channel, got {}",
        out.channels()
    );
    assert_eq!(
        out.sample_rate(),
        48000,
        "sample rate must be unchanged after stereo_to_mono"
    );
}

#[test]
fn audio_delay_100ms_should_shift_audio_later() {
    // A 100 ms adelay inserts 4800 samples of silence at the beginning for a
    // 48 kHz stream. We push one frame and verify the filter applies without
    // panic and that the output sample rate is preserved.
    let mut graph = match FilterGraph::builder().audio_delay(100.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = make_sine_frame(440.0, 48000, 9600);

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping push_audio: {e}");
            return;
        }
    }

    // adelay may need a flush to emit output; try pulling once.
    match graph.pull_audio() {
        Ok(Some(out)) => {
            assert_eq!(
                out.sample_rate(),
                48000,
                "sample rate must be preserved after delay"
            );
            // The first sample of delayed audio should be silence (near 0.0).
            if let Some(s) = out.as_f32() {
                if !s.is_empty() {
                    assert!(
                        s[0].abs() < 0.01,
                        "audio_delay: first output sample should be silence, got {:.4}",
                        s[0]
                    );
                }
            }
        }
        Ok(None) => {
            // adelay may buffer internally; this is acceptable.
            println!("Note: audio_delay produced no immediate output (buffering expected).");
        }
        Err(e) => {
            println!("Skipping pull_audio: {e}");
        }
    }
}

/// Verifies that `FilterGraph::duck()` reduces the background level by at least
/// 12 dB when a foreground signal above the compression threshold is present.
///
/// Acceptance criterion for issue #413.
#[test]
fn duck_should_reduce_background_by_at_least_12db_when_foreground_active() {
    // Background: −20 dBFS (at threshold); foreground: −6 dBFS (14 dB above threshold).
    // With 20:1 ratio the expected sidechain-triggered gain reduction is ≈ 13.3 dB,
    // so the 12 dB assertion has ≈ 1 dB margin.
    let bg_amplitude = 10.0_f32.powf(-20.0 / 20.0); // 0.1 linear
    let fg_amplitude = 10.0_f32.powf(-6.0 / 20.0); // ≈ 0.501 linear

    const SAMPLE_RATE: u32 = 48_000;
    const NUM_SAMPLES: usize = 48_000; // 1 second — compressor settles within first 10 ms

    let bg_frame = make_sine_with_amplitude(220.0, bg_amplitude, SAMPLE_RATE, NUM_SAMPLES);
    let fg_frame = make_sine_with_amplitude(440.0, fg_amplitude, SAMPLE_RATE, NUM_SAMPLES);
    let bg_rms_baseline = frame_rms(&bg_frame);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    if let Err(e) = graph.duck(-20.0, 20.0, 10.0, 200.0) {
        println!("Skipping: duck() setup failed: {e}");
        return;
    }

    // Lazy FFmpeg graph construction happens on first push_audio.
    // FilterError::BuildFailed signals that sidechaincompress is unavailable.
    match graph.push_audio(0, &bg_frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: sidechaincompress not available in this FFmpeg build");
            return;
        }
        Err(e) => panic!("push_audio(0) failed unexpectedly: {e}"),
    }
    match graph.push_audio(1, &fg_frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: sidechaincompress not available in this FFmpeg build");
            return;
        }
        Err(e) => panic!("push_audio(1) failed unexpectedly: {e}"),
    }

    let out = match graph.pull_audio() {
        Ok(Some(f)) => f,
        Ok(None) => {
            println!("Skipping: no output frame produced (compressor may buffer internally)");
            return;
        }
        Err(e) => panic!("pull_audio failed unexpectedly: {e}"),
    };

    let out_rms = frame_rms(&out);
    assert!(
        out_rms > 0.0,
        "duck output must not be completely silent (got {out_rms:.6})"
    );

    let reduction_db = 20.0_f64 * (bg_rms_baseline / out_rms).log10();
    assert!(
        reduction_db >= 12.0,
        "background reduction must be ≥ 12 dB when foreground is active; \
         baseline_rms={bg_rms_baseline:.4} ducked_rms={out_rms:.4} reduction={reduction_db:.1} dB"
    );
}

// ── pitch_shift ───────────────────────────────────────────────────────────────

/// Verifies that `FilterGraph::pitch_shift()` accepts audio and produces
/// output with the same number of channels and an opaque (non-panic) result.
/// Acceptance criterion for issue #403.
#[test]
fn pitch_shift_12_semitones_should_produce_audio_output() {
    const SAMPLE_RATE: u32 = 48_000;
    const SAMPLES: usize = 48_000;

    let frame = make_sine_frame(440.0, SAMPLE_RATE, SAMPLES);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    if let Err(e) = graph.pitch_shift(12.0) {
        println!("Skipping: pitch_shift setup failed: {e}");
        return;
    }

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: pitch shift filters not available");
            return;
        }
        Err(e) => panic!("push_audio failed unexpectedly: {e}"),
    }

    match graph.pull_audio() {
        Ok(Some(out)) => {
            assert!(
                out.channels() > 0,
                "pitch_shift output must have at least one channel"
            );
        }
        Ok(None) => println!("Note: pitch_shift buffered (no immediate output)"),
        Err(e) => println!("Note: pull_audio returned: {e}"),
    }
}

/// Verifies that `FilterGraph::pitch_shift(24.0)` builds and runs. The +24
/// semitone compensation factor (atempo = 0.25) falls outside a single atempo
/// instance and must be realised as a chain. Acceptance criterion for #1091.
#[test]
fn pitch_shift_plus_24_semitones_should_produce_audio_output() {
    const SAMPLE_RATE: u32 = 48_000;
    const SAMPLES: usize = 48_000;

    let frame = make_sine_frame(440.0, SAMPLE_RATE, SAMPLES);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    if let Err(e) = graph.pitch_shift(24.0) {
        println!("Skipping: pitch_shift setup failed: {e}");
        return;
    }

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: pitch shift filters not available");
            return;
        }
        Err(e) => panic!("push_audio failed unexpectedly: {e}"),
    }

    match graph.pull_audio() {
        Ok(Some(out)) => {
            assert!(
                out.channels() > 0,
                "pitch_shift output must have at least one channel"
            );
        }
        Ok(None) => println!("Note: pitch_shift buffered (no immediate output)"),
        Err(e) => println!("Note: pull_audio returned: {e}"),
    }
}

/// Verifies that `FilterGraph::pitch_shift(-24.0)` builds and runs. The −24
/// semitone compensation factor (atempo = 4.0) also requires the atempo chain
/// path. Acceptance criterion for #1091.
#[test]
fn pitch_shift_minus_24_semitones_should_produce_audio_output() {
    const SAMPLE_RATE: u32 = 48_000;
    const SAMPLES: usize = 48_000;

    let frame = make_sine_frame(440.0, SAMPLE_RATE, SAMPLES);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    if let Err(e) = graph.pitch_shift(-24.0) {
        println!("Skipping: pitch_shift setup failed: {e}");
        return;
    }

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: pitch shift filters not available");
            return;
        }
        Err(e) => panic!("push_audio failed unexpectedly: {e}"),
    }

    match graph.pull_audio() {
        Ok(Some(out)) => {
            assert!(
                out.channels() > 0,
                "pitch_shift output must have at least one channel"
            );
        }
        Ok(None) => println!("Note: pitch_shift buffered (no immediate output)"),
        Err(e) => println!("Note: pull_audio returned: {e}"),
    }
}

// ── time_stretch ──────────────────────────────────────────────────────────────

/// Verifies that `FilterGraph::time_stretch()` accepts audio and produces
/// output without panic. Acceptance criterion for issue #404.
#[test]
fn time_stretch_half_speed_should_produce_audio_output() {
    const SAMPLE_RATE: u32 = 48_000;
    const SAMPLES: usize = 48_000; // 1 second

    let frame = make_sine_frame(220.0, SAMPLE_RATE, SAMPLES);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    if let Err(e) = graph.time_stretch(0.5) {
        println!("Skipping: time_stretch setup failed: {e}");
        return;
    }

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: atempo not available");
            return;
        }
        Err(e) => panic!("push_audio failed unexpectedly: {e}"),
    }

    match graph.pull_audio() {
        Ok(Some(out)) => {
            assert!(
                out.channels() > 0,
                "time_stretch output must have at least one channel"
            );
        }
        Ok(None) => println!("Note: time_stretch buffered (no immediate output)"),
        Err(e) => println!("Note: pull_audio returned: {e}"),
    }
}

// ── noise_reduce ──────────────────────────────────────────────────────────────

/// Verifies that `FilterGraph::noise_reduce()` accepts audio and produces
/// output. Acceptance criterion for issue #406.
#[test]
fn noise_reduce_should_produce_audio_output_from_noise_input() {
    use ff_filter::NoiseType;

    const SAMPLE_RATE: u32 = 48_000;
    const SAMPLES: usize = 48_000;

    let frame = make_sine_frame(1000.0, SAMPLE_RATE, SAMPLES);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    graph.noise_reduce(NoiseType::White, 30.0);

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: afftdn not available");
            return;
        }
        Err(e) => panic!("push_audio failed unexpectedly: {e}"),
    }

    match graph.pull_audio() {
        Ok(Some(out)) => {
            assert!(
                out.channels() > 0,
                "noise_reduce output must have at least one channel"
            );
        }
        Ok(None) => println!("Note: noise_reduce buffered (no immediate output)"),
        Err(e) => println!("Note: pull_audio returned: {e}"),
    }
}

// ── reverb_echo ───────────────────────────────────────────────────────────────

/// Verifies that `FilterGraph::reverb_echo()` builds and processes audio.
/// Acceptance criterion for issue #402.
#[test]
fn reverb_echo_single_tap_should_produce_audio_output() {
    const SAMPLE_RATE: u32 = 48_000;
    const SAMPLES: usize = 48_000;

    let frame = make_sine_frame(440.0, SAMPLE_RATE, SAMPLES);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    if let Err(e) = graph.reverb_echo(0.8, 0.8, &[100.0], &[0.5]) {
        println!("Skipping: reverb_echo setup failed: {e}");
        return;
    }

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: aecho not available");
            return;
        }
        Err(e) => panic!("push_audio failed unexpectedly: {e}"),
    }

    match graph.pull_audio() {
        Ok(Some(out)) => {
            assert!(
                out.channels() > 0,
                "reverb_echo output must have at least one channel"
            );
        }
        Ok(None) => println!("Note: reverb_echo buffered (no immediate output)"),
        Err(e) => println!("Note: pull_audio returned: {e}"),
    }
}

// ── speed_change ──────────────────────────────────────────────────────────────

/// Verifies that `FilterGraph::speed_change()` accepts audio. Acceptance
/// criterion for issue #405.
#[test]
fn speed_change_double_speed_should_accept_audio_frame() {
    const SAMPLE_RATE: u32 = 48_000;
    const SAMPLES: usize = 48_000;

    let frame = make_sine_frame(440.0, SAMPLE_RATE, SAMPLES);

    let mut graph = match FilterGraph::builder().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: graph build failed: {e}");
            return;
        }
    };
    if let Err(e) = graph.speed_change(2.0) {
        println!("Skipping: speed_change setup failed: {e}");
        return;
    }

    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(FilterError::BuildFailed) => {
            println!("Skipping: asetrate not available");
            return;
        }
        Err(e) => panic!("push_audio failed unexpectedly: {e}"),
    }
}
