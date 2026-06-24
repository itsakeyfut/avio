//! Internal state structs for [`TimelineRunner`](super::runner::TimelineRunner).
//!
//! These types are `pub(super)` so the runner (a sibling module) can construct
//! and mutate them directly. They carry no behaviour beyond `AudioOnlyTrack`'s
//! thread lifecycle.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use ff_format::VideoFrame;
use ff_pipeline::Clip;

use crate::audio::AudioTrackHandle;
use crate::playback::SwsRgbaConverter;
use crate::playback::decode_buffer::DecodeBuffer;

use super::audio_resampling::spawn_audio_track_thread;

// ── ClipState ─────────────────────────────────────────────────────────────────

pub(super) struct ClipState {
    /// Source file path — needed to spawn audio threads on clip transition.
    pub(super) source: PathBuf,
    pub(super) decode_buf: DecodeBuffer,
    /// Global timeline position where this clip starts.
    pub(super) timeline_start: Duration,
    /// Global timeline position where this clip ends.
    pub(super) timeline_end: Duration,
    /// Source-file PTS at which this clip starts (= `Clip::in_point`).
    pub(super) in_point: Duration,
    /// Source-file PTS at which this clip ends (`None` = play to EOF).
    pub(super) out_point: Option<Duration>,
    /// Duration of the crossfade from the previous clip into this one.
    /// `Duration::ZERO` = hard cut.
    pub(super) transition_dur: Duration,
    /// Audio track handle — `None` if the clip has no audio stream.
    pub(super) audio_track: Option<AudioTrackHandle>,
    /// Playback speed multiplier from `Clip::speed` (`1.0` = normal).
    /// Used to remap source-file PTS → timeline PTS in `run()`.
    pub(super) speed: f64,
    /// Per-clip opacity for overlay compositing (`1.0` = fully opaque).
    /// V1 base opacity is pre-multiplied host-side; overlay opacity is applied by
    /// the composer via [`Clip::realtime_layer`].
    pub(super) opacity: f32,
    /// The source clip — provides the per-clip effect chain and blend mode for
    /// the real-time compositor via [`Clip::realtime_layer`].
    pub(super) clip: Clip,
}

// ── TransitionState ───────────────────────────────────────────────────────────

pub(super) struct TransitionState {
    /// Index of the incoming clip (the one being faded in).
    pub(super) next_idx: usize,
    /// Timeline PTS at which the transition begins.
    pub(super) start: Duration,
    /// Duration of the transition.
    pub(super) duration: Duration,
}

// ── OverlayLayer ──────────────────────────────────────────────────────────────

/// One secondary video layer (V2, V3, …) inside [`TimelineRunner`](super::runner::TimelineRunner).
pub(super) struct OverlayLayer {
    pub(super) clips: Vec<ClipState>,
    /// Index of the clip currently being decoded from this layer.
    pub(super) active: usize,
    pub(super) sws: SwsRgbaConverter,
    pub(super) rgba: Vec<u8>,
    /// Dimensions of the frame currently held in `rgba`, or `None` when nothing is
    /// being shown. Lets the layer hold its current frame across presents (so a
    /// low-fps overlay is not advanced once per present, which would speed it up).
    pub(super) cur_dims: Option<(u32, u32)>,
    /// A frame popped ahead of its presentation time, held until `timeline_pts`
    /// reaches it. Decouples decode order from the present rate.
    pub(super) pending: Option<VideoFrame>,
}

// ── AudioFadeConfig ───────────────────────────────────────────────────────────

/// Fade-in / fade-out parameters forwarded to
/// [`spawn_audio_track_thread`](super::audio_resampling::spawn_audio_track_thread).
#[derive(Clone, Copy)]
pub(super) struct AudioFadeConfig {
    pub(super) fade_in: Duration,
    pub(super) fade_out: Duration,
    /// Effective clip duration — used to position the fade-out start offset.
    pub(super) clip_dur: Duration,
    /// Source-file PTS at which the clip starts (used to offset the envelope on seek).
    pub(super) in_point: Duration,
    /// Playback speed multiplier (`1.0` = normal). When != 1.0, decoded samples are
    /// linearly resampled to compress/expand playback time without pitch preservation.
    pub(super) speed: f64,
}

impl AudioFadeConfig {
    pub(super) const NONE: Self = Self {
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
        clip_dur: Duration::ZERO,
        in_point: Duration::ZERO,
        speed: 1.0,
    };
}

// ── AudioOnlyTrack ────────────────────────────────────────────────────────────

/// One dedicated audio-only clip (from an A1/A2/… track) inside
/// [`TimelineRunner`](super::runner::TimelineRunner).
pub(super) struct AudioOnlyTrack {
    pub(super) source: PathBuf,
    pub(super) timeline_start: Duration,
    pub(super) timeline_end: Duration,
    pub(super) in_point: Duration,
    /// Per-clip audio fade-in duration (`Duration::ZERO` = no fade).
    pub(super) fade_in: Duration,
    /// Per-clip audio fade-out duration (`Duration::ZERO` = no fade).
    pub(super) fade_out: Duration,
    /// Effective clip duration — used to position the fade-out ramp.
    pub(super) clip_dur: Duration,
    pub(super) handle: AudioTrackHandle,
    pub(super) cancel: Option<Arc<AtomicBool>>,
    pub(super) thread: Option<JoinHandle<()>>,
}

impl AudioOnlyTrack {
    pub(super) fn start_at(&mut self, from_pts: Duration) {
        // Cancel any running thread first.
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Release);
        }
        drop(self.thread.take());
        self.handle.clear();
        let cancel = Arc::new(AtomicBool::new(false));
        let t = spawn_audio_track_thread(
            self.source.clone(),
            from_pts,
            self.handle.clone(),
            Arc::clone(&cancel),
            AudioFadeConfig {
                fade_in: self.fade_in,
                fade_out: self.fade_out,
                clip_dur: self.clip_dur,
                in_point: self.in_point,
                speed: 1.0,
            },
        );
        self.cancel = Some(cancel);
        self.thread = Some(t);
    }

    pub(super) fn stop(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Release);
        }
        drop(self.thread.take());
    }
}

impl Drop for AudioOnlyTrack {
    fn drop(&mut self) {
        self.stop();
    }
}
