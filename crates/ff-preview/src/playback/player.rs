//! Actor-model playback types for ff-preview.
//!
//! This module holds [`PreviewPlayer`] (the thin builder) and [`PlayerCommand`].
//! The implementation of the split parts lives in sibling modules:
//! - `player_handle`: [`PlayerHandle`]
//! - `player_runner`: [`PlayerRunner`] + `spawn_audio_thread`

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(feature = "timeline")]
use crate::scene::Scene;
use ff_decode::HardwareAccel;

use super::decode_buffer::DecodeBuffer;
use super::master_clock::MasterClock;
use super::player_handle::PlayerHandle;
use super::player_runner::{PlayerRunner, spawn_audio_thread};

use crate::error::PreviewError;

// -- Constants -----------------------------------------------------------

const CHANNEL_CAP: usize = 64;
/// Fixed output sample rate of the audio decode thread.
///
/// Must match the value used by `spawn_audio_thread` and `MasterClock::Audio`.
pub(crate) const DECODED_SAMPLE_RATE: u32 = 48_000;

// PlayerCommand

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
    /// Handled only by `SceneRunner`; `PlayerRunner` ignores it.
    /// The runner updates its internal `ClipState` / `AudioOnlyTrack` positions
    /// in place and seeks to the last known media PTS so the next frame is
    /// spatially correct after the layout change.
    #[cfg(feature = "timeline")]
    UpdateLayout(Box<Scene>),
}

// PreviewPlayer (thin builder)

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
