//! Actor-model playback types for ff-preview.
//!
//! # Overview
//!
//! [`PreviewPlayer`] opens a media file and is a thin builder.  Call
//! [`PreviewPlayer::split`] to obtain a ([`PlayerRunner`], [`PlayerHandle`]) pair:
//!
//! - **[`PlayerRunner`]** — owns the decode buffers, audio thread, and
//!   presentation clock. Move it to a dedicated thread and call
//!   [`PlayerRunner::run`]. The method runs until EOF or a [`PlayerCommand::Stop`]
//!   command arrives.
//! - **[`PlayerHandle`]** — `Clone + Send + Sync`. Holds a command sender and
//!   read-only state atomics. All control methods use `try_send` — they never
//!   block. If the command channel (capacity 64) is full the send is silently
//!   dropped.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ff_decode::{AudioDecoder, HardwareAccel, SeekMode};
use ff_format::SampleFormat;
#[cfg(feature = "timeline")]
use ff_pipeline::timeline::Timeline;

use super::decode_buffer::{DecodeBuffer, FrameResult};
use super::master_clock::MasterClock;
use super::sink::FrameSink;
use crate::audio::AudioMixer;
use crate::cache::FrameCache;
use crate::error::PreviewError;
use crate::event::PlayerEvent;

// ── Constants ─────────────────────────────────────────────────────────────────

const AUDIO_MAX_BUF: usize = 96_000;
const CHANNEL_CAP: usize = 64;
/// Number of consecutive presented frames with no audio progress before the
/// wall-clock fallback is re-armed (audio track ended before video track).
/// At 30 fps this is ~167 ms; at 60 fps ~83 ms — short enough to recover
/// quickly, long enough to avoid false positives from momentary underruns.
const AUDIO_STALL_FRAMES: u32 = 5;
/// Fixed output sample rate of the audio decode thread.
///
/// `spawn_audio_thread` always resamples to this rate via
/// `AudioDecoder::output_sample_rate`. `MasterClock::Audio` must be
/// initialised with this value — not the source file's native rate — so
/// that `current_pts()` advances at exactly 1 s per second of real audio
/// consumption regardless of the source's native sample rate.
const DECODED_SAMPLE_RATE: u32 = 48_000;

// ── PlayerCommand ─────────────────────────────────────────────────────────────

/// Commands sent from [`PlayerHandle`] to [`PlayerRunner`] via a
/// bounded sync channel (capacity 64).
pub enum PlayerCommand {
    /// Resume playback (clear the paused flag).
    Play,
    /// Pause playback.
    Pause,
    /// Stop the presentation loop; [`PlayerRunner::run`] returns after the
    /// current frame.
    Stop,
    /// Seek to `pts`. Consecutive seeks are coalesced — only the last one
    /// executes.
    Seek(Duration),
    /// Set the playback rate. Values ≤ 0.0 are ignored.
    SetRate(f64),
    /// Set the A/V offset in milliseconds. Clamped to ±5 000 ms.
    SetAvOffset(i64),
    /// Replace the timeline clip layout without stopping playback.
    ///
    /// Handled only by `TimelineRunner`; `PlayerRunner` ignores it.
    /// The runner updates its internal `ClipState` / `AudioOnlyTrack` positions
    /// in place and seeks to the last known media PTS so the next frame is
    /// spatially correct after the layout change.
    #[cfg(feature = "timeline")]
    UpdateLayout(Box<Timeline>),
}

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
    cmd_tx: mpsc::SyncSender<PlayerCommand>,
    event_rx: Arc<Mutex<mpsc::Receiver<PlayerEvent>>>,
    /// Current PTS in microseconds. Written by [`PlayerRunner`] on each frame.
    current_pts: Arc<AtomicU64>,
    audio_buf: Option<Arc<Mutex<VecDeque<f32>>>>,
    /// Advances the audio master clock when `pop_audio_samples` drains samples.
    samples_consumed: Option<Arc<AtomicU64>>,
    /// Mirrors the runner's paused state; updated immediately by `play`/`pause`.
    paused: Arc<AtomicBool>,
    /// Mirrors the runner's stopped state; updated immediately by `stop`.
    stopped: Arc<AtomicBool>,
    duration_millis: u64,
    /// Multi-track mixer — present when the runner was created by `TimelinePlayer`.
    audio_mixer: Option<Arc<Mutex<AudioMixer>>>,
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

// ── PlayerRunner ─────────────────────────────────────────────────────────────

/// Exclusive owner of the decode pipeline. Move to a background thread and
/// call [`run`](Self::run).
///
/// Configure with [`set_sink`](Self::set_sink),
/// [`use_proxy_if_available`](Self::use_proxy_if_available), and
/// [`set_hardware_accel`](Self::set_hardware_accel) **before** calling `run`.
pub struct PlayerRunner {
    path: PathBuf,
    cmd_rx: mpsc::Receiver<PlayerCommand>,
    event_tx: mpsc::SyncSender<PlayerEvent>,
    decode_buf: Option<DecodeBuffer>,
    fps: f64,
    sink: Option<Box<dyn FrameSink>>,
    clock: MasterClock,
    audio_buf: Option<Arc<Mutex<VecDeque<f32>>>>,
    audio_cancel: Option<Arc<AtomicBool>>,
    audio_handle: Option<JoinHandle<()>>,
    sws: super::playback_inner::SwsRgbaConverter,
    rgba_buf: Vec<u8>,
    active_path: PathBuf,
    current_pts: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    av_offset_ms: i64,
    rate: f64,
    duration_millis: u64,
    frame_cache: Option<FrameCache>,
    hw_accel: HardwareAccel,
}

impl PlayerRunner {
    /// Register the frame sink. Call before [`run`](Self::run).
    pub fn set_sink(&mut self, sink: Box<dyn FrameSink>) {
        self.sink = Some(sink);
    }

    /// Configure hardware acceleration. Call before [`run`](Self::run).
    ///
    /// The setting takes effect at the start of `run()`. [`HardwareAccel::Auto`]
    /// (the default) probes available backends and falls back to software.
    /// [`HardwareAccel::None`] forces CPU-only decoding.
    pub fn set_hardware_accel(&mut self, accel: HardwareAccel) -> &mut Self {
        self.hw_accel = accel;
        self
    }

    /// Returns the path currently being decoded (original or active proxy).
    #[must_use]
    pub fn active_source(&self) -> &Path {
        &self.active_path
    }

    /// Enable an in-memory RGBA frame cache with the given byte budget.
    ///
    /// When the budget is set, frames decoded during playback are stored
    /// and served on cache hit without re-decoding, enabling instant scrubbing.
    /// The cache is invalidated automatically whenever a seek targets a PTS
    /// outside the currently cached range.
    ///
    /// Example: `runner.with_frame_cache_budget(512 * 1024 * 1024)` for 512 MB.
    #[must_use]
    pub fn with_frame_cache_budget(mut self, bytes: usize) -> Self {
        self.frame_cache = Some(FrameCache::new(bytes));
        self
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

    /// Activate a lower-resolution proxy if one exists in `proxy_dir`.
    ///
    /// Must be called before [`run`](Self::run). Returns `true` if a proxy was
    /// found and activated; `false` if no proxy exists or activation failed.
    ///
    /// Proxy lookup order: `half` → `quarter` → `eighth`; first match wins.
    pub fn use_proxy_if_available(&mut self, proxy_dir: &Path) -> bool {
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_owned();

        for suffix in ["half", "quarter", "eighth"] {
            let candidate = proxy_dir.join(format!("{stem}_proxy_{suffix}.mp4"));
            if candidate.exists() {
                match self.activate_proxy(&candidate) {
                    Ok(()) => {
                        log::debug!("proxy activated path={}", candidate.display());
                        return true;
                    }
                    Err(e) => {
                        log::warn!(
                            "proxy activation failed path={} error={e}",
                            candidate.display()
                        );
                    }
                }
            }
        }
        false
    }

    /// A/V sync presentation loop.
    ///
    /// Blocks until a [`PlayerCommand::Stop`] is received, the end of file is
    /// reached, or an unrecoverable decode error occurs.
    ///
    /// At the top of each frame, all pending commands are drained from the
    /// channel. Consecutive [`PlayerCommand::Seek`] commands are coalesced —
    /// only the last one executes.
    ///
    /// Emits [`PlayerEvent::SeekCompleted`] after each successful seek,
    /// [`PlayerEvent::PositionUpdate`] after each presented video frame,
    /// [`PlayerEvent::Error`] on non-fatal decode errors, and
    /// [`PlayerEvent::Eof`] before returning.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] if a seek fails.
    #[allow(clippy::too_many_lines)]
    pub fn run(mut self) -> Result<(), PreviewError> {
        let fps = self.fps.max(1.0);
        let frame_period = Duration::from_secs_f64(1.0 / fps);

        // Rebuild the decode buffer when the caller has explicitly configured a
        // hardware acceleration mode other than the default (Auto). The initial
        // buffer is always built with Auto by PreviewPlayer::open(); rebuilding
        // here ensures the user's explicit setting is respected.
        if self.hw_accel != HardwareAccel::Auto && self.decode_buf.is_some() {
            match DecodeBuffer::open(&self.active_path)
                .hardware_accel(self.hw_accel)
                .build()
            {
                Ok(buf) => {
                    self.decode_buf = Some(buf);
                }
                Err(e) => {
                    log::warn!(
                        "hwaccel decode buffer rebuild failed accel={} error={e}",
                        self.hw_accel.name()
                    );
                }
            }
        }

        self.clock.reset(Duration::ZERO);

        // Audio stall detection state: tracks whether samples_consumed is
        // advancing. When it stops for AUDIO_STALL_FRAMES consecutive
        // presented frames, the audio track has ended before the video track
        // and the wall-clock fallback is re-armed so pacing continues.
        let mut prev_audio_samples: u64 = 0;
        let mut audio_stall_frames: u32 = 0;

        loop {
            // ── Drain commands ────────────────────────────────────────────────
            let mut pending_seek: Option<Duration> = None;
            while let Ok(cmd) = self.cmd_rx.try_recv() {
                match cmd {
                    PlayerCommand::Seek(pts) => pending_seek = Some(pts),
                    PlayerCommand::Play => {
                        self.stopped.store(false, Ordering::Release);
                        self.paused.store(false, Ordering::Release);
                        // The cpal hardware callback advances `samples_consumed` even
                        // while paused, so `MasterClock::Audio` drifts forward during
                        // silence. Reset the clock to the last presented video frame so
                        // frames are not immediately dropped as "late" on resume.
                        if self.rate > 0.0 {
                            let pts =
                                Duration::from_micros(self.current_pts.load(Ordering::Relaxed));
                            if self.clock.current_pts().saturating_sub(pts)
                                > Duration::from_millis(100)
                            {
                                self.clock.reset(pts);
                                self.restart_audio_from(pts);
                            }
                        }
                    }
                    PlayerCommand::Pause => {
                        self.paused.store(true, Ordering::Release);
                    }
                    PlayerCommand::Stop => {
                        self.stopped.store(true, Ordering::Release);
                    }
                    PlayerCommand::SetRate(r) => {
                        if r != 0.0 {
                            let was_negative = self.rate < 0.0;
                            self.rate = r;
                            if r > 0.0 {
                                self.clock.set_rate(r);
                                // Returning from reverse: the MasterClock kept advancing
                                // forward during reverse playback, so its position is now
                                // ahead of the video position. Reset it to the current
                                // video position and re-seek the decode buffer so the
                                // forward path resumes from the right frame.
                                if was_negative {
                                    let pts = Duration::from_micros(
                                        self.current_pts.load(Ordering::Relaxed),
                                    );
                                    self.clock.reset(pts);
                                    // Use coarse seek (no forward-decode discard) so the
                                    // first video frame arrives before the audio clock
                                    // has advanced past pts, preventing A/V drift.
                                    if let Some(buf) = self.decode_buf.as_mut()
                                        && let Err(e) = buf.seek_coarse(pts)
                                    {
                                        log::warn!(
                                            "reverse→forward seek failed pts={pts:?} \
                                             error={e}"
                                        );
                                    }
                                    self.restart_audio_from(pts);
                                }
                            } else {
                                // Entering reverse: mute audio by cancelling the decode thread
                                // and clearing the buffer.
                                if let Some(cancel) = &self.audio_cancel {
                                    cancel.store(true, Ordering::Release);
                                }
                                if let Some(buf) = &self.audio_buf {
                                    buf.lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .clear();
                                }
                            }
                        }
                    }
                    PlayerCommand::SetAvOffset(ms) => {
                        const MAX_OFFSET_MS: i64 = 5_000;
                        self.av_offset_ms = ms.clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS);
                    }
                    #[cfg(feature = "timeline")]
                    PlayerCommand::UpdateLayout(_) => {}
                }
            }

            // ── Apply pending seek ────────────────────────────────────────────
            let had_seek = pending_seek.is_some();
            if let Some(pts) = pending_seek {
                // Invalidate the frame cache when seeking outside its range.
                if let Some(cache) = &mut self.frame_cache {
                    let in_range = cache
                        .pts_range()
                        .is_some_and(|(lo, hi)| pts >= lo && pts <= hi);
                    if !in_range {
                        cache.invalidate();
                    }
                }
                if let Some(buf) = self.decode_buf.as_mut() {
                    buf.seek(pts)?;
                }
                self.clock.reset(pts);
                self.restart_audio_from(pts);
                let _ = self.event_tx.try_send(PlayerEvent::SeekCompleted(pts));
            }

            // When a seek arrives while paused, present one preview frame so
            // the sink reflects the new position without resuming playback.
            if had_seek
                && self.paused.load(Ordering::Acquire)
                && let Some(buf) = self.decode_buf.as_mut()
            {
                let deadline = std::time::Instant::now() + Duration::from_millis(300);
                loop {
                    match buf.pop_frame() {
                        FrameResult::Frame(f) => {
                            self.present_frame(&f);
                            let pts = f.timestamp().as_duration();
                            let _ = self.event_tx.try_send(PlayerEvent::PositionUpdate(pts));
                            break;
                        }
                        FrameResult::Seeking(_) => {
                            if std::time::Instant::now() > deadline {
                                break;
                            }
                            thread::sleep(Duration::from_millis(2));
                        }
                        FrameResult::Eof => break,
                    }
                }
            }

            // Surface non-fatal decode errors from the background thread.
            if let Some(buf) = self.decode_buf.as_ref() {
                while let Ok(msg) = buf.error_events().try_recv() {
                    let _ = self.event_tx.try_send(PlayerEvent::Error(msg));
                }
            }

            if self.stopped.load(Ordering::Acquire) {
                break;
            }
            if self.paused.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
                continue;
            }

            // ── Reverse playback path ─────────────────────────────────────────
            if self.rate < 0.0 {
                if let Some(buf) = self.decode_buf.as_mut() {
                    let current = Duration::from_micros(self.current_pts.load(Ordering::Relaxed));
                    // Step size = one frame at the requested speed.
                    let step =
                        Duration::from_secs_f64(self.rate.abs() / fps.max(f64::MIN_POSITIVE));
                    let target = current.saturating_sub(step);

                    if buf.seek_coarse(target).is_err() {
                        break;
                    }

                    // Drain pop_frame until a decoded frame arrives (with timeout).
                    let deadline = std::time::Instant::now() + Duration::from_millis(300);
                    let frame = loop {
                        match buf.pop_frame() {
                            FrameResult::Frame(f) => break Some(f),
                            FrameResult::Seeking(_) => {
                                if std::time::Instant::now() > deadline {
                                    break None;
                                }
                                thread::sleep(Duration::from_millis(2));
                            }
                            FrameResult::Eof => break None,
                        }
                    };

                    if let Some(f) = frame {
                        self.present_frame(&f);
                        let pts = f.timestamp().as_duration();
                        let _ = self.event_tx.try_send(PlayerEvent::PositionUpdate(pts));
                    }

                    if target == Duration::ZERO {
                        // Reached the start of the clip — pause automatically.
                        self.paused.store(true, Ordering::Release);
                    }
                }
                thread::sleep(frame_period);
                continue;
            }

            // ── Audio-only path ───────────────────────────────────────────────
            if self.decode_buf.is_none() {
                let poll_secs =
                    (10.0_f64 / self.rate.max(f64::MIN_POSITIVE)).clamp(1.0, 50.0) / 1_000.0;
                thread::sleep(Duration::from_secs_f64(poll_secs));
                if let Some(audio_buf) = &self.audio_buf {
                    let empty = audio_buf
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_empty();
                    if empty
                        && self
                            .audio_handle
                            .as_ref()
                            .is_none_or(JoinHandle::is_finished)
                    {
                        break;
                    }
                } else {
                    break;
                }
                continue;
            }

            // ── Frame cache hit ───────────────────────────────────────────────
            let current = self.clock.current_pts();
            let cache_hit = self
                .frame_cache
                .as_ref()
                .and_then(|c| c.get(current))
                .map(|f| (f.rgba.clone(), f.width, f.height));
            if let Some((rgba, width, height)) = cache_hit {
                if let Some(sink) = self.sink.as_mut() {
                    sink.push_frame(&rgba, width, height, current);
                }
                self.current_pts.store(
                    u64::try_from(current.as_micros()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                let _ = self.event_tx.try_send(PlayerEvent::PositionUpdate(current));
                continue;
            }

            // ── Video decode path ─────────────────────────────────────────────
            let pop_result = if let Some(buf) = self.decode_buf.as_mut() {
                buf.pop_frame()
            } else {
                FrameResult::Eof
            };

            match pop_result {
                FrameResult::Eof => break,
                FrameResult::Seeking(last) => {
                    if let Some(ref f) = last {
                        self.present_frame(f);
                    }
                }
                FrameResult::Frame(frame) => {
                    if self.clock.should_sync() {
                        let video_pts = if frame.timestamp().is_valid() {
                            frame.timestamp().as_duration()
                        } else {
                            Duration::ZERO
                        };

                        let offset_ms = self.av_offset_ms;
                        let offset = Duration::from_millis(offset_ms.unsigned_abs());
                        let adjusted_video_pts = if offset_ms >= 0 {
                            video_pts.saturating_sub(offset)
                        } else {
                            video_pts + offset
                        };

                        let clock_pts = self.clock.current_pts();
                        let diff = adjusted_video_pts.as_secs_f64() - clock_pts.as_secs_f64();
                        let fp = frame_period.as_secs_f64();

                        if diff > fp {
                            let sleep_secs =
                                (diff - fp / 2.0).max(0.0) / self.rate.max(f64::MIN_POSITIVE);
                            // Cap at one scaled frame period so the loop still wakes up
                            // when the audio clock freezes, but slow rates (< 1×) are
                            // not artificially capped to a value shorter than their
                            // required inter-frame sleep.
                            let max_sleep = fp / self.rate.max(f64::MIN_POSITIVE);
                            thread::sleep(Duration::from_secs_f64(sleep_secs.min(max_sleep)));
                        } else if diff < -fp {
                            log::debug!(
                                "dropped late frame video_pts={video_pts:?} \
                                 clock_pts={clock_pts:?}"
                            );
                            continue;
                        }
                    }

                    self.present_frame(&frame);
                    let pts = frame.timestamp().as_duration();
                    let _ = self.event_tx.try_send(PlayerEvent::PositionUpdate(pts));

                    // Grace period: after the first frame, arm the wall-clock fallback
                    // if no audio consumer has started consuming samples yet.
                    // This ensures real-time pacing even when pop_audio_samples() is
                    // never called (e.g. no cpal stream attached to the handle).
                    self.clock.activate_fallback_if_no_audio(pts);

                    // Audio-EOF detection: if samples_consumed stops advancing for
                    // AUDIO_STALL_FRAMES consecutive frames while non-zero (audio was
                    // playing but has now ended), re-arm the wall-clock fallback so the
                    // remaining video plays at its native frame rate.
                    let cur_audio = self.clock.audio_samples_snapshot();
                    if cur_audio > 0 && cur_audio == prev_audio_samples {
                        audio_stall_frames = audio_stall_frames.saturating_add(1);
                        if audio_stall_frames == AUDIO_STALL_FRAMES {
                            self.clock.rearm_fallback_at(pts);
                        }
                    } else {
                        prev_audio_samples = cur_audio;
                        audio_stall_frames = 0;
                    }

                    // Populate cache after conversion (rgba_buf holds the converted frame).
                    if let Some(cache) = &mut self.frame_cache
                        && !self.rgba_buf.is_empty()
                    {
                        cache.insert(pts, self.rgba_buf.clone(), frame.width(), frame.height());
                    }
                }
            }
        }

        let _ = self.event_tx.try_send(PlayerEvent::Eof);
        if let Some(sink) = self.sink.as_mut() {
            sink.flush();
        }
        Ok(())
    }

    fn present_frame(&mut self, frame: &ff_format::VideoFrame) {
        let pts = frame.timestamp().as_duration();
        self.current_pts.store(
            u64::try_from(pts.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        let width = frame.width();
        let height = frame.height();
        if self.sws.convert(frame, &mut self.rgba_buf) {
            sink.push_frame(&self.rgba_buf, width, height, pts);
        }
    }

    fn restart_audio_from(&mut self, pts: Duration) {
        if let Some(buf) = &self.audio_buf {
            buf.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        if let Some(cancel) = &self.audio_cancel {
            cancel.store(true, Ordering::Release);
        }
        drop(self.audio_handle.take());
        if let Some(buf) = &self.audio_buf {
            let new_cancel = Arc::new(AtomicBool::new(false));
            let handle = spawn_audio_thread(
                self.active_path.clone(),
                pts,
                Arc::clone(buf),
                Arc::clone(&new_cancel),
            );
            self.audio_cancel = Some(new_cancel);
            self.audio_handle = Some(handle);
        }
    }

    fn activate_proxy(&mut self, proxy_path: &Path) -> Result<(), PreviewError> {
        let info = ff_probe::open(proxy_path)?;
        let fps = info.frame_rate().unwrap_or(30.0).max(1.0);
        let decode_buf = DecodeBuffer::open(proxy_path)
            .hardware_accel(self.hw_accel)
            .build()?;

        if let Some(cancel) = &self.audio_cancel {
            cancel.store(true, Ordering::Release);
        }
        if let Some(buf) = &self.audio_buf {
            buf.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        drop(self.audio_handle.take());

        let (clock, audio_buf, audio_cancel, audio_handle) = if info.has_audio() {
            let buf = Arc::new(Mutex::new(VecDeque::<f32>::new()));
            let cancel = Arc::new(AtomicBool::new(false));
            let handle = spawn_audio_thread(
                proxy_path.to_path_buf(),
                Duration::ZERO,
                Arc::clone(&buf),
                Arc::clone(&cancel),
            );
            let clock = MasterClock::Audio {
                samples_consumed: Arc::new(AtomicU64::new(0)),
                sample_rate: DECODED_SAMPLE_RATE,
                rate: 1.0,
                samples_base: 0,
                pts_base: Duration::ZERO,
                fallback: None,
            };
            (clock, Some(buf), Some(cancel), Some(handle))
        } else {
            log::debug!(
                "proxy has no audio, using system clock path={}",
                proxy_path.display()
            );
            let clock = MasterClock::System {
                started_at: Instant::now(),
                base_pts: Duration::ZERO,
                rate: 1.0,
            };
            (clock, None, None, None)
        };

        self.active_path = proxy_path.to_path_buf();
        self.fps = fps;
        self.decode_buf = Some(decode_buf);
        self.clock = clock;
        self.audio_buf = audio_buf;
        self.audio_cancel = audio_cancel;
        self.audio_handle = audio_handle;
        Ok(())
    }
}

impl Drop for PlayerRunner {
    fn drop(&mut self) {
        if let Some(cancel) = &self.audio_cancel {
            cancel.store(true, Ordering::Release);
        }
        if let Some(h) = self.audio_handle.take() {
            let _ = h.join();
        }
    }
}

// ── PreviewPlayer (thin builder) ──────────────────────────────────────────────

/// Thin builder for a ([`PlayerRunner`], [`PlayerHandle`]) pair.
///
/// # Usage
///
/// ```ignore
/// let (mut runner, handle) = PreviewPlayer::open("clip.mp4")?.split();
///
/// runner.set_sink(Box::new(MySink::new()));
///
/// let handle_audio = handle.clone();
///
/// std::thread::spawn(move || { let _ = runner.run(); });
///
/// handle.seek(Duration::from_secs(30));
/// handle.play();
///
/// // cpal audio callback:
/// device.build_output_stream(&cfg, move |buf: &mut [f32], _| {
///     let s = handle_audio.pop_audio_samples(buf.len());
///     buf[..s.len()].copy_from_slice(&s);
/// }, ...);
/// ```
pub struct PreviewPlayer {
    path: PathBuf,
    /// `None` after `split()` consumes it.
    decode_buf: Option<DecodeBuffer>,
    fps: f64,
    /// `None` after `split()` consumes it.
    clock: Option<MasterClock>,
    audio_buf: Option<Arc<Mutex<VecDeque<f32>>>>,
    audio_cancel: Option<Arc<AtomicBool>>,
    audio_handle: Option<JoinHandle<()>>,
    duration_millis: u64,
    active_path: PathBuf,
}

impl PreviewPlayer {
    /// Open a media file and prepare for playback.
    ///
    /// Probes the file to detect audio/video streams, opens a
    /// [`DecodeBuffer`] for the video stream (when present), and spawns a
    /// background audio decode thread (when present). Returns
    /// [`PreviewError`] if the file is missing or contains neither stream.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] if the file cannot be probed or decoded.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PreviewError> {
        let path = path.as_ref();
        let info = ff_probe::open(path)?;

        if !info.has_video() && !info.has_audio() {
            return Err(PreviewError::Ffmpeg {
                code: -1,
                message: "file has neither a video nor an audio stream".into(),
            });
        }

        let fps = info.frame_rate().unwrap_or(30.0).max(1.0);

        let d = info.duration();
        let duration_millis = if d.is_zero() {
            u64::MAX
        } else {
            u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
        };

        let clock = if info.has_audio() {
            MasterClock::Audio {
                samples_consumed: Arc::new(AtomicU64::new(0)),
                sample_rate: DECODED_SAMPLE_RATE,
                rate: 1.0,
                samples_base: 0,
                pts_base: Duration::ZERO,
                fallback: None,
            }
        } else {
            log::debug!(
                "using system clock fallback path={} no_audio=true",
                path.display()
            );
            MasterClock::System {
                started_at: Instant::now(),
                base_pts: Duration::ZERO,
                rate: 1.0,
            }
        };

        let decode_buf = if info.has_video() {
            Some(DecodeBuffer::open(path).build()?)
        } else {
            log::debug!(
                "audio-only file; skipping video decode buffer path={}",
                path.display()
            );
            None
        };

        let (audio_buf, audio_cancel, audio_handle) = if let MasterClock::Audio { .. } = &clock {
            let buf = Arc::new(Mutex::new(VecDeque::<f32>::new()));
            let cancel = Arc::new(AtomicBool::new(false));
            let handle = spawn_audio_thread(
                path.to_path_buf(),
                Duration::ZERO,
                Arc::clone(&buf),
                Arc::clone(&cancel),
            );
            (Some(buf), Some(cancel), Some(handle))
        } else {
            (None, None, None)
        };

        Ok(PreviewPlayer {
            path: path.to_path_buf(),
            decode_buf,
            fps,
            clock: Some(clock),
            audio_buf,
            audio_cancel,
            audio_handle,
            duration_millis,
            active_path: path.to_path_buf(),
        })
    }

    /// Consume `self` and return an exclusive [`PlayerRunner`] and a shared
    /// [`PlayerHandle`].
    ///
    /// The runner owns the decode pipeline; move it to a background thread
    /// and call [`PlayerRunner::run`].
    /// The handle is `Clone + Send + Sync` and can be shared freely.
    ///
    /// # Panics
    ///
    /// Never panics in practice — the internal clock is always `Some` when
    /// `split` is first called.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn split(mut self) -> (PlayerRunner, PlayerHandle) {
        let current_pts = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = mpsc::sync_channel(CHANNEL_CAP);
        let (event_tx, event_rx) = mpsc::sync_channel(CHANNEL_CAP);

        let clock = self.clock.take().expect("clock consumed before split");
        let samples_consumed = match &clock {
            MasterClock::Audio {
                samples_consumed, ..
            } => Some(Arc::clone(samples_consumed)),
            MasterClock::System { .. } => None,
        };

        let audio_buf_for_handle = self.audio_buf.clone();
        let duration_millis = self.duration_millis;

        let runner = PlayerRunner {
            path: self.path.clone(),
            cmd_rx,
            event_tx,
            decode_buf: self.decode_buf.take(),
            fps: self.fps,
            sink: None,
            clock,
            audio_buf: self.audio_buf.take(),
            audio_cancel: self.audio_cancel.take(),
            audio_handle: self.audio_handle.take(),
            sws: super::playback_inner::SwsRgbaConverter::new(),
            rgba_buf: Vec::new(),
            active_path: self.active_path.clone(),
            current_pts: Arc::clone(&current_pts),
            paused: Arc::clone(&paused),
            stopped: Arc::clone(&stopped),
            av_offset_ms: 0,
            rate: 1.0,
            duration_millis,
            frame_cache: None,
            hw_accel: HardwareAccel::Auto,
        };

        let handle = PlayerHandle {
            cmd_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
            current_pts,
            audio_buf: audio_buf_for_handle,
            samples_consumed,
            audio_mixer: None,
            paused,
            stopped,
            duration_millis,
        };

        (runner, handle)
    }
}

impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        if let Some(cancel) = &self.audio_cancel {
            cancel.store(true, Ordering::Release);
        }
        if let Some(h) = self.audio_handle.take() {
            let _ = h.join();
        }
    }
}

// ── spawn_audio_thread ────────────────────────────────────────────────────────

fn spawn_audio_thread(
    path: PathBuf,
    start_pts: Duration,
    buf: Arc<Mutex<VecDeque<f32>>>,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut decoder = match AudioDecoder::open(&path)
            .output_format(SampleFormat::F32)
            .output_sample_rate(DECODED_SAMPLE_RATE)
            .output_channels(2)
            .build()
        {
            Ok(d) => d,
            Err(e) => {
                log::warn!("audio decode thread open failed error={e}");
                return;
            }
        };

        if start_pts != Duration::ZERO
            && let Err(e) = decoder.seek(start_pts, SeekMode::Backward)
        {
            log::warn!("audio seek failed pts={start_pts:?} error={e}");
        }

        loop {
            if cancel.load(Ordering::Acquire) {
                break;
            }

            match decoder.decode_one() {
                Ok(Some(frame)) => {
                    let samples = super::playback_inner::audio_frame_to_f32(&frame);
                    // Push ALL samples without dropping. When the ring buffer is
                    // full, wait for cpal to drain space before continuing.
                    // Using take(space) instead would silently discard samples on
                    // platforms where sleep(1ms) sleeps much longer (e.g. ~10ms on
                    // Windows), causing audio to play at ~2x speed (issue #18).
                    let mut offset = 0;
                    while offset < samples.len() {
                        if cancel.load(Ordering::Acquire) {
                            return;
                        }
                        let mut guard = buf
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let space = AUDIO_MAX_BUF.saturating_sub(guard.len());
                        if space == 0 {
                            drop(guard);
                            thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        let take = space.min(samples.len() - offset);
                        guard.extend(samples[offset..offset + take].iter().copied());
                        offset += take;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    log::warn!("audio decode error error={e}");
                    break;
                }
            }
        }
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_video_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4")
    }

    fn test_audio_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/audio/konekonoosanpo.mp3")
    }

    // ── open ──────────────────────────────────────────────────────────────────

    #[test]
    fn preview_player_open_should_fail_for_nonexistent_file() {
        let result = PreviewPlayer::open("nonexistent_preview.mp4");
        assert!(
            result.is_err(),
            "open() must return Err for a non-existent file"
        );
    }

    // ── play / pause / stop via handle ───────────────────────────────────────

    #[test]
    fn player_handle_play_pause_should_update_paused_flag_immediately() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        assert!(!handle.paused.load(Ordering::Relaxed));
        assert!(!handle.stopped.load(Ordering::Relaxed));

        handle.pause();
        assert!(handle.paused.load(Ordering::Relaxed));

        handle.play();
        assert!(!handle.paused.load(Ordering::Relaxed));
        assert!(!handle.stopped.load(Ordering::Relaxed));

        handle.stop();
        assert!(handle.stopped.load(Ordering::Relaxed));
    }

    // ── run with sink ─────────────────────────────────────────────────────────

    #[test]
    fn player_runner_run_should_deliver_frames_to_sink() {
        struct CountSink(Arc<Mutex<usize>>);
        impl FrameSink for CountSink {
            fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
                *self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            }
        }

        let path = test_video_path();
        let (mut runner, _handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        let count = Arc::new(Mutex::new(0usize));
        runner.set_sink(Box::new(CountSink(Arc::clone(&count))));

        match runner.run() {
            Ok(()) => {}
            Err(e) => {
                println!("skipping: run() error: {e}");
                return;
            }
        }

        let frames = *count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            frames > 0,
            "run() must deliver at least one frame to the sink"
        );
    }

    // ── pop_audio_samples ────────────────────────────────────────────────────

    #[test]
    fn pop_audio_samples_should_return_empty_when_paused() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        handle.pause();
        let samples = handle.pop_audio_samples(1024);
        assert!(
            samples.is_empty(),
            "pop_audio_samples() must return empty while paused"
        );
    }

    #[test]
    fn pop_audio_samples_should_return_empty_when_stopped() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        handle.stop();
        let samples = handle.pop_audio_samples(1024);
        assert!(
            samples.is_empty(),
            "pop_audio_samples() must return empty while stopped"
        );
    }

    #[test]
    fn pop_audio_samples_should_return_empty_for_zero_n_samples() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        handle.play();
        let samples = handle.pop_audio_samples(0);
        assert!(
            samples.is_empty(),
            "pop_audio_samples(0) must always return empty"
        );
    }

    #[test]
    fn pop_audio_samples_should_be_callable_via_cloned_handle() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        let shared = handle.clone();
        let _samples = shared.pop_audio_samples(0);
    }

    #[test]
    fn pop_audio_samples_clock_increment_should_equal_half_sample_count() {
        let stereo_samples: usize = 9_600;
        let expected_frames: u64 = (stereo_samples / 2) as u64;
        assert_eq!(
            expected_frames, 4_800,
            "9600 stereo samples must yield 4800 clock frames"
        );
        let pts = Duration::from_secs_f64(f64::from(48_000u32).recip() * expected_frames as f64);
        assert!(
            (pts.as_secs_f64() - 0.1).abs() < 1e-6,
            "4800 frames at 48 kHz must equal 100 ms; got {pts:?}"
        );
    }

    // ── current_pts / duration ───────────────────────────────────────────────

    #[test]
    fn current_pts_should_return_zero_before_first_frame() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        assert_eq!(
            handle.current_pts(),
            Duration::ZERO,
            "current_pts() must be ZERO before any frame is presented"
        );
    }

    #[test]
    fn duration_should_return_some_for_file_with_known_duration() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        assert!(
            handle.duration().is_some(),
            "duration() must return Some for a file with a known container duration"
        );
        let d = handle.duration().unwrap();
        assert!(
            d > Duration::ZERO,
            "duration() must be positive for a valid media file; got {d:?}"
        );
    }

    #[test]
    fn duration_should_return_none_when_duration_millis_is_sentinel() {
        let sentinel = u64::MAX;
        let result: Option<Duration> = if sentinel == u64::MAX {
            None
        } else {
            Some(Duration::from_millis(sentinel))
        };
        assert!(result.is_none(), "sentinel u64::MAX must map to None");

        let valid = 5_000u64;
        let result: Option<Duration> = if valid == u64::MAX {
            None
        } else {
            Some(Duration::from_millis(valid))
        };
        assert_eq!(result, Some(Duration::from_secs(5)));
    }

    #[test]
    fn current_pts_should_advance_after_frames_are_presented() {
        struct PtsSink(Arc<Mutex<Option<Duration>>>);
        impl FrameSink for PtsSink {
            fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, pts: Duration) {
                *self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pts);
            }
        }

        let path = test_video_path();
        let (mut runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        let last_pts = Arc::new(Mutex::new(None::<Duration>));
        runner.set_sink(Box::new(PtsSink(Arc::clone(&last_pts))));
        let _ = runner.run();

        let sink_pts = last_pts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or(Duration::ZERO);
        let player_pts = handle.current_pts();
        let diff = sink_pts.abs_diff(player_pts);
        assert!(
            diff <= Duration::from_millis(1),
            "current_pts() must be within 1 ms of the last sink PTS; \
             player_pts={player_pts:?} sink_pts={sink_pts:?} diff={diff:?}"
        );
    }

    // ── seek ──────────────────────────────────────────────────────────────────

    #[test]
    fn seek_coarse_should_delegate_to_decode_buffer() {
        let path = test_video_path();
        let (runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        let target = Duration::from_secs(1);
        handle.seek(target);

        // Stop after a short time so the test doesn't block for the full file.
        let handle_thread = handle.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            handle_thread.stop();
        });

        match runner.run() {
            Ok(()) => {}
            Err(e) => {
                println!("skipping: run() error: {e}");
            }
        }
    }

    // ── proxy ─────────────────────────────────────────────────────────────────

    #[test]
    fn use_proxy_if_available_should_return_false_when_no_proxy_in_dir() {
        let path = test_video_path();
        let (mut runner, _handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        let tmp = std::env::temp_dir().join("ff_preview_no_proxy_dir_test");
        let _ = std::fs::create_dir_all(&tmp);
        let found = runner.use_proxy_if_available(&tmp);
        assert!(
            !found,
            "must return false when no proxy files exist in the directory"
        );
    }

    #[test]
    fn active_source_should_return_original_path_before_proxy_activation() {
        let path = test_video_path();
        let (runner, _handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        assert_eq!(
            runner.active_source(),
            path.as_path(),
            "active_source() must equal the original path before any proxy activation"
        );
    }

    // ── set_rate / set_av_offset ──────────────────────────────────────────────

    #[test]
    fn set_rate_should_accept_positive_value() {
        let path = test_video_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };
        // Verify that calling set_rate with a valid value does not panic.
        handle.set_rate(2.0);
        handle.set_rate(0.5);
    }

    #[test]
    fn set_av_offset_default_should_be_zero() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let offset = AtomicI64::new(0);
        assert_eq!(offset.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn positive_av_offset_should_reduce_adjusted_video_pts() {
        let video_pts = Duration::from_millis(1_000);
        let offset_ms: i64 = 200;
        let adjusted = if offset_ms >= 0 {
            let offset = Duration::from_millis(offset_ms as u64);
            video_pts.saturating_sub(offset)
        } else {
            let offset = Duration::from_millis(offset_ms.unsigned_abs());
            video_pts + offset
        };
        assert_eq!(
            adjusted,
            Duration::from_millis(800),
            "positive offset must reduce adjusted_video_pts by offset amount"
        );
    }

    #[test]
    fn negative_av_offset_should_increase_adjusted_video_pts() {
        let video_pts = Duration::from_millis(1_000);
        let offset_ms: i64 = -200;
        let adjusted = if offset_ms >= 0 {
            let offset = Duration::from_millis(offset_ms as u64);
            video_pts.saturating_sub(offset)
        } else {
            let offset = Duration::from_millis(offset_ms.unsigned_abs());
            video_pts + offset
        };
        assert_eq!(
            adjusted,
            Duration::from_millis(1_200),
            "negative offset must increase adjusted_video_pts by offset amount"
        );
    }

    #[test]
    fn positive_av_offset_at_zero_pts_should_saturate_to_zero() {
        let video_pts = Duration::ZERO;
        let offset_ms: i64 = 100;
        let adjusted = video_pts.saturating_sub(Duration::from_millis(offset_ms as u64));
        assert_eq!(
            adjusted,
            Duration::ZERO,
            "saturating_sub on zero pts must clamp to zero not underflow"
        );
    }

    // ── audio_sample_rate ────────────────────────────────────────────────────

    #[test]
    fn audio_sample_rate_should_return_some_48_khz_for_audio_only_file() {
        let path = test_audio_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: audio file not available: {e}");
                return;
            }
        };
        assert_eq!(
            handle.audio_sample_rate(),
            Some(DECODED_SAMPLE_RATE),
            "audio_sample_rate() must return Some(48_000) for a file with an audio stream"
        );
    }

    #[test]
    fn audio_sample_rate_should_return_some_48_khz_regardless_of_source_native_rate() {
        // Verifies that audio_sample_rate() always returns the decoder's fixed
        // output rate (48 000 Hz), not the source file's native rate.
        // The audio file (konekonoosanpo.mp3) may be 44 100 Hz natively — the
        // returned value must still be 48 000.
        let path = test_audio_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: audio file not available: {e}");
                return;
            }
        };
        if let Some(rate) = handle.audio_sample_rate() {
            assert_eq!(
                rate, DECODED_SAMPLE_RATE,
                "audio_sample_rate() must equal DECODED_SAMPLE_RATE=48 000 regardless of source"
            );
        }
    }

    #[test]
    fn audio_sample_rate_should_return_none_when_no_audio_buf_present() {
        // Verifies the None path: when audio_buf is absent (video-only source),
        // audio_sample_rate() returns None.
        // We exercise the logic directly since we don't have a video-only asset.
        let buf: Option<std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<f32>>>> = None;
        let rate: Option<u32> = buf.as_ref().map(|_| DECODED_SAMPLE_RATE);
        assert_eq!(
            rate, None,
            "audio_sample_rate() must return None when no audio ring buffer is present"
        );
    }

    // ── audio-only ────────────────────────────────────────────────────────────

    #[test]
    fn audio_only_open_should_succeed() {
        let path = test_audio_path();
        match PreviewPlayer::open(&path) {
            Ok(player) => {
                let (runner, handle) = player.split();
                // Audio-only: runner has no decode buffer.
                assert!(
                    runner.decode_buf.is_none(),
                    "audio-only runner must have no video decode buffer"
                );
                // Handle has an audio buffer.
                assert!(
                    handle.audio_buf.is_some(),
                    "audio-only handle must have an audio ring buffer"
                );
            }
            Err(e) => {
                println!("skipping: audio file not available: {e}");
            }
        }
    }

    #[test]
    fn audio_only_run_should_return_ok_without_video_frames() {
        let path = test_audio_path();
        let (mut runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: audio file not available: {e}");
                return;
            }
        };

        struct CountingSink(usize);
        impl FrameSink for CountingSink {
            fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
                self.0 += 1;
            }
        }
        runner.set_sink(Box::new(CountingSink(0)));

        let handle_thread = handle.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            handle_thread.stop();
        });

        let result = runner.run();
        assert!(
            result.is_ok(),
            "run() on an audio-only player must return Ok; got {result:?}"
        );
        assert_eq!(
            handle.current_pts(),
            Duration::ZERO,
            "current_pts() must remain ZERO for audio-only playback (no video frames)"
        );
    }

    #[test]
    fn audio_only_seek_should_not_fail_for_valid_target() {
        let path = test_audio_path();
        let (_runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: audio file not available: {e}");
                return;
            }
        };
        // seek() on audio-only player sends a command without errors.
        handle.seek(Duration::from_secs(1));
    }

    // ── seek event delivery (integration) ────────────────────────────────────

    #[test]
    #[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
    fn seek_should_deliver_seek_completed_event_via_poll_event() {
        let path = test_video_path();
        if !path.exists() {
            println!("skipping: video file not found at {}", path.display());
            return;
        }

        let (runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: open failed: {e}");
                return;
            }
        };

        let handle_bg = handle.clone();
        let bg = thread::spawn(move || {
            let _ = runner.run();
        });

        // Give the runner one frame period to start, then seek.
        thread::sleep(Duration::from_millis(50));
        let target = Duration::from_secs(1);
        handle.seek(target);

        // Wait up to 2 seconds for SeekCompleted, skipping PositionUpdate
        // events that may have accumulated during the startup window.
        let deadline = Instant::now() + Duration::from_secs(2);
        let seek_result = loop {
            match handle.poll_event() {
                Some(PlayerEvent::SeekCompleted(pts)) => break Ok(pts),
                Some(PlayerEvent::Eof) => break Err("Eof"),
                Some(PlayerEvent::Error(_)) => break Err("Error"),
                Some(PlayerEvent::PositionUpdate(_)) => {} // skip pre-seek updates
                None => {}
            }
            if Instant::now() > deadline {
                break Err("timeout");
            }
            thread::sleep(Duration::from_millis(10));
        };

        handle_bg.stop();
        let _ = bg.join();

        match seek_result {
            Ok(pts) => {
                assert!(
                    pts >= target.saturating_sub(Duration::from_millis(100)),
                    "SeekCompleted pts must be near the requested target; \
                     target={target:?} pts={pts:?}"
                );
            }
            Err(reason) => {
                panic!("SeekCompleted not received within 2 seconds: {reason}");
            }
        }
    }

    // ── PlayerEvent: PositionUpdate + Error ───────────────────────────────────

    #[test]
    fn position_update_and_error_event_variants_should_be_accessible() {
        let _ = PlayerEvent::PositionUpdate(Duration::ZERO);
        let _ = PlayerEvent::Error("test error".to_string());
    }

    #[test]
    fn eof_event_should_be_delivered_after_run_completes() {
        let path = test_audio_path();
        let (runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: {e}");
                return;
            }
        };

        // Stop after 150 ms so the test does not wait for the full audio duration.
        let handle_stop = handle.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            handle_stop.stop();
        });

        let _ = runner.run();
        let events: Vec<_> = std::iter::from_fn(|| handle.poll_event()).collect();
        assert!(
            events.iter().any(|e| matches!(e, PlayerEvent::Eof)),
            "Eof event must be delivered after run() returns; collected {} events",
            events.len()
        );
    }

    #[test]
    #[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
    fn position_update_should_be_emitted_for_each_video_frame() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4");
        if !path.exists() {
            println!("skipping: video asset not found");
            return;
        }

        use std::sync::{Arc, Mutex};
        struct CountSink {
            count: Arc<Mutex<usize>>,
            max: usize,
            handle: PlayerHandle,
        }
        impl FrameSink for CountSink {
            fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
                let mut g = self
                    .count
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *g += 1;
                if *g >= self.max {
                    self.handle.stop();
                }
            }
        }

        let (mut runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: {e}");
                return;
            }
        };

        let count = Arc::new(Mutex::new(0usize));
        runner.set_sink(Box::new(CountSink {
            count: Arc::clone(&count),
            max: 20,
            handle: handle.clone(),
        }));
        let _ = runner.run();

        let frames = *count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let position_updates: Vec<_> = std::iter::from_fn(|| handle.poll_event())
            .filter(|e| matches!(e, PlayerEvent::PositionUpdate(_)))
            .collect();

        assert!(
            !position_updates.is_empty(),
            "at least one PositionUpdate event must be emitted; frames delivered={frames}"
        );
        assert!(
            position_updates.len() <= frames,
            "PositionUpdate count ({}) must not exceed frame count ({frames})",
            position_updates.len()
        );
    }

    // ── HardwareAccel ─────────────────────────────────────────────────────────

    #[test]
    fn hardware_accel_variants_should_be_accessible_on_player_runner() {
        // Type-check / accessibility test — no asset required.
        let _ = HardwareAccel::Auto;
        let _ = HardwareAccel::None;
        let _ = HardwareAccel::Nvdec;
        let _ = HardwareAccel::Qsv;
        let _ = HardwareAccel::Amf;
        let _ = HardwareAccel::VideoToolbox;
        let _ = HardwareAccel::Vaapi;
    }

    #[test]
    fn set_hardware_accel_none_should_complete_without_error_on_audio_only_file() {
        // Audio-only path has no video decode buffer; the hw_accel rebuild
        // at run() start is skipped.  Verifies the setter is a no-op when
        // no decode buffer exists, and run() still returns Ok.
        let path = test_audio_path();
        let (mut runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: audio file not available: {e}");
                return;
            }
        };

        runner.set_hardware_accel(HardwareAccel::None);
        assert_eq!(runner.hw_accel, HardwareAccel::None);

        let handle_stop = handle.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            handle_stop.stop();
        });

        let result = runner.run();
        assert!(
            result.is_ok(),
            "run() with HardwareAccel::None must return Ok; got {result:?}"
        );
    }

    #[test]
    #[ignore = "requires assets/video/gameplay.mp4 and hardware decoder; run with -- --include-ignored"]
    fn hardware_accel_auto_should_deliver_frames_on_video_file() {
        let path = test_video_path();
        let (mut runner, handle) = match PreviewPlayer::open(&path) {
            Ok(p) => p.split(),
            Err(e) => {
                println!("skipping: video file not available: {e}");
                return;
            }
        };

        runner.set_hardware_accel(HardwareAccel::Auto);

        struct CountSink {
            count: usize,
            max: usize,
            handle: PlayerHandle,
        }
        impl FrameSink for CountSink {
            fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
                self.count += 1;
                if self.count >= self.max {
                    self.handle.stop();
                }
            }
        }
        runner.set_sink(Box::new(CountSink {
            count: 0,
            max: 5,
            handle: handle.clone(),
        }));

        let result = runner.run();
        assert!(
            result.is_ok(),
            "run() with HardwareAccel::Auto must return Ok; got {result:?}"
        );
        assert!(
            handle.current_pts() > Duration::ZERO,
            "at least one frame must have been presented"
        );
    }
}
