//! Audio decode-thread spawning and linear resampling for timeline playback.
//!
//! [`spawn_audio_track_thread`] runs one [`AudioDecoder`] per audio-bearing clip,
//! applies the per-clip fade envelope and speed resampling, and pushes mono
//! samples into the mixer track. [`resample_linear`] is the preview-quality
//! (no pitch correction) resampler it uses for non-1.0 speeds.

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
                        // ── Speed resampling (linear interpolation) ────────────
                        // For speed > 1.0: fewer output samples (fast motion, pitch up).
                        // For speed < 1.0: more output samples (slow motion, pitch down).
                        // This is a simple preview-quality resample; no pitch correction.
                        let samples: &[f32];
                        let resampled: Vec<f32>;
                        if apply_speed {
                            resampled = resample_linear(raw, speed, &mut speed_phase);
                            samples = &resampled;
                        } else {
                            samples = raw;
                        }

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
