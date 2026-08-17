//! The timeline decode/present state machine.
//!
//! [`SceneRunner`] owns the per-track decode buffers and the audio mixer,
//! and drives frame presentation. Construct it via
//! [`ScenePlayer::open`](super::ScenePlayer::open).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ff_filter::{
    AnimatedValue, BlendMode, CompositeOp, RealtimeComposer, RealtimeLayer, XfadeTransition,
};
use ff_format::{PixelFormat, Rational, Timestamp, VideoFrame};

use crate::audio::AudioMixer;
use crate::error::PreviewError;
use crate::event::PlayerEvent;
use crate::playback::SwsRgbaConverter;
use crate::playback::decode_buffer::FrameResult;
use crate::playback::master_clock::MasterClock;
use crate::playback::player::PlayerCommand;
use crate::playback::sink::FrameSink;

use super::audio_resampling::spawn_audio_track_thread;
use super::inner;
use super::state::{
    AudioFadeConfig, AudioOnlyTrack, ClipState, LavfiOverlayState, OverlayLayer, TransitionState,
    db_to_linear,
};

// ── SceneRunner ────────────────────────────────────────────────────────────

/// Exclusive owner of the timeline decode pipeline.
///
/// Move to a background thread and call [`run`](Self::run). Register a
/// [`FrameSink`] with [`set_sink`](Self::set_sink) before calling `run`.
pub struct SceneRunner {
    pub(super) clips: Vec<ClipState>,
    /// Secondary video overlay layers (V2, V3, …). Each is composited over V1
    /// in order before the frame is delivered to the sink.
    pub(super) overlay_layers: Vec<OverlayLayer>,
    /// Dedicated audio-only clips (from A1, A2, … tracks). Each is started and
    /// stopped as the playhead crosses its timeline window.
    pub(super) audio_only_tracks: Vec<AudioOnlyTrack>,
    /// Index of the clip currently being decoded and presented.
    pub(super) active: usize,
    /// Non-`None` while a crossfade transition is in progress.
    pub(super) transition: Option<TransitionState>,
    pub(super) cmd_rx: mpsc::Receiver<PlayerCommand>,
    pub(super) event_tx: mpsc::SyncSender<PlayerEvent>,
    pub(super) sink: Option<Box<dyn FrameSink>>,
    pub(super) current_pts: Arc<AtomicU64>,
    pub(super) paused: Arc<AtomicBool>,
    pub(super) stopped: Arc<AtomicBool>,
    pub(super) fps: f64,
    pub(super) rate: f64,
    pub(super) clock: MasterClock,
    /// Media PTS to re-anchor the System clock to when `PlayerCommand::Play`
    /// is received from a paused state. Updated on every seek and after every
    /// presented frame so that accumulated wall-clock time during pause does
    /// not advance `current_pts()` past the last known media position.
    pub(super) resume_pts: Duration,
    /// Pixel-format converter for the active (outgoing) frame.
    pub(super) sws_a: SwsRgbaConverter,
    /// Pixel-format converter for the incoming frame during transitions.
    pub(super) sws_b: SwsRgbaConverter,
    pub(super) rgba_a: Vec<u8>,
    pub(super) rgba_b: Vec<u8>,
    pub(super) blend_buf: Vec<u8>,
    /// Width of the most recently presented primary-track frame; used to
    /// synthesise fill frames during primary-track gaps.
    pub(super) last_frame_w: u32,
    /// Height of the most recently presented primary-track frame.
    pub(super) last_frame_h: u32,
    /// Scratch buffer for synthesising black fill frames during primary-track gaps.
    pub(super) gap_buf: Vec<u8>,
    /// Multi-track audio mixer — `None` when no clip has audio.
    pub(super) audio_mixer: Option<Arc<Mutex<AudioMixer>>>,
    /// Cancel flag for the currently running audio decode thread.
    pub(super) active_audio_cancel: Option<Arc<AtomicBool>>,
    /// Handle to the currently running audio decode thread.
    pub(super) active_audio_thread: Option<JoinHandle<()>>,
    /// Cached real-time compositor that applies per-clip effects + blend modes
    /// (the same chain as export). Rebuilt only when the active clip set or frame
    /// geometry changes; `None` until the first composite.
    pub(super) composer: Option<RealtimeComposer>,
    /// Identifies the composer's current configuration as
    /// `(layer_id, active_clip_idx, width, height)` per layer. Rebuild on change.
    pub(super) composer_key: Vec<(usize, usize, u32, u32)>,
    /// Project output canvas, when the timeline set one explicitly. When `Some`,
    /// every composited frame is letterboxed to these dimensions so the preview
    /// matches the project's output aspect. `None` composites at the base clip's
    /// own size (legacy behaviour).
    pub(super) canvas: Option<(u32, u32)>,
    /// Timeline-global generated `lavfi` overlay, composited as the topmost layer
    /// (above every file overlay). `None` when the timeline set no `lavfi_overlay`.
    pub(super) lavfi: Option<LavfiOverlayState>,
}

impl SceneRunner {
    /// Register the frame sink. Call before [`run`](Self::run).
    pub fn set_sink(&mut self, sink: Box<dyn FrameSink>) {
        self.sink = Some(sink);
    }

    /// Advances every overlay layer to the frame whose presentation time has
    /// arrived at `target_pts`, holding the current frame otherwise (so a layer
    /// whose fps differs from the timeline plays at the right speed rather than
    /// advancing once per present). Returns `(layer_index, width, height)` for
    /// each layer that currently has a frame to show.
    fn sync_overlays(&mut self, target_pts: Duration) -> Vec<(usize, u32, u32)> {
        let mut active = Vec::new();
        for (li, layer) in self.overlay_layers.iter_mut().enumerate() {
            let maybe_cidx = layer
                .clips
                .iter()
                .position(|c| target_pts >= c.timeline_start && target_pts < c.timeline_end);
            let Some(cidx) = maybe_cidx else {
                layer.rgba.clear();
                layer.cur_dims = None;
                layer.pending = None;
                continue;
            };
            if cidx != layer.active {
                let local = layer.clips[cidx].in_point
                    + target_pts.saturating_sub(layer.clips[cidx].timeline_start);
                let _ = layer.clips[cidx].decode_buf.seek(local);
                layer.active = cidx;
                layer.cur_dims = None;
                layer.pending = None;
            }
            let clip_in = layer.clips[cidx].in_point;
            let tl_start = layer.clips[cidx].timeline_start;
            loop {
                let f = match layer.pending.take() {
                    Some(pf) => pf,
                    None => match layer.clips[cidx].decode_buf.pop_frame() {
                        FrameResult::Frame(f) => f,
                        _ => break,
                    },
                };
                let v2_pts = tl_start + f.timestamp().as_duration().saturating_sub(clip_in);
                if v2_pts > target_pts {
                    // Not due yet — hold it for a later present.
                    layer.pending = Some(f);
                    break;
                }
                if layer.sws.convert(&f, &mut layer.rgba) {
                    layer.cur_dims = Some((f.width(), f.height()));
                }
            }
            match layer.cur_dims {
                Some((ow, oh)) => active.push((li, ow, oh)),
                None => layer.rgba.clear(),
            }
        }
        active
    }

    /// Advances the generated `lavfi` overlay (if any) to the frame due at
    /// `target_pts`, holding otherwise — see [`LavfiOverlayState::advance_to`].
    /// Returns the current frame's `(width, height)`, or `None` when there is no
    /// lavfi overlay / no frame yet.
    fn sync_lavfi(&mut self, target_pts: Duration) -> Option<(u32, u32)> {
        self.lavfi.as_mut()?.advance_to(target_pts)
    }

    /// Composites `base_frame` (the bottom layer) with the given overlay layers
    /// through the cached [`RealtimeComposer`], applying each layer's effects and
    /// blend mode. `base_id` identifies the base for cache invalidation — the V1
    /// clip index, or `usize::MAX` for the gap-fill black base. Returns the
    /// composited RGBA frame together with its actual `(width, height)`, or `None`
    /// on failure.
    ///
    /// The composited size can differ from `base_w`/`base_h` when the base layer's
    /// effect chain resizes the frame (`Crop`, `Scale`, `Pad`, `FitToAspect`), so
    /// callers must push the returned dimensions to the sink rather than the
    /// decoded ones — otherwise the buffer length no longer matches the reported
    /// size and the frame is dropped.
    #[allow(clippy::too_many_arguments)]
    fn composite_frame(
        &mut self,
        base_layer: RealtimeLayer,
        base_id: usize,
        mut base_frame: VideoFrame,
        base_w: u32,
        base_h: u32,
        overlays: &[(usize, u32, u32)],
        t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let mut specs = vec![base_layer];
        let mut key: Vec<(usize, usize, u32, u32)> = vec![(0, base_id, base_w, base_h)];
        for &(li, ow, oh) in overlays {
            let oc = &self.overlay_layers[li];
            specs.push(RealtimeLayer::with_dimensions(
                oc.clips[oc.active].layer_desc.clone(),
                ow,
                oh,
                PixelFormat::Rgba,
            ));
            key.push((li + 1, oc.active, ow, oh));
        }
        // Topmost timeline-global lavfi overlay (generated), when a frame is held.
        // Fixed full-frame Normal/Over layer, matching export; its transparency comes
        // from the lavfi content's own alpha. A sentinel layer id keys the cache.
        let lavfi_dims = self.lavfi.as_ref().and_then(|s| s.dims);
        if let Some((lw, lh)) = lavfi_dims {
            specs.push(RealtimeLayer {
                width: lw,
                height: lh,
                pixel_format: PixelFormat::Rgba,
                effects: Vec::new(),
                opacity: AnimatedValue::Static(1.0),
                x: AnimatedValue::Static(0.0),
                y: AnimatedValue::Static(0.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
            });
            key.push((usize::MAX - 1, 0, lw, lh));
        }
        if self.composer.is_none() || self.composer_key != key {
            self.composer = RealtimeComposer::with_canvas(&specs, self.canvas).ok();
            self.composer_key = if self.composer.is_some() {
                key
            } else {
                Vec::new()
            };
        }
        let composer = self.composer.as_mut()?;
        // Stamp every pushed frame with the composite's timeline PTS so the graph's
        // per-frame animation tick (in `push_video`) evaluates each layer's opacity
        // track at the same time. Frames from `from_rgba` carry PTS 0 otherwise, and
        // any registered `AnimationEntry` would be frozen at t=0.
        let ts = Timestamp::from_duration(t, Rational::new(1, 1_000_000));
        base_frame.set_timestamp(ts);
        if composer.push_layer(0, &base_frame).is_err() {
            return None;
        }
        for (slot, &(li, ow, oh)) in overlays.iter().enumerate() {
            let mut vf =
                VideoFrame::from_rgba(ow, oh, self.overlay_layers[li].rgba.clone()).ok()?;
            vf.set_timestamp(ts);
            if composer.push_layer(slot + 1, &vf).is_err() {
                return None;
            }
        }
        // Push the lavfi overlay last (topmost slot), reading its held rgba from the
        // disjoint `lavfi` field (as the overlay loop reads `overlay_layers`).
        if let Some((lw, lh)) = lavfi_dims {
            let rgba = self
                .lavfi
                .as_ref()
                .map_or_else(Vec::new, |s| s.rgba.clone());
            let mut vf = VideoFrame::from_rgba(lw, lh, rgba).ok()?;
            vf.set_timestamp(ts);
            if composer.push_layer(overlays.len() + 1, &vf).is_err() {
                return None;
            }
        }
        let f = composer.pull().ok().flatten()?;
        let (w, h) = (f.width(), f.height());
        f.to_rgba().map(|rgba| (rgba, w, h))
    }

    /// A/V sync presentation loop.
    ///
    /// Plays all clips in the primary video track from start to finish (or until
    /// a [`PlayerCommand::Stop`] is received).
    ///
    /// Emits [`PlayerEvent::SeekCompleted`] after each successful seek,
    /// [`PlayerEvent::PositionUpdate`] after each presented video frame,
    /// [`PlayerEvent::Error`] on non-fatal decode errors, and
    /// [`PlayerEvent::Eof`] before returning.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError::SeekOutOfRange`] if a seek command targets a
    /// timestamp that falls outside all clips on the timeline.
    #[allow(clippy::too_many_lines)]
    pub fn run(mut self) -> Result<(), PreviewError> {
        if self.clips.is_empty() {
            let _ = self.event_tx.try_send(PlayerEvent::Eof);
            return Ok(());
        }

        let fps = self.fps.max(1.0);
        let frame_period = Duration::from_secs_f64(1.0 / fps);
        self.clock.reset(Duration::ZERO);

        loop {
            // ── Drain commands ────────────────────────────────────────────────
            let mut pending_seek: Option<Duration> = None;
            while let Ok(cmd) = self.cmd_rx.try_recv() {
                match cmd {
                    PlayerCommand::Seek(pts) => pending_seek = Some(pts),
                    PlayerCommand::Play => {
                        // Always re-anchor the System clock on Play.
                        //
                        // PlayerHandle::play() sets the shared `paused` atomic
                        // to `false` BEFORE enqueueing PlayerCommand::Play, so
                        // paused.load() here always returns false — a guard on
                        // `if paused` would never fire. Re-anchoring
                        // unconditionally is safe: when the player was not
                        // actually paused, resume_pts equals the last presented
                        // frame PTS (or the seek target), which is already the
                        // clock's current base, so clock.reset() is a no-op
                        // in effect.
                        self.clock.reset(self.resume_pts);
                        self.stopped.store(false, Ordering::Release);
                        self.paused.store(false, Ordering::Release);
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
                                if was_negative {
                                    // Returning from reverse: rebase clock and
                                    // restart audio from the current video position.
                                    let pts = Duration::from_micros(
                                        self.current_pts.load(Ordering::Relaxed),
                                    );
                                    self.clock.reset(pts);
                                    self.resume_pts = pts;
                                    if let Err(e) = self.seek_timeline_coarse(pts) {
                                        log::warn!(
                                            "timeline reverse→forward seek failed \
                                             pts={pts:?} error={e}"
                                        );
                                    } else {
                                        let ci = self.active;
                                        let clip_local = self.clips[ci].in_point
                                            + pts.saturating_sub(self.clips[ci].timeline_start);
                                        if let Some(m) = &self.audio_mixer {
                                            m.lock()
                                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                                .invalidate_all();
                                        }
                                        self.restart_audio_at(ci, clip_local);
                                    }
                                }
                            } else {
                                // Entering reverse: silence audio.
                                if let Some(cancel) = &self.active_audio_cancel {
                                    cancel.store(true, Ordering::Release);
                                }
                                if let Some(m) = &self.audio_mixer {
                                    m.lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .invalidate_all();
                                }
                            }
                        }
                    }
                    PlayerCommand::SetAvOffset(_) => {} // audio timing is system-clock driven
                    PlayerCommand::UpdateLayout(scene) => {
                        if let Err(e) = self.update_layout_in_place(&scene, self.resume_pts) {
                            log::warn!("timeline layout update ignored: {e}");
                        }
                    }
                }
            }

            // ── Apply pending seek ────────────────────────────────────────────
            let had_seek = pending_seek.is_some();
            if let Some(target) = pending_seek {
                self.seek_timeline(target)?;
                self.clock.reset(target);
                self.resume_pts = target;
                let _ = self.event_tx.try_send(PlayerEvent::SeekCompleted(target));
            }

            // When a seek arrives while paused, present one preview frame so
            // the sink reflects the new position without resuming playback.
            if had_seek && self.paused.load(Ordering::Acquire) {
                let active = self.active;
                let deadline = std::time::Instant::now() + Duration::from_millis(300);
                loop {
                    match self.clips[active].decode_buf.pop_frame() {
                        FrameResult::Frame(f) => {
                            let f_pts = f.timestamp().as_duration();
                            let elapsed = f_pts.saturating_sub(self.clips[active].in_point);
                            let tl_pts = self.clips[active].timeline_start
                                + if (self.clips[active].speed - 1.0).abs() < 1e-9 {
                                    elapsed
                                } else {
                                    elapsed.div_f64(self.clips[active].speed)
                                };
                            let w = f.width();
                            let h = f.height();
                            if self.sws_a.convert(&f, &mut self.rgba_a)
                                && let Some(sink) = self.sink.as_mut()
                            {
                                sink.push_frame(&self.rgba_a, w, h, tl_pts);
                            }
                            self.current_pts.store(
                                u64::try_from(tl_pts.as_micros()).unwrap_or(u64::MAX),
                                Ordering::Relaxed,
                            );
                            let _ = self.event_tx.try_send(PlayerEvent::PositionUpdate(tl_pts));
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

            // ── Error events from active clip ─────────────────────────────────
            {
                let active = self.active;
                while let Ok(msg) = self.clips[active].decode_buf.error_events().try_recv() {
                    let _ = self.event_tx.try_send(PlayerEvent::Error(msg));
                }
            }
            let trans_next = self.transition.as_ref().map(|tp| tp.next_idx);
            if let Some(next_idx) = trans_next {
                while let Ok(msg) = self.clips[next_idx].decode_buf.error_events().try_recv() {
                    let _ = self.event_tx.try_send(PlayerEvent::Error(msg));
                }
            }

            // ── Stopped / paused ──────────────────────────────────────────────
            if self.stopped.load(Ordering::Acquire) {
                break;
            }
            if self.paused.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
                continue;
            }

            // ── Reverse playback path ─────────────────────────────────────────
            if self.rate < 0.0 {
                let current = Duration::from_micros(self.current_pts.load(Ordering::Relaxed));
                let step = Duration::from_secs_f64(self.rate.abs() / fps.max(f64::MIN_POSITIVE));
                let target = current.saturating_sub(step);

                let clip_idx = self
                    .clips
                    .iter()
                    .position(|c| target >= c.timeline_start && target < c.timeline_end);

                if let Some(ci) = clip_idx {
                    let elapsed_tl = target.saturating_sub(self.clips[ci].timeline_start);
                    let clip_local = self.clips[ci].in_point
                        + if (self.clips[ci].speed - 1.0).abs() < 1e-9 {
                            elapsed_tl
                        } else {
                            elapsed_tl.mul_f64(self.clips[ci].speed)
                        };
                    if self.clips[ci].decode_buf.seek_coarse(clip_local).is_ok() {
                        if ci != self.active {
                            self.active = ci;
                            self.transition = None;
                        }
                        let deadline = std::time::Instant::now() + Duration::from_millis(300);
                        let frame = loop {
                            match self.clips[ci].decode_buf.pop_frame() {
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
                            let f_pts = f.timestamp().as_duration();
                            let elapsed = f_pts.saturating_sub(self.clips[ci].in_point);
                            let tl_pts = self.clips[ci].timeline_start
                                + if (self.clips[ci].speed - 1.0).abs() < 1e-9 {
                                    elapsed
                                } else {
                                    elapsed.div_f64(self.clips[ci].speed)
                                };
                            let w = f.width();
                            let h = f.height();
                            if self.sws_a.convert(&f, &mut self.rgba_a)
                                && let Some(sink) = self.sink.as_mut()
                            {
                                sink.push_frame(&self.rgba_a, w, h, tl_pts);
                            }
                            self.current_pts.store(
                                u64::try_from(tl_pts.as_micros()).unwrap_or(u64::MAX),
                                Ordering::Relaxed,
                            );
                            self.resume_pts = tl_pts;
                            let _ = self.event_tx.try_send(PlayerEvent::PositionUpdate(tl_pts));
                        }
                    }
                }

                if self
                    .clips
                    .first()
                    .is_some_and(|c| target < c.timeline_start)
                {
                    self.paused.store(true, Ordering::Release);
                }
                thread::sleep(frame_period);
                continue;
            }

            // ── Pop frame from active clip ─────────────────────────────────────
            let active = self.active;
            let pop_result = self.clips[active].decode_buf.pop_frame();

            match pop_result {
                FrameResult::Eof => {
                    let old_active = active;
                    if let Some(tp) = self.transition.take() {
                        self.active = tp.next_idx;
                    } else if active + 1 < self.clips.len() {
                        self.active += 1;
                    } else {
                        break;
                    }
                    if self.active != old_active {
                        // Clear the outgoing clip's pre-decoded audio so its stale
                        // samples do not continue to mix in after the transition.
                        if let Some(h) = self.clips[old_active].audio_track.clone() {
                            h.clear();
                        }
                        let in_pt = self.clips[self.active].in_point;
                        self.restart_audio_at(self.active, in_pt);
                    }
                }

                FrameResult::Seeking(last) => {
                    if let Some(ref f) = last {
                        let f_pts = f.timestamp().as_duration();
                        let in_pt = self.clips[active].in_point;
                        // Suppress pre-seek artefact frames: when a DecodeBuffer
                        // is opened and immediately seeked to in_point, the
                        // background thread may have decoded one frame from
                        // position 0 before processing the seek command. That
                        // frame ends up as `last` and must not be displayed —
                        // its content is from before the clip's in_point.
                        if f_pts >= in_pt {
                            let tl_start = self.clips[active].timeline_start;
                            let elapsed = f_pts.saturating_sub(in_pt);
                            let spd = self.clips[active].speed;
                            let tl_pts = tl_start
                                + if (spd - 1.0).abs() < 1e-9 {
                                    elapsed
                                } else {
                                    elapsed.div_f64(spd)
                                };
                            let w = f.width();
                            let h = f.height();
                            if self.sws_a.convert(f, &mut self.rgba_a)
                                && let Some(sink) = self.sink.as_mut()
                            {
                                sink.push_frame(&self.rgba_a, w, h, tl_pts);
                            }
                        }
                    }
                }

                FrameResult::Frame(frame) => {
                    let f_pts = frame.timestamp().as_duration();
                    let clip_in = self.clips[active].in_point;
                    let clip_out = self.clips[active].out_point;
                    let clip_tl_start = self.clips[active].timeline_start;
                    let clip_tl_end = self.clips[active].timeline_end;
                    let clip_speed = self.clips[active].speed;

                    // Skip frames before in_point (e.g. right after a seek).
                    if f_pts < clip_in {
                        continue;
                    }

                    // Treat frames past out_point as EOF for this clip.
                    let past_out = clip_out.is_some_and(|op| f_pts >= op);
                    let elapsed = f_pts.saturating_sub(clip_in);
                    // Remap source PTS → timeline PTS via speed factor.
                    // For speed=2.0 the clip occupies half the timeline duration;
                    // for speed=0.5 it occupies double.
                    let tl_elapsed = if (clip_speed - 1.0).abs() < 1e-9 {
                        elapsed
                    } else {
                        elapsed.div_f64(clip_speed)
                    };
                    let past_end = clip_tl_start + tl_elapsed >= clip_tl_end;

                    if past_out || past_end {
                        let old_active = active;
                        if let Some(tp) = self.transition.take() {
                            self.active = tp.next_idx;
                        } else if active + 1 < self.clips.len() {
                            self.active += 1;
                        } else {
                            break;
                        }
                        if self.active != old_active {
                            // Clear the outgoing clip's pre-decoded audio so its
                            // stale samples do not continue to mix in after the
                            // transition.
                            if let Some(h) = self.clips[old_active].audio_track.clone() {
                                h.clear();
                            }
                            let in_pt = self.clips[self.active].in_point;
                            self.restart_audio_at(self.active, in_pt);
                        }
                        continue;
                    }

                    let timeline_pts = clip_tl_start + tl_elapsed;

                    // ── Manage audio-only decode threads ──────────────────────
                    for at in &mut self.audio_only_tracks {
                        let should_run =
                            timeline_pts >= at.timeline_start && timeline_pts < at.timeline_end;
                        let is_running = at.cancel.is_some();
                        if should_run && !is_running {
                            let local =
                                at.in_point + timeline_pts.saturating_sub(at.timeline_start);
                            at.start_at(local);
                        } else if !should_run && is_running {
                            at.stop();
                            // Clear stale pre-decoded samples so the mixer does
                            // not play this track's buffered audio past clip end.
                            at.handle.clear();
                        }
                        // Per-clip volume automation: an animated gain is evaluated at
                        // the timeline PTS each tick (a static gain was set at open).
                        if should_run && let AnimatedValue::Track(track) = &at.volume {
                            at.handle
                                .set_volume(db_to_linear(track.value_at(timeline_pts)));
                        }
                    }

                    // Primary-track volume automation for the active clip.
                    if let Some(handle) = &self.clips[active].audio_track
                        && let AnimatedValue::Track(track) = &self.clips[active].volume
                    {
                        handle.set_volume(db_to_linear(track.value_at(timeline_pts)));
                    }

                    // Update shared current_pts and resume anchor.
                    self.current_pts.store(
                        u64::try_from(timeline_pts.as_micros()).unwrap_or(u64::MAX),
                        Ordering::Relaxed,
                    );
                    self.resume_pts = timeline_pts;

                    // ── Transition zone entry check ────────────────────────────
                    if self.transition.is_none() && active + 1 < self.clips.len() {
                        let next = &self.clips[active + 1];
                        if next.xfade_dur > Duration::ZERO && timeline_pts >= next.timeline_start {
                            if timeline_pts < next.timeline_start + next.xfade_dur {
                                self.transition = Some(TransitionState {
                                    next_idx: active + 1,
                                    start: next.timeline_start,
                                    duration: next.xfade_dur,
                                    kind: next.xfade_kind.unwrap_or(XfadeTransition::Fade),
                                });
                            } else {
                                // Jumped past the entire transition zone.
                                let old_active = active;
                                self.active = active + 1;
                                if self.active != old_active {
                                    let in_pt = self.clips[self.active].in_point;
                                    self.restart_audio_at(self.active, in_pt);
                                }
                                continue;
                            }
                        }
                    }

                    // ── A/V sync (system clock) ───────────────────────────────
                    {
                        let clock_pts = self.clock.current_pts();
                        let diff = timeline_pts.as_secs_f64() - clock_pts.as_secs_f64();
                        let fp = frame_period.as_secs_f64();

                        // Only enter gap fill for an actual gap between clips.
                        // For slow-motion clips (speed < 1.0) the large diff is expected
                        // and should be handled by the `diff > fp` sleep below instead.
                        if diff > fp * 2.0
                            && (clip_speed - 1.0) > -1e-9
                            && self.transition.is_none()
                            && self.last_frame_w > 0
                        {
                            // Gap in the primary track: the next V1 clip starts more than
                            // 2 frame-periods ahead of the clock.  Synthesise black frames
                            // composited with overlay-layer content for every missing
                            // frame period so that V2 overlays and audio-only tracks
                            // remain live during the gap.
                            // With an explicit canvas, fill gaps at the canvas size so
                            // the preview frame size stays constant across gaps.
                            let (gw, gh) = self
                                .canvas
                                .unwrap_or((self.last_frame_w, self.last_frame_h));
                            let n = (gw * gh * 4) as usize;
                            'gap: loop {
                                // Drain incoming commands.
                                while let Ok(cmd) = self.cmd_rx.try_recv() {
                                    match cmd {
                                        PlayerCommand::Play => {
                                            self.clock.reset(self.resume_pts);
                                            self.stopped.store(false, Ordering::Release);
                                            self.paused.store(false, Ordering::Release);
                                        }
                                        PlayerCommand::Pause => {
                                            self.paused.store(true, Ordering::Release);
                                        }
                                        PlayerCommand::Stop => {
                                            self.stopped.store(true, Ordering::Release);
                                        }
                                        PlayerCommand::SetRate(r) if r > 0.0 => {
                                            self.rate = r;
                                            self.clock.set_rate(r);
                                        }
                                        _ => {}
                                    }
                                }
                                if self.stopped.load(Ordering::Acquire) {
                                    break 'gap;
                                }
                                if self.paused.load(Ordering::Acquire) {
                                    thread::sleep(Duration::from_millis(5));
                                    continue 'gap;
                                }
                                let gap_pts = self.clock.current_pts();
                                if gap_pts + frame_period >= timeline_pts {
                                    break 'gap;
                                }
                                // Build a black base and composite the overlays onto it
                                // through the shared compositor — same held-frame timing,
                                // effects, and blend modes as the main present path.
                                self.gap_buf.resize(n, 0);
                                self.gap_buf.fill(0);
                                let gap_overlays = self.sync_overlays(gap_pts);
                                let gap_lavfi = self.sync_lavfi(gap_pts);
                                // Composite when there is a file overlay OR a lavfi
                                // overlay to draw over the gap's black base.
                                let gap_composited =
                                    if gap_overlays.is_empty() && gap_lavfi.is_none() {
                                        None
                                    } else {
                                        let base_layer = RealtimeLayer {
                                            width: gw,
                                            height: gh,
                                            pixel_format: PixelFormat::Rgba,
                                            effects: Vec::new(),
                                            opacity: AnimatedValue::Static(1.0),
                                            x: AnimatedValue::Static(0.0),
                                            y: AnimatedValue::Static(0.0),
                                            scale_x: AnimatedValue::Static(1.0),
                                            scale_y: AnimatedValue::Static(1.0),
                                            rotation: AnimatedValue::Static(0.0),
                                            blend_mode: BlendMode::Normal,
                                            composite_op: ff_filter::CompositeOp::Over,
                                        };
                                        match VideoFrame::from_rgba(gw, gh, self.gap_buf.clone()) {
                                            Ok(bf) => self.composite_frame(
                                                base_layer,
                                                usize::MAX,
                                                bf,
                                                gw,
                                                gh,
                                                &gap_overlays,
                                                gap_pts,
                                            ),
                                            Err(_) => None,
                                        }
                                    };
                                // Manage audio-only decode threads (A1/A2…).
                                for at in &mut self.audio_only_tracks {
                                    let should_run =
                                        gap_pts >= at.timeline_start && gap_pts < at.timeline_end;
                                    let is_running = at.cancel.is_some();
                                    if should_run && !is_running {
                                        let local =
                                            at.in_point + gap_pts.saturating_sub(at.timeline_start);
                                        at.start_at(local);
                                    } else if !should_run && is_running {
                                        at.stop();
                                        at.handle.clear();
                                    }
                                }
                                // Manage V1 inline audio: start it the moment the
                                // gap clock reaches the active clip's timeline_start.
                                if self.active_audio_cancel.is_none()
                                    && self.clips[self.active].audio_track.is_some()
                                    && gap_pts >= self.clips[self.active].timeline_start
                                {
                                    let tl_start = self.clips[self.active].timeline_start;
                                    let in_pt = self.clips[self.active].in_point;
                                    let gap_elapsed = gap_pts.saturating_sub(tl_start);
                                    let spd = self.clips[self.active].speed;
                                    let local = in_pt
                                        + if (spd - 1.0).abs() < 1e-9 {
                                            gap_elapsed
                                        } else {
                                            gap_elapsed.mul_f64(spd)
                                        };
                                    self.restart_audio_at(self.active, local);
                                }
                                self.current_pts.store(
                                    u64::try_from(gap_pts.as_micros()).unwrap_or(u64::MAX),
                                    Ordering::Relaxed,
                                );
                                self.resume_pts = gap_pts;
                                let _ =
                                    self.event_tx.try_send(PlayerEvent::PositionUpdate(gap_pts));
                                if let Some(sink) = self.sink.as_mut() {
                                    match &gap_composited {
                                        Some((rgba, cw, ch)) => {
                                            sink.push_frame(rgba, *cw, *ch, gap_pts);
                                        }
                                        None => sink.push_frame(&self.gap_buf, gw, gh, gap_pts),
                                    }
                                }
                                thread::sleep(frame_period);
                            }
                        } else if diff > fp {
                            let sleep_secs =
                                (diff - fp / 2.0).max(0.0) / self.rate.max(f64::MIN_POSITIVE);
                            thread::sleep(Duration::from_secs_f64(sleep_secs));
                        } else if diff < -fp {
                            log::debug!(
                                "timeline dropped late frame timeline_pts={timeline_pts:?} \
                                 clock_pts={clock_pts:?}"
                            );
                            continue;
                        }
                    }

                    // Start V1 inline audio on the first presented frame when a
                    // pre-roll gap prevented the thread from starting at open() time.
                    // The gap-fill loop attempts this but exits one frame-period before
                    // timeline_start, so we catch the remaining case here.
                    if self.active_audio_cancel.is_none()
                        && self.clips[active].audio_track.is_some()
                    {
                        let in_pt = self.clips[active].in_point;
                        let elapsed_tl =
                            timeline_pts.saturating_sub(self.clips[active].timeline_start);
                        let local = in_pt
                            + if (clip_speed - 1.0).abs() < 1e-9 {
                                elapsed_tl
                            } else {
                                elapsed_tl.mul_f64(clip_speed)
                            };
                        self.restart_audio_at(active, local);
                    }

                    // ── Present frame ─────────────────────────────────────────
                    let w = frame.width();
                    let h = frame.height();
                    self.last_frame_w = w;
                    self.last_frame_h = h;

                    // Copy transition fields to avoid holding a borrow while
                    // calling `pop_frame` on the next clip.
                    let (in_trans, next_idx, trans_start, trans_dur, trans_kind) =
                        match &self.transition {
                            Some(tp) => (true, tp.next_idx, tp.start, tp.duration, tp.kind),
                            None => (
                                false,
                                0,
                                Duration::ZERO,
                                Duration::ZERO,
                                XfadeTransition::Fade,
                            ),
                        };

                    let a_ok = self.sws_a.convert(&frame, &mut self.rgba_a);

                    if a_ok {
                        // V1 per-clip opacity: pre-multiply toward black (producer-side;
                        // the composer ignores base-layer opacity). The merged opacity is
                        // an `AnimatedValue`; a track is evaluated at the timeline PTS
                        // (tracks are timeline-global), so base-layer opacity animates too.
                        let v1_op = match &self.clips[active].layer_desc.opacity {
                            // Value is clamped to [0.0, 1.0], so the f32 narrowing is safe.
                            #[allow(clippy::cast_possible_truncation)]
                            AnimatedValue::Track(track) => {
                                track.value_at(timeline_pts).clamp(0.0, 1.0) as f32
                            }
                            AnimatedValue::Static(_) => self.clips[active].opacity,
                        };
                        if (v1_op - 1.0).abs() > 1e-6 {
                            for chunk in self.rgba_a.chunks_exact_mut(4) {
                                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                                {
                                    chunk[0] = (f32::from(chunk[0]) * v1_op).round() as u8;
                                    chunk[1] = (f32::from(chunk[1]) * v1_op).round() as u8;
                                    chunk[2] = (f32::from(chunk[2]) * v1_op).round() as u8;
                                }
                            }
                        }

                        // Transition crossfade (producer-side): blend the incoming clip
                        // into rgba_a so the composer grades the crossfaded V1 frame.
                        if in_trans
                            && let FrameResult::Frame(next_frame) =
                                self.clips[next_idx].decode_buf.pop_frame()
                            && self.sws_b.convert(&next_frame, &mut self.rgba_b)
                        {
                            let alpha = (timeline_pts.saturating_sub(trans_start).as_secs_f32()
                                / trans_dur.as_secs_f32())
                            .clamp(0.0, 1.0);
                            inner::apply_xfade(
                                trans_kind,
                                &self.rgba_a,
                                &self.rgba_b,
                                alpha,
                                w,
                                h,
                                &mut self.blend_buf,
                            );
                            std::mem::swap(&mut self.rgba_a, &mut self.blend_buf);
                        }

                        // Update overlays (held-frame, advanced by PTS) and composite
                        // the V1 base with them through the shared compositor.
                        let active_overlays = self.sync_overlays(timeline_pts);
                        self.sync_lavfi(timeline_pts);
                        let base_layer = RealtimeLayer::with_dimensions(
                            self.clips[active].layer_desc.clone(),
                            w,
                            h,
                            PixelFormat::Rgba,
                        );
                        let composited = match VideoFrame::from_rgba(w, h, self.rgba_a.clone()) {
                            Ok(bf) => self.composite_frame(
                                base_layer,
                                active,
                                bf,
                                w,
                                h,
                                &active_overlays,
                                timeline_pts,
                            ),
                            Err(_) => None,
                        };

                        // Deliver: the composited frame, or the raw V1 as a fallback.
                        if let Some(sink) = self.sink.as_mut() {
                            match &composited {
                                Some((rgba, cw, ch)) => {
                                    sink.push_frame(rgba, *cw, *ch, timeline_pts);
                                }
                                None => sink.push_frame(&self.rgba_a, w, h, timeline_pts),
                            }
                        }

                        // Advance past a completed transition.
                        if in_trans && timeline_pts >= trans_start + trans_dur {
                            let old_active = self.active;
                            self.transition = None;
                            self.active = next_idx;
                            if self.active != old_active {
                                let in_pt = self.clips[self.active].in_point;
                                self.restart_audio_at(self.active, in_pt);
                            }
                        }
                    }

                    let _ = self
                        .event_tx
                        .try_send(PlayerEvent::PositionUpdate(timeline_pts));
                }
            }
        }

        let _ = self.event_tx.try_send(PlayerEvent::Eof);
        if let Some(sink) = self.sink.as_mut() {
            sink.flush();
        }
        Ok(())
    }

    /// Seek all decode buffers so that `active` is the clip containing `target`
    /// and that clip's buffer is positioned at the correct source-file PTS.
    ///
    /// When `target` falls in a pre-roll or inter-clip gap the method finds the
    /// next clip after `target`, seeks it to its `in_point`, and returns without
    /// starting audio — the gap-fill loop in `run()` will start audio at the
    /// right time.
    pub(super) fn seek_timeline(&mut self, target: Duration) -> Result<(), PreviewError> {
        // Try to find a clip that contains `target`.
        let clip_in_range = self
            .clips
            .iter()
            .position(|c| target >= c.timeline_start && target < c.timeline_end);

        // If target is in a gap, find the next clip after `target`.
        let (clip_idx, clip_local_pts, is_gap_seek) = if let Some(ci) = clip_in_range {
            let elapsed_tl = target.saturating_sub(self.clips[ci].timeline_start);
            let local = self.clips[ci].in_point
                + if (self.clips[ci].speed - 1.0).abs() < 1e-9 {
                    elapsed_tl
                } else {
                    elapsed_tl.mul_f64(self.clips[ci].speed)
                };
            (ci, local, false)
        } else if let Some(ci) = self.clips.iter().position(|c| c.timeline_start > target) {
            // Seek the clip to its in_point; gap-fill loop will tick until it starts.
            (ci, self.clips[ci].in_point, true)
        } else {
            return Err(PreviewError::SeekOutOfRange { pts: target });
        };

        self.clips[clip_idx].decode_buf.seek(clip_local_pts)?;
        self.active = clip_idx;
        self.transition = None;

        // Discard stale audio and restart from the seek position.
        if let Some(mixer_arc) = &self.audio_mixer {
            mixer_arc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .invalidate_all();
        }
        if is_gap_seek {
            // Cancel any running V1 audio thread; the gap loop will restart it
            // once the clock reaches the clip's timeline_start.
            if let Some(cancel) = self.active_audio_cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            drop(self.active_audio_thread.take());
        } else {
            self.restart_audio_at(clip_idx, clip_local_pts);
        }

        // Seek overlay layers to the new target position.
        for layer in &mut self.overlay_layers {
            let cidx = layer
                .clips
                .iter()
                .position(|c| target >= c.timeline_start && target < c.timeline_end);
            if let Some(cidx) = cidx {
                let local = layer.clips[cidx].in_point
                    + target.saturating_sub(layer.clips[cidx].timeline_start);
                let _ = layer.clips[cidx].decode_buf.seek(local);
                layer.active = cidx;
            }
        }

        // The lavfi overlay source exposes no seek — rebuild it so it restarts from
        // t=0. Static overlays are unaffected; a time-varying lavfi restarts (a
        // documented limitation).
        if let Some(st) = &mut self.lavfi {
            st.rebuild();
        }

        // Stop all audio-only threads; they restart on the next frame tick.
        for at in &mut self.audio_only_tracks {
            at.stop();
        }

        Ok(())
    }

    /// Coarse (I-frame only) seek variant of [`seek_timeline`].
    ///
    /// Does not restart audio or invalidate the mixer — caller is responsible.
    /// Used for the reverse→forward recovery path where latency matters more
    /// than frame-accurate positioning.
    fn seek_timeline_coarse(&mut self, target: Duration) -> Result<(), PreviewError> {
        let clip_idx = self
            .clips
            .iter()
            .position(|c| target >= c.timeline_start && target < c.timeline_end)
            .ok_or(PreviewError::SeekOutOfRange { pts: target })?;
        let elapsed_tl = target.saturating_sub(self.clips[clip_idx].timeline_start);
        let clip_local_pts = self.clips[clip_idx].in_point
            + if (self.clips[clip_idx].speed - 1.0).abs() < 1e-9 {
                elapsed_tl
            } else {
                elapsed_tl.mul_f64(self.clips[clip_idx].speed)
            };
        self.clips[clip_idx]
            .decode_buf
            .seek_coarse(clip_local_pts)?;
        self.active = clip_idx;
        self.transition = None;
        // Keep the lavfi overlay consistent with the main seek path (no source seek).
        if let Some(st) = &mut self.lavfi {
            st.rebuild();
        }
        Ok(())
    }

    /// Cancel the current audio decode thread (if any) and start a new one
    /// for `clip_idx` beginning at `start_pts`.
    fn restart_audio_at(&mut self, clip_idx: usize, start_pts: Duration) {
        // Cancel and drop the previous thread.
        if let Some(cancel) = &self.active_audio_cancel {
            cancel.store(true, Ordering::Release);
        }
        drop(self.active_audio_thread.take());
        self.active_audio_cancel = None;

        let Some(handle) = self.clips.get(clip_idx).and_then(|c| c.audio_track.clone()) else {
            return;
        };
        handle.clear(); // discard stale samples

        let c = &self.clips[clip_idx];
        let source = c.source.clone();
        // V1 clip audio honours its own fades + speed (the A-track path already does).
        // `clip_dur` must be the SOURCE-time span (the resampler multiplies it by
        // `1/speed` to get timeline time, as the A-tracks feed `out-in` source span);
        // the timeline span is `source/speed`, so scale it back by `speed`.
        let fades = AudioFadeConfig {
            fade_in: c.fade_in,
            fade_out: c.fade_out,
            clip_dur: c
                .timeline_end
                .saturating_sub(c.timeline_start)
                .mul_f64(c.speed),
            in_point: c.in_point,
            speed: c.speed,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let thread =
            spawn_audio_track_thread(source, start_pts, handle, Arc::clone(&cancel), fades);
        self.active_audio_cancel = Some(cancel);
        self.active_audio_thread = Some(thread);
    }
}

impl Drop for SceneRunner {
    fn drop(&mut self) {
        if let Some(cancel) = &self.active_audio_cancel {
            cancel.store(true, Ordering::Release);
        }
        if let Some(h) = self.active_audio_thread.take() {
            let _ = h.join();
        }
    }
}
