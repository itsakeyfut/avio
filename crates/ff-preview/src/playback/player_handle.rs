//! Shared, cloneable control handle for a running [`PlayerRunner`](super::player_runner::PlayerRunner).

#[cfg(feature = "timeline")]
use ff_pipeline::timeline::Timeline;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

pub(crate) use super::player::DECODED_SAMPLE_RATE;
use super::player::PlayerCommand;
use crate::audio::AudioMixer;
use crate::event::PlayerEvent;

// ── PlayerHandle ─────────────────────────────────────────────────────────────

/// Shared, cloneable handle to a running [`PlayerRunner`].
///
/// All methods are non-blocking. Commands that cannot be queued immediately
/// (channel full) are silently dropped.
///
/// # Thread safety
///
/// `PlayerHandle` is `Clone + Send + Sync` and can be shared freely across
/// threads without locking.
#[derive(Clone)]
pub struct PlayerHandle {
    pub(crate) cmd_tx: mpsc::SyncSender<PlayerCommand>,
    pub(crate) event_rx: Arc<Mutex<mpsc::Receiver<PlayerEvent>>>,
    /// Current PTS in microseconds. Written by [`PlayerRunner`] on each frame.
    pub(crate) current_pts: Arc<AtomicU64>,
    pub(crate) audio_buf: Option<Arc<Mutex<VecDeque<f32>>>>,
    /// Advances the audio master clock when `pop_audio_samples` drains samples.
    pub(crate) samples_consumed: Option<Arc<AtomicU64>>,
    /// Mirrors the runner's paused state; updated immediately by `play`/`pause`.
    pub(crate) paused: Arc<AtomicBool>,
    /// Mirrors the runner's stopped state; updated immediately by `stop`.
    pub(crate) stopped: Arc<AtomicBool>,
    pub(crate) duration_millis: u64,
    /// Multi-track mixer — present when the runner was created by `TimelinePlayer`.
    pub(crate) audio_mixer: Option<Arc<Mutex<AudioMixer>>>,
}

impl PlayerHandle {
    /// Resume playback.
    pub fn play(&self) {
        self.stopped.store(false, Ordering::Release);
        self.paused.store(false, Ordering::Release);
        let _ = self.cmd_tx.try_send(PlayerCommand::Play);
    }

    /// Pause playback.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
        let _ = self.cmd_tx.try_send(PlayerCommand::Pause);
    }

    /// Stop the presentation loop.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.cmd_tx.try_send(PlayerCommand::Stop);
    }

    /// Seek to `pts`.
    ///
    /// Consecutive calls before the runner processes them are coalesced —
    /// only the most recent `pts` executes.
    pub fn seek(&self, pts: Duration) {
        let _ = self.cmd_tx.try_send(PlayerCommand::Seek(pts));
    }

    /// Set the playback rate.
    ///
    /// - Positive values play forward at the given speed multiplier (e.g. `2.0` = 2×).
    /// - Negative values play in reverse at `abs(rate)` speed (e.g. `-1.0` = 1× reverse).
    ///   Audio is muted during reverse playback and automatically resumes on the next
    ///   positive-rate call.
    /// - `0.0` is ignored.
    pub fn set_rate(&self, rate: f64) {
        let _ = self.cmd_tx.try_send(PlayerCommand::SetRate(rate));
    }

    /// Set the A/V offset correction in milliseconds.
    ///
    /// Positive: video PTS is shifted down relative to audio (video appears
    /// delayed). Negative: video PTS is shifted up (audio appears delayed).
    pub fn set_av_offset(&self, ms: i64) {
        let _ = self.cmd_tx.try_send(PlayerCommand::SetAvOffset(ms));
    }

    /// Replace the running timeline's clip layout in place.
    ///
    /// Sends a [`PlayerCommand::UpdateLayout`] to `TimelineRunner`. The runner
    /// updates `timeline_start` / `timeline_end` / `in_point` / `out_point` for
    /// every existing clip, stops audio decode threads, and seeks all decode
    /// buffers to the last known media PTS — so the next presented frame is
    /// spatially correct after the move.
    ///
    /// The `MasterClock` and `paused` / `stopped` atomics are unaffected.
    /// Drops silently if the command channel (capacity 64) is full.
    ///
    /// No-op when called on a [`PlayerRunner`]-backed handle (single-track
    /// player). Only `TimelineRunner` handles this command.
    #[cfg(feature = "timeline")]
    pub fn update_timeline(&self, timeline: Timeline) {
        let _ = self
            .cmd_tx
            .try_send(PlayerCommand::UpdateLayout(Box::new(timeline)));
    }

    /// PTS of the most recently presented frame.
    ///
    /// Returns [`Duration::ZERO`] before the first frame is presented.
    #[must_use]
    pub fn current_pts(&self) -> Duration {
        Duration::from_micros(self.current_pts.load(Ordering::Relaxed))
    }

    /// Container-reported duration, or `None` for live / streaming sources.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        if self.duration_millis == u64::MAX {
            None
        } else {
            Some(Duration::from_millis(self.duration_millis))
        }
    }

    /// Sample rate of the PCM data returned by [`pop_audio_samples`](Self::pop_audio_samples).
    ///
    /// Returns `Some(48_000)` for files that contain an audio stream, and
    /// `None` for video-only files (where `pop_audio_samples` always returns
    /// an empty `Vec`).
    ///
    /// Use this to configure your audio backend without hardcoding a magic
    /// constant:
    ///
    /// ```ignore
    /// let cfg = cpal::StreamConfig {
    ///     channels: 2,
    ///     sample_rate: cpal::SampleRate(handle.audio_sample_rate().unwrap_or(48_000)),
    ///     ..Default::default()
    /// };
    /// ```
    #[must_use]
    pub fn audio_sample_rate(&self) -> Option<u32> {
        self.audio_buf.as_ref().map(|_| DECODED_SAMPLE_RATE)
    }

    /// Pull up to `n` interleaved stereo `f32` PCM samples at 48 kHz.
    ///
    /// Returns an empty `Vec` when:
    /// - playback is paused or stopped,
    /// - `n` is 0,
    /// - there is no audio track, or
    /// - the ring buffer is empty (underrun — caller should output silence).
    ///
    /// Advances the audio master clock by `samples.len() / 2` stereo frames.
    #[allow(clippy::cast_precision_loss)]
    pub fn pop_audio_samples(&self, n: usize) -> Vec<f32> {
        if self.paused.load(Ordering::Relaxed) || self.stopped.load(Ordering::Relaxed) {
            return Vec::new();
        }
        if n == 0 {
            return Vec::new();
        }
        // Mixer path — used when the handle was created by TimelinePlayer.
        // The timeline clock is System-based so samples_consumed is not advanced here.
        if let Some(mixer) = &self.audio_mixer {
            return mixer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .mix(n);
        }
        // Legacy ring-buffer path — used by PlayerRunner (single-track audio).
        let Some(buf) = &self.audio_buf else {
            return Vec::new();
        };
        let mut guard = buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let take = n.min(guard.len());
        if take == 0 {
            return Vec::new();
        }
        let samples: Vec<f32> = guard.drain(..take).collect();
        if let Some(sc) = &self.samples_consumed {
            sc.fetch_add((take / 2) as u64, Ordering::Relaxed);
        }
        samples
    }

    /// Pull up to `pop_n` interleaved stereo `f32` PCM samples at 48 kHz and
    /// advance the A/V sync clock by exactly `clock_stereo_pairs` — independent
    /// of how many samples are actually available in the ring buffer.
    ///
    /// Use this instead of [`pop_audio_samples`](Self::pop_audio_samples) when
    /// playing at rates other than 1×.  The cpal callback pops `out_len * rate`
    /// decoded samples to drive rate-scaled audio, but the master clock must
    /// still advance at the **hardware** output rate (`out_len / 2` per callback)
    /// so that `MasterClock::Audio`'s `pts_base + delta / sr * rate` formula
    /// yields the correct media PTS without double-counting the rate.
    ///
    /// # Arguments
    ///
    /// * `pop_n` — decoded samples to drain from the ring buffer
    ///   (`output_buf.len() * rate`, rounded).
    /// * `clock_stereo_pairs` — hardware stereo pairs to add to the sync counter
    ///   (`output_buf.len() / 2`, constant regardless of rate).
    #[allow(clippy::cast_precision_loss)]
    pub fn pop_audio_samples_for_rate(&self, pop_n: usize, clock_stereo_pairs: u64) -> Vec<f32> {
        if self.paused.load(Ordering::Relaxed) || self.stopped.load(Ordering::Relaxed) {
            // Clock still advances — the hardware keeps running even during silence.
            if let Some(sc) = &self.samples_consumed {
                sc.fetch_add(clock_stereo_pairs, Ordering::Relaxed);
            }
            return Vec::new();
        }
        if pop_n == 0 {
            if let Some(sc) = &self.samples_consumed {
                sc.fetch_add(clock_stereo_pairs, Ordering::Relaxed);
            }
            return Vec::new();
        }
        // Mixer path (TimelinePlayer) — System clock, no samples_consumed tracking.
        if let Some(mixer) = &self.audio_mixer {
            return mixer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .mix(pop_n);
        }
        // Ring-buffer path (PlayerRunner single-track audio).
        let Some(buf) = &self.audio_buf else {
            if let Some(sc) = &self.samples_consumed {
                sc.fetch_add(clock_stereo_pairs, Ordering::Relaxed);
            }
            return Vec::new();
        };
        let mut guard = buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let take = pop_n.min(guard.len());
        let samples: Vec<f32> = if take > 0 {
            guard.drain(..take).collect()
        } else {
            Vec::new()
        };
        drop(guard);
        // Advance the clock by the hardware output size, not the decoded drain size.
        if let Some(sc) = &self.samples_consumed {
            sc.fetch_add(clock_stereo_pairs, Ordering::Relaxed);
        }
        samples
    }

    /// Poll for the next [`PlayerEvent`] without blocking.
    ///
    /// Returns `None` when no events are pending.
    #[must_use]
    pub fn poll_event(&self) -> Option<PlayerEvent> {
        self.event_rx.lock().ok()?.try_recv().ok()
    }

    /// Block until the next [`PlayerEvent`] arrives or the channel closes.
    ///
    /// Returns `None` when the runner has exited and all events have been
    /// drained. Intended for use inside `spawn_blocking`.
    #[must_use]
    pub fn recv_event(&self) -> Option<PlayerEvent> {
        self.event_rx.lock().ok()?.recv().ok()
    }

    /// Construct a handle for a non-`PlayerRunner` runner (e.g., `TimelineRunner`).
    ///
    /// Audio fields are set to `None`; the handle's
    /// [`pop_audio_samples`](Self::pop_audio_samples) always returns an empty `Vec`.
    #[cfg(feature = "timeline")]
    pub(crate) fn for_timeline(
        cmd_tx: mpsc::SyncSender<PlayerCommand>,
        event_rx: Arc<Mutex<mpsc::Receiver<PlayerEvent>>>,
        current_pts: Arc<AtomicU64>,
        paused: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
        duration_millis: u64,
        audio_mixer: Option<Arc<Mutex<AudioMixer>>>,
    ) -> Self {
        Self {
            cmd_tx,
            event_rx,
            current_pts,
            audio_buf: None,
            samples_consumed: None,
            audio_mixer,
            paused,
            stopped,
            duration_millis,
        }
    }
}
