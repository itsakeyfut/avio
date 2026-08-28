//! Multi-track audio mixer for real-time preview.
//!
//! [`AudioMixer`] combines `N` mono tracks into a single interleaved stereo
//! `f32` output at 48 kHz. Per-track volume and pan are controlled from any
//! thread via the cloneable [`AudioTrackHandle`].

use std::collections::VecDeque;
use std::f32::consts;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

// AudioTrack (private)

struct AudioTrack {
    buf: Arc<Mutex<VecDeque<f32>>>,
    volume: Arc<AtomicU32>,
    pan: Arc<AtomicU32>,
}

// AudioTrackHandle

/// Cloneable handle for filling a track and adjusting its gain from any thread.
///
/// Obtained by calling [`AudioMixer::add_track`]. All methods are lock-free
/// on the hot path (volume/pan reads) and only lock for buffer access.
#[derive(Clone)]
pub struct AudioTrackHandle {
    buf: Arc<Mutex<VecDeque<f32>>>,
    volume: Arc<AtomicU32>,
    pan: Arc<AtomicU32>,
}

impl AudioTrackHandle {
    /// Set per-track volume.
    ///
    /// `1.0` = unity gain. Values above `1.0` amplify; values below reduce.
    /// Negative values are clamped to `0.0` (silence). The mixer output is
    /// always clipped to `[-1.0, 1.0]`, so amplification may cause saturation
    /// on loud signals.
    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Set stereo pan. Clamped to `[-1.0` (full left) `.. +1.0` (full right)`]`.
    pub fn set_pan(&self, p: f32) {
        self.pan
            .store(p.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Push decoded mono PCM samples into the track buffer.
    ///
    /// Called by the background audio-decode thread. The samples should be
    /// `f32` mono at 48 kHz (i.e., one value per time step).
    pub fn push_samples(&self, samples: &[f32]) {
        self.buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(samples.iter().copied());
    }

    /// Number of samples currently buffered.
    ///
    /// Used by background audio threads to implement back-pressure.
    #[cfg(feature = "timeline")]
    pub(crate) fn buffered_samples(&self) -> usize {
        self.buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Drain all buffered samples.
    ///
    /// Called on seek to discard audio that is no longer relevant.
    #[cfg(feature = "timeline")]
    pub(crate) fn clear(&self) {
        self.buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

// AudioMixer

/// Multi-track, constant-power-panned stereo mixer.
///
/// Combines `N` mono tracks into a single interleaved stereo `f32` output at
/// 48 kHz.  Per-track volume and pan adjustments take effect on the next call
/// to [`mix`](Self::mix).
///
/// # Pan law
///
/// For a pan position `p ∈ [-1.0, +1.0]`:
/// ```text
/// p_norm = (p + 1.0) * π / 4
/// l_gain = volume * cos(p_norm)
/// r_gain = volume * sin(p_norm)
/// ```
/// At `p = 0` (center): `l_gain == r_gain ≈ 0.707 × volume` (constant-power
/// law — equal loudness in both ears).
///
/// # Example
///
/// ```ignore
/// let mut mixer = AudioMixer::new(48_000);
/// let track = mixer.add_track();
///
/// // Background audio-decode thread:
/// track.push_samples(&mono_pcm_chunk);
///
/// // Audio-device output callback:
/// let stereo = mixer.mix(output_buf.len());
/// output_buf[..stereo.len()].copy_from_slice(&stereo);
/// ```
pub struct AudioMixer {
    tracks: Vec<AudioTrack>,
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// Number of output channels — always 2 (stereo).
    pub channels: u16,
}

impl AudioMixer {
    /// Create a new mixer with no tracks.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            tracks: Vec::new(),
            sample_rate,
            channels: 2,
        }
    }

    /// Add a new mono track and return a cloneable handle.
    ///
    /// The track starts with `volume = 1.0` and `pan = 0.0` (center).
    pub fn add_track(&mut self) -> AudioTrackHandle {
        let buf = Arc::new(Mutex::new(VecDeque::new()));
        let volume = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let pan = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let handle = AudioTrackHandle {
            buf: Arc::clone(&buf),
            volume: Arc::clone(&volume),
            pan: Arc::clone(&pan),
        };
        self.tracks.push(AudioTrack { buf, volume, pan });
        handle
    }

    /// Mix `n_samples` interleaved stereo `f32` values from all tracks.
    ///
    /// `n_samples` is the total number of `f32` elements to produce (L + R
    /// interleaved). Tracks with insufficient buffered data are zero-padded.
    /// The output is clipped to `[-1.0, 1.0]`.
    #[allow(clippy::cast_precision_loss)]
    pub fn mix(&mut self, n_samples: usize) -> Vec<f32> {
        let n_frames = n_samples / 2;
        let mut out = vec![0.0_f32; n_frames * 2];

        for track in &self.tracks {
            let volume = f32::from_bits(track.volume.load(Ordering::Relaxed));
            let pan = f32::from_bits(track.pan.load(Ordering::Relaxed));

            // Constant-power pan law.
            let p_norm = (pan + 1.0) * consts::FRAC_PI_4;
            let l_gain = volume * p_norm.cos();
            let r_gain = volume * p_norm.sin();

            let mut guard = track
                .buf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for i in 0..n_frames {
                let s = guard.pop_front().unwrap_or(0.0);
                out[i * 2] += s * l_gain;
                out[i * 2 + 1] += s * r_gain;
            }
        }

        // Clip to [-1.0, 1.0].
        for sample in &mut out {
            *sample = sample.clamp(-1.0, 1.0);
        }

        out
    }

    /// Drain all track buffers.
    ///
    /// Called on seek to discard stale audio across all tracks.
    #[cfg(feature = "timeline")]
    pub(crate) fn invalidate_all(&mut self) {
        for track in &self.tracks {
            track
                .buf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_mixer_mix_two_tracks_should_sum_and_clip_left_channel() {
        // Two tracks, full-left pan (l_gain = volume = 1.0), amplitude 0.8.
        // Without clipping: L = 0.8 + 0.8 = 1.6. After clip: 1.0.
        let mut mixer = AudioMixer::new(48_000);
        let t1 = mixer.add_track();
        let t2 = mixer.add_track();
        t1.set_pan(-1.0);
        t2.set_pan(-1.0);
        t1.push_samples(&[0.8, 0.8]);
        t2.push_samples(&[0.8, 0.8]);

        let out = mixer.mix(4); // 2 stereo frames
        assert_eq!(out.len(), 4);
        assert!(
            (out[0] - 1.0).abs() < 1e-6,
            "L must clip to 1.0; got {}",
            out[0]
        );
        assert!(
            out[1].abs() < 1e-6,
            "R must be 0.0 for full-left pan; got {}",
            out[1]
        );
    }

    #[test]
    fn audio_mixer_pan_full_left_should_produce_zero_right_channel() {
        let mut mixer = AudioMixer::new(48_000);
        let track = mixer.add_track();
        track.set_pan(-1.0);
        track.push_samples(&[0.5, 0.5, 0.5, 0.5]);

        let out = mixer.mix(8); // 4 stereo frames
        assert_eq!(out.len(), 8);
        for i in (1..8usize).step_by(2) {
            assert!(
                out[i].abs() < 1e-6,
                "R channel must be 0.0 for full-left pan; got {} at index {i}",
                out[i]
            );
        }
    }

    #[test]
    fn audio_mixer_pan_full_right_should_produce_zero_left_channel() {
        let mut mixer = AudioMixer::new(48_000);
        let track = mixer.add_track();
        track.set_pan(1.0);
        track.push_samples(&[0.5, 0.5, 0.5, 0.5]);

        let out = mixer.mix(8);
        for i in (0..8usize).step_by(2) {
            assert!(
                out[i].abs() < 1e-6,
                "L channel must be 0.0 for full-right pan; got {} at index {i}",
                out[i]
            );
        }
    }

    #[test]
    fn audio_mixer_two_tracks_volume_sum_exceeding_one_should_be_clipped() {
        // Two tracks, volume 0.7, full-left pan, amplitude 0.8.
        // L = 0.8 * 0.7 + 0.8 * 0.7 = 1.12 > 1.0 → clipped to 1.0.
        let mut mixer = AudioMixer::new(48_000);
        let t1 = mixer.add_track();
        let t2 = mixer.add_track();
        t1.set_volume(0.7);
        t2.set_volume(0.7);
        t1.set_pan(-1.0);
        t2.set_pan(-1.0);
        t1.push_samples(&[0.8, 0.8]);
        t2.push_samples(&[0.8, 0.8]);

        let out = mixer.mix(4);
        for &s in &out {
            assert!(
                s >= -1.0 && s <= 1.0,
                "all output must be within [-1.0, 1.0]; got {s}"
            );
        }
    }

    #[test]
    fn audio_mixer_center_pan_should_apply_constant_power_law() {
        // At pan = 0.0: p_norm = π/4, cos = sin = 1/√2 ≈ 0.7071.
        let mut mixer = AudioMixer::new(48_000);
        let track = mixer.add_track();
        // pan = 0 (center) by default, volume = 1.0 by default.
        track.push_samples(&[1.0]);

        let out = mixer.mix(2); // 1 stereo frame
        let expected = (std::f32::consts::FRAC_PI_4).cos(); // ≈ 0.7071
        assert!(
            (out[0] - expected).abs() < 1e-5,
            "L at center should be cos(π/4) ≈ {expected:.5}; got {}",
            out[0]
        );
        assert!(
            (out[1] - expected).abs() < 1e-5,
            "R at center should be sin(π/4) ≈ {expected:.5}; got {}",
            out[1]
        );
    }

    #[test]
    fn audio_mixer_underrun_should_zero_pad_remaining_frames() {
        let mut mixer = AudioMixer::new(48_000);
        let track = mixer.add_track();
        track.set_pan(-1.0); // full left for determinism
        track.push_samples(&[0.5]); // only one sample, but we request 4 frames

        let out = mixer.mix(8);
        assert_eq!(out.len(), 8);

        // Frames 1-3 must be zero (underrun).
        for i in 2..8 {
            assert_eq!(out[i], 0.0, "underrun frame must be silent; got {}", out[i]);
        }
    }

    #[test]
    fn audio_mixer_empty_tracks_should_produce_silence() {
        let mut mixer = AudioMixer::new(48_000);
        let _track = mixer.add_track();
        let out = mixer.mix(8);
        assert_eq!(out.len(), 8);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "empty track must produce silence"
        );
    }

    #[cfg(feature = "timeline")]
    #[test]
    fn audio_mixer_invalidate_all_should_clear_all_buffers() {
        let mut mixer = AudioMixer::new(48_000);
        let t1 = mixer.add_track();
        let t2 = mixer.add_track();
        t1.push_samples(&[0.5, 0.5]);
        t2.push_samples(&[0.5, 0.5]);

        mixer.invalidate_all();

        let out = mixer.mix(4);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "after invalidate_all, mix must be silent"
        );
    }

    #[test]
    fn audio_track_handle_set_volume_above_one_should_amplify() {
        let mut mixer = AudioMixer::new(48_000);
        let track = mixer.add_track();
        track.set_volume(2.0);
        track.set_pan(-1.0); // full left for determinism
        track.push_samples(&[0.4]);
        let out = mixer.mix(2); // 1 stereo frame
        // L = 0.4 * 2.0 = 0.8 (below clip threshold).
        assert!(
            (out[0] - 0.8).abs() < 1e-5,
            "volume 2.0 should amplify to 0.8; got {}",
            out[0]
        );
    }

    #[test]
    fn audio_track_handle_set_negative_volume_should_be_silent() {
        let mut mixer = AudioMixer::new(48_000);
        let track = mixer.add_track();
        track.set_volume(-1.0); // clamped to 0.0
        track.push_samples(&[1.0]);
        let out = mixer.mix(2);
        assert!(
            out.iter().all(|&s| s.abs() < 1e-6),
            "negative volume must be silent"
        );
    }

    #[cfg(feature = "timeline")]
    #[test]
    fn audio_track_handle_clear_should_drain_buffered_samples() {
        let mut mixer = AudioMixer::new(48_000);
        let track = mixer.add_track();
        track.push_samples(&[0.5, 0.5, 0.5, 0.5]);
        assert_eq!(track.buffered_samples(), 4);
        track.clear();
        assert_eq!(
            track.buffered_samples(),
            0,
            "clear() must drain all samples"
        );
    }
}
