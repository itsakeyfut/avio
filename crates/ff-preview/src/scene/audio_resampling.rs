//! Audio decode-thread spawning, pitch shifting, and linear resampling for
//! timeline playback.
//!
//! [`spawn_audio_track_thread`] runs one [`AudioDecoder`] per audio-bearing clip,
//! applies the per-clip pitch shift, fade envelope, and speed resampling, and
//! pushes mono samples into the mixer track. [`resample_linear`] is the
//! preview-quality resampler used for non-1.0 speeds; [`PitchShifter`] is the
//! duration-preserving pitch shifter (preview quality, hand-rolled so the audio
//! thread stays pure-Rust).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ff_decode::{AudioDecoder, SeekMode};
use ff_format::SampleFormat;

use crate::audio::AudioTrackHandle;

use super::state::AudioFadeConfig;

/// Back-pressure limit for the audio decode thread (mono samples).
const AUDIO_MAX_BUF: usize = 96_000;
/// Sample rate used for all audio decode threads.
const AUDIO_SAMPLE_RATE: f64 = 48_000.0;

/// Linear-interpolation resample of a mono `f32` slice.
///
/// Consumes `speed` input samples per output sample:
/// - `speed > 1.0` → fewer output samples (fast motion, pitch raised)
/// - `speed < 1.0` → more output samples (slow motion, pitch lowered)
///
/// `phase` carries the fractional position across chunk boundaries so the
/// resampling is seamless across successive calls with consecutive chunks.
///
/// Preview quality only — no pitch correction.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn resample_linear(input: &[f32], speed: f64, phase: &mut f64) -> Vec<f32> {
    let capacity = ((input.len() as f64 / speed) + 1.0) as usize;
    let mut out = Vec::with_capacity(capacity);
    let mut pos = *phase;
    let len = input.len();
    while pos < len as f64 {
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let s = if idx + 1 < len {
            input[idx] * (1.0 - frac) + input[idx + 1] * frac
        } else {
            input[idx]
        };
        out.push(s);
        pos += speed;
    }
    // Carry the fractional overshoot into the next chunk.
    *phase = pos - len as f64;
    out
}

/// Grain (analysis/synthesis window) length for the OLA pitch shifter.
const PITCH_GRAIN: usize = 1024;
/// Synthesis hop = 50% overlap. Hann at this hop is constant-overlap-add
/// (adjacent windows sum to unity), so synthesis has unity gain.
const PITCH_HOP: usize = PITCH_GRAIN / 2;

/// Pitch-shift ratio for `semitones`, clamped to the export range (+/-24).
/// `2^(semitones/12)`: `+12` -> `2.0` (octave up), `-12` -> `0.5` (octave down).
fn pitch_ratio(semitones: f64) -> f64 {
    2f64.powf(semitones.clamp(-24.0, 24.0) / 12.0)
}

/// Preview-quality, duration-preserving pitch shifter for one mono `f32` stream.
///
/// Pitch shift = OLA time-stretch by the pitch ratio `r`, then linear resample by
/// `r`. The stretch changes duration (not pitch), the resample changes both, and
/// the two compose to `pitch x r` at the original duration. Preview quality only
/// (plain overlap-add, no phase-locking); artifacts are acceptable here — this is
/// deliberately not the export `asetrate`/`atempo` path (which needs libavfilter
/// and an EOF flush) so the audio thread stays pure-Rust and CI-testable.
///
/// State is carried across [`process`](Self::process) calls so output is seamless
/// across the decoder's successive chunks (like [`resample_linear`]'s `phase`).
///
/// Introduces ~one grain of onset latency and drops the final partial grain at
/// stream end; both are inaudible and acceptable at preview quality (audio is not
/// sample-accurate against video for a pitched clip, only model-accurate).
struct PitchShifter {
    ratio: f64,
    /// Analysis hop = synthesis hop / ratio (fractional). `r > 1` reads grains
    /// closer together (more overlap-add repeats -> longer stretch).
    analysis_hop: f64,
    /// Hann window, precomputed once.
    window: Vec<f32>,
    /// Unconsumed input; a grain reads `PITCH_GRAIN` samples from `in_pos`.
    in_buf: Vec<f32>,
    /// Fractional start of the next grain, relative to the front of `in_buf`.
    in_pos: f64,
    /// Overlap-add carry: the second half of the last grain, awaiting the next.
    ola_tail: Vec<f32>,
    /// Fractional phase for the resample stage, carried across calls.
    resample_phase: f64,
}

impl PitchShifter {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn new(semitones: f64) -> Self {
        let clamped = semitones.clamp(-24.0, 24.0);
        if (clamped - semitones).abs() > f64::EPSILON {
            log::warn!("preview pitch clamped to +/-24 semitones from={semitones}");
        }
        let ratio = pitch_ratio(clamped);
        // Hann window: sin^2(pi n / N) == 0.5 (1 - cos(2 pi n / N)).
        let window = (0..PITCH_GRAIN)
            .map(|n| {
                let s = (std::f64::consts::PI * n as f64 / PITCH_GRAIN as f64).sin();
                (s * s) as f32
            })
            .collect();
        Self {
            ratio,
            analysis_hop: PITCH_HOP as f64 / ratio,
            window,
            in_buf: Vec::new(),
            in_pos: 0.0,
            ola_tail: vec![0.0; PITCH_HOP],
            resample_phase: 0.0,
        }
    }

    /// Pitch-shift one chunk, returning ~`input.len()` samples (duration preserved).
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.in_buf.extend_from_slice(input);

        // OLA time-stretch by `ratio`: emit HOP finalized samples per grain (the
        // previous grain's windowed second half overlap-added with this grain's
        // windowed first half), carry this grain's second half for the next.
        let mut stretched: Vec<f32> = Vec::new();
        while self.in_pos as usize + PITCH_GRAIN < self.in_buf.len() {
            let base = self.in_pos;
            for k in 0..PITCH_HOP {
                let g_a = self.grain_sample(base, k);
                let g_b = self.grain_sample(base, k + PITCH_HOP);
                stretched.push(self.ola_tail[k] + g_a);
                self.ola_tail[k] = g_b;
            }
            self.in_pos += self.analysis_hop;
        }

        // Drop the consumed input prefix; the next grain starts at floor(in_pos).
        let consumed = self.in_pos as usize;
        if consumed > 0 {
            self.in_buf.drain(0..consumed);
            self.in_pos -= consumed as f64;
        }

        // Resample by `ratio` -> net pitch x ratio at the original duration.
        resample_linear(&stretched, self.ratio, &mut self.resample_phase)
    }

    /// Windowed, linearly-interpolated sample at fractional `base + k` of `in_buf`.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn grain_sample(&self, base: f64, k: usize) -> f32 {
        let pos = base + k as f64;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let s = if idx + 1 < self.in_buf.len() {
            self.in_buf[idx] * (1.0 - frac) + self.in_buf[idx + 1] * frac
        } else if idx < self.in_buf.len() {
            self.in_buf[idx]
        } else {
            0.0
        };
        s * self.window[k]
    }
}

pub(super) fn spawn_audio_track_thread(
    path: PathBuf,
    start_pts: Duration,
    track: AudioTrackHandle,
    cancel: Arc<AtomicBool>,
    fades: AudioFadeConfig,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut decoder = match AudioDecoder::open(&path)
            .output_format(SampleFormat::F32)
            .output_sample_rate(48_000)
            .output_channels(1) // mono — the mixer applies panning
            .build()
        {
            Ok(d) => d,
            Err(e) => {
                log::warn!("timeline audio thread open failed error={e}");
                return;
            }
        };

        if start_pts > Duration::ZERO
            && let Err(e) = decoder.seek(start_pts, SeekMode::Backward)
        {
            log::warn!("timeline audio seek failed pts={start_pts:?} error={e}");
        }

        let speed = fades.speed.max(0.01);
        let apply_speed = (speed - 1.0).abs() > 1e-6;
        // Fractional position within the current source chunk carried across iterations.
        let mut speed_phase: f64 = 0.0;
        // Per-clip pitch shift (semitones), applied duration-preserving before the
        // speed resample so the fade envelope (computed from output samples) is
        // unaffected. `None` = no pitch.
        let mut pitch_shifter = (fades.pitch.abs() > 1e-9).then(|| PitchShifter::new(fades.pitch));

        // All fade/total timings are expressed in TIMELINE time (= source time / speed)
        // so that `samples_pushed / AUDIO_SAMPLE_RATE` (output time) lines up correctly.
        let inv_speed = 1.0 / speed;
        let fade_in_secs = fades.fade_in.as_secs_f64() * inv_speed;
        let fade_out_secs = fades.fade_out.as_secs_f64() * inv_speed;
        let total_secs = fades.clip_dur.as_secs_f64() * inv_speed;
        // Elapsed output time at thread start due to seeking past in_point.
        let seek_offset_secs = start_pts.saturating_sub(fades.in_point).as_secs_f64() * inv_speed;
        let apply_fades = fade_in_secs > 0.0 || fade_out_secs > 0.0;
        let mut samples_pushed: u64 = 0;

        loop {
            if cancel.load(Ordering::Acquire) {
                break;
            }

            // Back-pressure: pause decoding when the buffer is full.
            if track.buffered_samples() >= AUDIO_MAX_BUF {
                thread::sleep(Duration::from_millis(1));
                continue;
            }

            match decoder.decode_one() {
                Ok(Some(frame)) => {
                    if let Some(raw) = frame.as_f32()
                        && !raw.is_empty()
                    {
                        // Pitch shift (duration-preserving), then speed resampling.
                        let pitched: Vec<f32>;
                        let pre_speed: &[f32] = if let Some(ps) = pitch_shifter.as_mut() {
                            pitched = ps.process(raw);
                            &pitched
                        } else {
                            raw
                        };

                        // Speed resampling (linear interpolation)
                        // For speed > 1.0: fewer output samples (fast motion, pitch up).
                        // For speed < 1.0: more output samples (slow motion, pitch down).
                        // This is a simple preview-quality resample; no pitch correction.

                        let resampled: Vec<f32>;
                        let samples: &[f32] = if apply_speed {
                            resampled = resample_linear(pre_speed, speed, &mut speed_phase);
                            &resampled
                        } else {
                            pre_speed
                        };

                        if apply_fades {
                            let mut buf: Vec<f32> = samples.to_vec();
                            for (i, s) in buf.iter_mut().enumerate() {
                                // u64→f64 loses precision for very large sample counts
                                // (>2^52 samples ≈ 1.9M years at 48 kHz); acceptable here.
                                #[allow(clippy::cast_precision_loss)]
                                let pos_secs = seek_offset_secs
                                    + (samples_pushed + i as u64) as f64 / AUDIO_SAMPLE_RATE;

                                // f64→f32: gain values are in [0.0, 1.0]; truncation is inaudible.
                                #[allow(clippy::cast_possible_truncation)]
                                let gain_in = if fade_in_secs > 0.0 && pos_secs < fade_in_secs {
                                    (pos_secs / fade_in_secs) as f32
                                } else {
                                    1.0_f32
                                };

                                #[allow(clippy::cast_possible_truncation)]
                                let gain_out = if fade_out_secs > 0.0
                                    && total_secs > 0.0
                                    && pos_secs >= total_secs - fade_out_secs
                                {
                                    let elapsed = pos_secs - (total_secs - fade_out_secs);
                                    (1.0 - elapsed / fade_out_secs).clamp(0.0, 1.0) as f32
                                } else {
                                    1.0_f32
                                };

                                *s *= gain_in * gain_out;
                            }
                            samples_pushed += buf.len() as u64;
                            track.push_samples(&buf);
                        } else {
                            #[allow(clippy::cast_possible_truncation)]
                            {
                                samples_pushed += samples.len() as u64;
                            }
                            track.push_samples(samples);
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    log::warn!("timeline audio decode error error={e}");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin() as f32)
            .collect()
    }

    /// Estimate frequency (Hz) from positive-going zero crossings.
    fn estimate_freq(samples: &[f32], rate: f64) -> f64 {
        let crossings = samples
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        crossings as f64 / (samples.len() as f64 / rate)
    }

    #[test]
    fn pitch_ratio_should_be_2_at_12_semitones() {
        assert!((pitch_ratio(12.0) - 2.0).abs() < 1e-9);
        assert!((pitch_ratio(-12.0) - 0.5).abs() < 1e-9);
        assert!((pitch_ratio(0.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn pitch_ratio_should_clamp_to_24_semitones() {
        assert!((pitch_ratio(100.0) - pitch_ratio(24.0)).abs() < 1e-12);
        assert!((pitch_ratio(-100.0) - pitch_ratio(-24.0)).abs() < 1e-12);
    }

    #[test]
    fn pitch_shift_should_raise_fundamental_frequency() {
        let rate = 48_000.0;
        let f0 = 1000.0;
        let input = sine(f0, rate, 48_000); // 1 s
        let mut ps = PitchShifter::new(12.0); // +1 octave -> x2
        let out = ps.process(&input);
        // Skip the onset/tail grains where the window ramps up/down.
        let mid = &out[PITCH_GRAIN..out.len().saturating_sub(PITCH_GRAIN)];
        let f = estimate_freq(mid, rate);
        assert!(
            (f - 2.0 * f0).abs() < 0.1 * 2.0 * f0,
            "expected ~{} Hz, got {f} Hz",
            2.0 * f0
        );
    }

    #[test]
    fn pitch_shift_should_preserve_sample_count() {
        let input = sine(440.0, 48_000.0, 24_000);
        let mut ps = PitchShifter::new(7.0);
        let out = ps.process(&input);
        let ratio = out.len() as f64 / input.len() as f64;
        assert!(
            (0.85..=1.15).contains(&ratio),
            "duration should be preserved; out/in = {ratio}"
        );
    }

    #[test]
    fn pitch_shift_zero_semitones_should_preserve_fundamental_frequency() {
        let rate = 48_000.0;
        let input = sine(440.0, rate, 24_000);
        let mut ps = PitchShifter::new(0.0);
        let out = ps.process(&input);
        let mid = &out[PITCH_GRAIN..out.len().saturating_sub(PITCH_GRAIN)];
        let f = estimate_freq(mid, rate);
        assert!((f - 440.0).abs() < 44.0, "expected ~440 Hz, got {f} Hz");
    }
}
