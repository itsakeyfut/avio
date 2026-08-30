//! Internal state structs for [`SceneRunner`](super::runner::SceneRunner).
//!
//! These types are `pub(super)` so the runner (a sibling module) can construct
//! and mutate them directly. They carry no behaviour beyond `AudioOnlyTrack`'s
//! thread lifecycle.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Duration;

use ff_filter::{AnimatedValue, LavfiSource, RealtimeLayerDescriptor, XfadeTransition};
use ff_format::{Rational, Timestamp, VideoFrame};

use crate::audio::AudioTrackHandle;
use crate::error::PreviewError;
use crate::playback::SwsRgbaConverter;
use crate::playback::decode_buffer::{DecodeBuffer, FrameResult};

use super::audio_resampling::spawn_audio_track_thread;

// ClipVideoSource

/// The per-clip video source the runner pulls frames from: a decoded media file,
/// or a **held** constant frame for a generated (solid/text) source.
///
/// A generated source renders the *same* pixels for its whole timeline span (a solid
/// fill, or static drawtext), so the pixels are pulled once at open (via `ff-filter`'s
/// `SolidSource` / `TextSource`) and held. But the runner drives clip progression off
/// each frame's own PTS (out-point / due-frame checks in `runner.rs` and
/// `sync_overlays`), so a fixed PTS would stall the timeline (V1) or spin forever
/// (overlays). The held source therefore stamps each returned frame with a synthetic,
/// monotonically advancing PTS (stepped by `1/fps`), and `seek` resets that cursor —
/// exactly the PTS behaviour of a real decoding source. The enum mirrors the subset of
/// the [`DecodeBuffer`] surface the runner uses, so the compositing loop is unchanged.
pub(super) enum ClipVideoSource {
    /// A decoding media file.
    File(DecodeBuffer),
    /// A generated source: the constant pixels (`None` when the generator was
    /// unavailable — e.g. the `color`/`drawtext` filters are missing — so it renders
    /// nothing rather than failing `open`), plus the synthetic-PTS cursor and step.
    Held {
        frame: Option<VideoFrame>,
        next_pts: Duration,
        step: Duration,
    },
}

/// Microsecond time base for the synthetic held-frame PTS (matching the runner's
/// own frame stamping).
fn micros_timestamp(pts: Duration) -> Timestamp {
    Timestamp::from_duration(pts, Rational::new(1, 1_000_000))
}

impl ClipVideoSource {
    /// Builds a held generated source that stamps frames at `1/fps` starting from
    /// `start_pts` (the clip's `in_point`).
    pub(super) fn held(frame: Option<VideoFrame>, start_pts: Duration, fps: f64) -> Self {
        Self::Held {
            frame,
            next_pts: start_pts,
            step: Duration::from_secs_f64(1.0 / fps.max(1.0)),
        }
    }

    /// Next frame. A `Held` source returns its constant pixels stamped with the next
    /// synthetic PTS (so the runner's PTS-driven progression advances), or `Eof` when
    /// it has no frame.
    pub(super) fn pop_frame(&mut self) -> FrameResult {
        match self {
            Self::File(buf) => buf.pop_frame(),
            Self::Held {
                frame: Some(frame),
                next_pts,
                step,
            } => {
                let mut out = frame.clone();
                out.set_timestamp(micros_timestamp(*next_pts));
                *next_pts = next_pts.saturating_add(*step);
                FrameResult::Frame(out)
            }
            Self::Held { frame: None, .. } => FrameResult::Eof,
        }
    }

    /// Seeks a file source; for a held source, resets the synthetic-PTS cursor so the
    /// next frame is stamped at `target_pts` (the runner seeks on clip activation).
    pub(super) fn seek(&mut self, target_pts: Duration) -> Result<(), PreviewError> {
        match self {
            Self::File(buf) => buf.seek(target_pts),
            Self::Held { next_pts, .. } => {
                *next_pts = target_pts;
                Ok(())
            }
        }
    }

    /// Coarse seek for a file source; resets the held cursor like [`seek`](Self::seek).
    pub(super) fn seek_coarse(&mut self, target_pts: Duration) -> Result<(), PreviewError> {
        match self {
            Self::File(buf) => buf.seek_coarse(target_pts),
            Self::Held { next_pts, .. } => {
                *next_pts = target_pts;
                Ok(())
            }
        }
    }

    /// The decode error channel for a file source; `None` for a held source (no
    /// background decode, so no errors).
    pub(super) fn error_events(&self) -> Option<&Receiver<String>> {
        match self {
            Self::File(buf) => Some(buf.error_events()),
            Self::Held { .. } => None,
        }
    }
}

/// Converts a gain in dB to a linear amplitude multiplier for the mixer.
#[allow(clippy::cast_possible_truncation)]
pub(super) fn db_to_linear(db: f64) -> f32 {
    10.0_f64.powf(db / 20.0) as f32
}

// ClipState

pub(super) struct ClipState {
    /// The clip's video source (file or generated). Used to spawn the audio thread
    /// on clip transition (only a `File` source has audio) and to detect a source
    /// change on a positional re-layout.
    pub(super) source: super::types::SceneSource,
    /// The clip's video source: a decoding file, or a generated held frame.
    pub(super) decode_buf: ClipVideoSource,
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
    pub(super) xfade_dur: Duration,
    /// The `xfade` transition kind for that crossfade (`None` = default `Fade`).
    pub(super) xfade_kind: Option<XfadeTransition>,
    /// Audio track handle — `None` if the clip has no audio stream.
    pub(super) audio_track: Option<AudioTrackHandle>,
    /// Playback speed multiplier from `Clip::speed` (`1.0` = normal).
    /// Used to remap source-file PTS → timeline PTS in `run()`.
    pub(super) speed: f64,
    /// Per-clip opacity for overlay compositing (`1.0` = fully opaque).
    /// V1 base opacity is pre-multiplied host-side; overlay opacity is applied by
    /// the composer via the layer descriptor.
    pub(super) opacity: f32,
    /// Dimension-free compositing description (per-clip effect chain, blend mode,
    /// opacity/position tracks). The runner realises it into a `RealtimeLayer`
    /// each frame via [`RealtimeLayer::with_dimensions`](ff_filter::RealtimeLayer::with_dimensions).
    pub(super) layer_desc: RealtimeLayerDescriptor,
    /// Audio gain (dB), static or automated (the 3-way-merged value). Applied to the
    /// primary-track mixer gain: a static value once at open, an animated one per-tick.
    pub(super) volume: AnimatedValue<f64>,
    /// Per-clip audio fade-in / fade-out for this clip's own audio (`Duration::ZERO`
    /// = none). Forwarded to the audio decode thread's fade envelope.
    pub(super) fade_in: Duration,
    pub(super) fade_out: Duration,
    /// Per-clip audio pitch shift in semitones (`0.0` = none). Forwarded to the
    /// audio decode thread.
    pub(super) pitch: f64,
}

// TransitionState

pub(super) struct TransitionState {
    /// Index of the incoming clip (the one being faded in).
    pub(super) next_idx: usize,
    /// Timeline PTS at which the transition begins.
    pub(super) start: Duration,
    /// Duration of the transition.
    pub(super) duration: Duration,
    /// The `xfade` kind to render for this transition.
    pub(super) kind: XfadeTransition,
}

// OverlayLayer

/// One secondary video layer (V2, V3, …) inside [`SceneRunner`](super::runner::SceneRunner).
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

// LavfiOverlayState

/// A timeline-global generated `lavfi` overlay, composited as the topmost layer.
///
/// The [`LavfiSource`] produces frames sequentially (no seek); the runner advances
/// it with the same held-frame model as [`OverlayLayer`] and rebuilds it on seek.
pub(super) struct LavfiOverlayState {
    /// The lavfi filtergraph string, kept for rebuild-on-seek.
    lavfi: String,
    /// The frame generator (`movie=…:format_name=lavfi → format=rgba`).
    source: LavfiSource,
    /// Reuses one buffer across ticks (like [`OverlayLayer::sws`]).
    sws: SwsRgbaConverter,
    /// Current held rgba frame data.
    pub(super) rgba: Vec<u8>,
    /// Dimensions of the current held frame, or `None` when none is held yet.
    pub(super) dims: Option<(u32, u32)>,
    /// A frame popped ahead of its presentation time (held until due).
    pub(super) pending: Option<VideoFrame>,
}

impl LavfiOverlayState {
    /// Builds the generator from a lavfi string, or logs and returns `None` when the
    /// source cannot be built (e.g. the `movie` / `lavfi` demuxer is unavailable) —
    /// the overlay is then simply dropped from the preview rather than failing `open`.
    pub(super) fn new(lavfi: &str) -> Option<Self> {
        match LavfiSource::new(lavfi) {
            Ok(source) => Some(Self {
                lavfi: lavfi.to_string(),
                source,
                sws: SwsRgbaConverter::new(),
                rgba: Vec::new(),
                dims: None,
                pending: None,
            }),
            Err(e) => {
                log::warn!("lavfi overlay unavailable, dropped from preview: {e}");
                None
            }
        }
    }

    /// Advances the generator to the frame due at `target_pts`, holding the current
    /// frame otherwise (the same held-frame model as [`OverlayLayer`]). The source
    /// PTS runs monotonically from 0 alongside the timeline. Returns the current
    /// frame's `(width, height)`, or `None` when no frame is held yet.
    ///
    /// Only the newest due frame is converted (not every intermediate one during a
    /// far-seek catch-up), reusing the `rgba` buffer like the file-overlay path.
    pub(super) fn advance_to(&mut self, target_pts: Duration) -> Option<(u32, u32)> {
        let mut latest: Option<VideoFrame> = None;
        loop {
            let f = match self.pending.take() {
                Some(pf) => pf,
                None => match self.source.pull() {
                    Ok(Some(f)) => f,
                    _ => break,
                },
            };
            if f.timestamp().as_duration() > target_pts {
                // Not due yet — hold it for a later present.
                self.pending = Some(f);
                break;
            }
            latest = Some(f);
        }
        if let Some(f) = latest
            && self.sws.convert(&f, &mut self.rgba)
        {
            self.dims = Some((f.width(), f.height()));
        }
        self.dims
    }

    /// Rebuilds the source from t=0 (the source has no seek) and clears held state.
    /// On failure the overlay keeps its previous source and is logged.
    pub(super) fn rebuild(&mut self) {
        match LavfiSource::new(&self.lavfi) {
            Ok(source) => {
                self.source = source;
                self.pending = None;
                self.rgba.clear();
                self.dims = None;
            }
            Err(e) => log::warn!("lavfi overlay seek rebuild failed: {e}"),
        }
    }
}

// AudioFadeConfig

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
    /// Per-clip pitch shift in semitones (`0.0` = none). Applied duration-preserving
    /// by the audio thread, before the speed resample.
    pub(super) pitch: f64,
}

impl AudioFadeConfig {
    pub(super) const NONE: Self = Self {
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
        clip_dur: Duration::ZERO,
        in_point: Duration::ZERO,
        speed: 1.0,
        pitch: 0.0,
    };
}

// AudioOnlyTrack

/// One dedicated audio-only clip (from an A1/A2/… track) inside
/// [`SceneRunner`](super::runner::SceneRunner).
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
    /// Playback speed multiplier (`1.0` = normal), applied by resampling.
    pub(super) speed: f64,
    /// Per-clip pitch shift in semitones (`0.0` = none), forwarded to the thread.
    pub(super) pitch: f64,
    pub(super) handle: AudioTrackHandle,
    /// Audio gain (dB), static or automated (the 3-way-merged value). Applied to
    /// `handle`: a static value once at open, an animated one per-tick.
    pub(super) volume: AnimatedValue<f64>,
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
                speed: self.speed,
                pitch: self.pitch,
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ff_format::{Rational, Timestamp};

    #[test]
    fn db_to_linear_should_convert_gain() {
        // The shared dB→linear conversion used by every audio-gain site.
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6, "0 dB = unity");
        assert!(
            (db_to_linear(-6.0) - 0.501_187).abs() < 1e-3,
            "-6 dB ≈ 0.501"
        );
        assert!((db_to_linear(6.0) - 1.995).abs() < 1e-3, "+6 dB ≈ 1.995");
        assert!(db_to_linear(-120.0) < 1e-5, "very quiet ≈ 0");
    }

    #[test]
    fn held_source_should_return_injected_frame_with_advancing_pts() {
        // #1615 (RK-013 injection): the held-frame routing is deterministic without
        // FFmpeg. The pixels are constant, but the PTS must advance by 1/fps each pull
        // (the runner drives clip progression off the frame PTS — a fixed PTS would
        // stall V1 / spin overlays forever). `seek` resets the cursor.
        let frame = VideoFrame::from_rgba(2, 3, vec![10u8; 2 * 3 * 4]).unwrap();
        let mut src = ClipVideoSource::held(Some(frame), Duration::ZERO, 30.0);

        let step = Duration::from_secs_f64(1.0 / 30.0);
        for i in 0..3u32 {
            let FrameResult::Frame(f) = src.pop_frame() else {
                panic!("a held source must return its constant frame every pull");
            };
            assert_eq!((f.width(), f.height()), (2, 3), "pixels are constant");
            let want = step * i;
            let got = f.timestamp().as_duration();
            assert!(
                got.abs_diff(want) < Duration::from_micros(1),
                "held PTS must advance by 1/fps: pull {i} want {want:?} got {got:?}"
            );
        }

        // Seek resets the cursor: the next frame is stamped at the seek target.
        assert!(src.seek(Duration::from_secs(5)).is_ok());
        let FrameResult::Frame(f) = src.pop_frame() else {
            panic!("expected a frame after seek");
        };
        assert!(
            f.timestamp().as_duration().abs_diff(Duration::from_secs(5)) < Duration::from_micros(1)
        );
        assert!(src.seek_coarse(Duration::from_secs(1)).is_ok());
        assert!(
            src.error_events().is_none(),
            "a held source has no decode error channel"
        );

        // A held source with no frame (generator unavailable) is end-of-stream.
        let mut empty = ClipVideoSource::held(None, Duration::ZERO, 30.0);
        assert!(matches!(empty.pop_frame(), FrameResult::Eof));
    }

    #[test]
    fn lavfi_advance_to_should_surface_due_frames_and_hold_future_ones() {
        // Probe-gated: needs the `movie` filter to build the source (absent on CI's
        // Linux FFmpeg). The lavfi demuxer is also absent here, so `source.pull()`
        // yields nothing and the seeded `pending` frames drive the held-frame logic
        // deterministically.
        let Some(mut st) = LavfiOverlayState::new("color=c=red:s=8x8:d=1") else {
            println!("Skipping: movie/lavfi filter unavailable");
            return;
        };
        let stamped = |w: u32, h: u32, secs: u64| {
            let mut f = VideoFrame::from_rgba(w, h, vec![200u8; (w * h * 4) as usize]).unwrap();
            f.set_timestamp(Timestamp::from_duration(
                Duration::from_secs(secs),
                Rational::new(1, 1_000_000),
            ));
            f
        };
        // A frame due at t=0 is surfaced (dims set, buffer converted).
        st.pending = Some(stamped(8, 8, 0));
        assert_eq!(st.advance_to(Duration::ZERO), Some((8, 8)));
        assert!(!st.rgba.is_empty(), "the due frame was converted into rgba");
        // A far-future frame is held, not surfaced; dims stay the last shown 8x8.
        st.pending = Some(stamped(4, 4, 10));
        assert_eq!(st.advance_to(Duration::from_secs(1)), Some((8, 8)));
        assert!(st.pending.is_some(), "the future frame is held for later");
    }
}
