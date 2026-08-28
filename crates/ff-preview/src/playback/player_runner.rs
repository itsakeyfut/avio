//! Exclusive owner of the decode pipeline for ff-preview.
//!
//! Move [`PlayerRunner`] to a background thread and call [`PlayerRunner::run`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ff_decode::{AudioDecoder, HardwareAccel, SeekMode};
use ff_format::SampleFormat;

use super::decode_buffer::{DecodeBuffer, FrameResult};
use super::master_clock::MasterClock;
use super::player::{DECODED_SAMPLE_RATE, PlayerCommand};
use super::sink::FrameSink;
use crate::cache::FrameCache;
use crate::error::PreviewError;
use crate::event::PlayerEvent;

// -- Constants -----------------------------------------------------------

const AUDIO_MAX_BUF: usize = 96_000;
const AUDIO_STALL_FRAMES: u32 = 5;

// PlayerRunner

/// Exclusive owner of the decode pipeline. Move to a background thread and
/// call [`run`](Self::run).
///
/// Configure with [`set_sink`](Self::set_sink),
/// [`use_proxy_if_available`](Self::use_proxy_if_available), and
/// [`set_hardware_accel`](Self::set_hardware_accel) **before** calling `run`.
pub struct PlayerRunner {
    pub(crate) path: PathBuf,
    pub(crate) cmd_rx: mpsc::Receiver<PlayerCommand>,
    pub(crate) event_tx: mpsc::SyncSender<PlayerEvent>,
    pub(crate) decode_buf: Option<DecodeBuffer>,
    pub(crate) fps: f64,
    pub(crate) sink: Option<Box<dyn FrameSink>>,
    pub(crate) clock: MasterClock,
    pub(crate) audio_buf: Option<Arc<Mutex<VecDeque<f32>>>>,
    pub(crate) audio_cancel: Option<Arc<AtomicBool>>,
    pub(crate) audio_handle: Option<JoinHandle<()>>,
    pub(crate) sws: super::playback_inner::SwsRgbaConverter,
    pub(crate) rgba_buf: Vec<u8>,
    pub(crate) active_path: PathBuf,
    pub(crate) current_pts: Arc<AtomicU64>,
    pub(crate) paused: Arc<AtomicBool>,
    pub(crate) stopped: Arc<AtomicBool>,
    pub(crate) av_offset_ms: i64,
    pub(crate) rate: f64,
    pub(crate) duration_millis: u64,
    pub(crate) frame_cache: Option<FrameCache>,
    pub(crate) hw_accel: HardwareAccel,
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
            // Drain commands
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

            // Apply pending seek
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

            // Reverse playback path
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

            // Audio-only path
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

            // Frame cache hit
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

            // Video decode path
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

// spawn_audio_thread

pub(crate) fn spawn_audio_thread(
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

#[cfg(test)]
#[path = "player_runner_tests.rs"]
mod tests;
