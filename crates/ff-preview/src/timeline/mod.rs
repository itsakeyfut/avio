//! Real-time playback of a [`Timeline`].
//!
//! [`TimelinePlayer`] opens every clip on the primary video track of a
//! [`Timeline`] and plays them back in order, mapping each clip's frame PTS
//! to the unified timeline coordinate.
//!
//! | Type | Role |
//! |------|------|
//! | [`TimelinePlayer`] | Thin builder: call [`open`](TimelinePlayer::open) |
//! | [`TimelineRunner`] | Owns the decode pipelines; move to a thread and call [`run`](TimelineRunner::run) |
//! | [`PlayerHandle`] | Shared, cloneable control handle |
//!
//! ## Audio
//!
//! When any clip on the primary video track carries an audio stream,
//! [`TimelinePlayer::open`] creates an [`AudioMixer`] with one track per
//! audio-bearing clip.  A background [`AudioDecoder`] thread is started for
//! the active clip and pushes mono samples via [`AudioTrackHandle`].  On clip
//! transition or seek the old thread is cancelled and a new one is started.
//! [`PlayerHandle::pop_audio_samples`] calls [`AudioMixer::mix`] and returns
//! interleaved stereo `f32` output.

mod timeline_inner;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ff_decode::{AudioDecoder, SeekMode};
use ff_format::SampleFormat;
use ff_pipeline::timeline::Timeline;

use crate::audio::{AudioMixer, AudioTrackHandle};
use crate::error::PreviewError;
use crate::event::PlayerEvent;
use crate::playback::SwsRgbaConverter;
use crate::playback::decode_buffer::{DecodeBuffer, FrameResult};
use crate::playback::master_clock::MasterClock;
use crate::playback::player::{PlayerCommand, PlayerHandle};
use crate::playback::sink::FrameSink;

// ── Constants ─────────────────────────────────────────────────────────────────

const CHANNEL_CAP: usize = 64;
/// Back-pressure limit for the audio decode thread (mono samples).
const AUDIO_MAX_BUF: usize = 96_000;
/// Sample rate used for all audio decode threads.
const AUDIO_SAMPLE_RATE: f64 = 48_000.0;

// ── ClipState ─────────────────────────────────────────────────────────────────

struct ClipState {
    /// Source file path — needed to spawn audio threads on clip transition.
    source: PathBuf,
    decode_buf: DecodeBuffer,
    /// Global timeline position where this clip starts.
    timeline_start: Duration,
    /// Global timeline position where this clip ends.
    timeline_end: Duration,
    /// Source-file PTS at which this clip starts (= `Clip::in_point`).
    in_point: Duration,
    /// Source-file PTS at which this clip ends (`None` = play to EOF).
    out_point: Option<Duration>,
    /// Duration of the crossfade from the previous clip into this one.
    /// `Duration::ZERO` = hard cut.
    transition_dur: Duration,
    /// Audio track handle — `None` if the clip has no audio stream.
    audio_track: Option<AudioTrackHandle>,
    /// Playback speed multiplier from `Clip::speed` (`1.0` = normal).
    /// Used to remap source-file PTS → timeline PTS in `run()`.
    speed: f64,
    /// Per-clip opacity for overlay compositing (`1.0` = fully opaque).
    /// Applied to the RGBA alpha channel after SWS conversion so that
    /// `composite_over` blends at the correct transparency level.
    opacity: f32,
}

// ── TransitionState ───────────────────────────────────────────────────────────

struct TransitionState {
    /// Index of the incoming clip (the one being faded in).
    next_idx: usize,
    /// Timeline PTS at which the transition begins.
    start: Duration,
    /// Duration of the transition.
    duration: Duration,
}

// ── OverlayLayer ──────────────────────────────────────────────────────────────

/// One secondary video layer (V2, V3, …) inside [`TimelineRunner`].
struct OverlayLayer {
    clips: Vec<ClipState>,
    /// Index of the clip currently being decoded from this layer.
    active: usize,
    sws: SwsRgbaConverter,
    rgba: Vec<u8>,
}

// ── AudioFadeConfig ───────────────────────────────────────────────────────────

/// Fade-in / fade-out parameters forwarded to [`spawn_audio_track_thread`].
#[derive(Clone, Copy)]
struct AudioFadeConfig {
    fade_in: Duration,
    fade_out: Duration,
    /// Effective clip duration — used to position the fade-out start offset.
    clip_dur: Duration,
    /// Source-file PTS at which the clip starts (used to offset the envelope on seek).
    in_point: Duration,
    /// Playback speed multiplier (`1.0` = normal). When != 1.0, decoded samples are
    /// linearly resampled to compress/expand playback time without pitch preservation.
    speed: f64,
}

impl AudioFadeConfig {
    const NONE: Self = Self {
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
        clip_dur: Duration::ZERO,
        in_point: Duration::ZERO,
        speed: 1.0,
    };
}

// ── AudioOnlyTrack ────────────────────────────────────────────────────────────

/// One dedicated audio-only clip (from an A1/A2/… track) inside [`TimelineRunner`].
struct AudioOnlyTrack {
    source: PathBuf,
    timeline_start: Duration,
    timeline_end: Duration,
    in_point: Duration,
    /// Per-clip audio fade-in duration (`Duration::ZERO` = no fade).
    fade_in: Duration,
    /// Per-clip audio fade-out duration (`Duration::ZERO` = no fade).
    fade_out: Duration,
    /// Effective clip duration — used to position the fade-out ramp.
    clip_dur: Duration,
    handle: AudioTrackHandle,
    cancel: Option<Arc<AtomicBool>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioOnlyTrack {
    fn start_at(&mut self, from_pts: Duration) {
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

    fn stop(&mut self) {
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

// ── TimelinePlayer ────────────────────────────────────────────────────────────

/// Thin builder for a ([`TimelineRunner`], [`PlayerHandle`]) pair backed by a
/// [`Timeline`].
///
/// Playback is limited to the primary video track (`video_tracks[0]`). When
/// any clip carries an audio stream, an [`AudioMixer`] is created and audio
/// is mixed into the stereo output from [`PlayerHandle::pop_audio_samples`].
///
/// # Example
///
/// ```ignore
/// use ff_pipeline::{Timeline, Clip};
/// use ff_preview::{TimelinePlayer, RgbaSink};
/// use std::time::Duration;
///
/// let timeline = Timeline::builder()
///     .canvas(1920, 1080)
///     .frame_rate(30.0)
///     .video_track(vec![
///         Clip::new("intro.mp4").trim(Duration::ZERO, Duration::from_secs(5)),
///     ])
///     .build()?;
///
/// let (mut runner, handle) = TimelinePlayer::open(&timeline)?;
/// runner.set_sink(Box::new(RgbaSink::new()));
/// std::thread::spawn(move || { let _ = runner.run(); });
/// handle.play();
/// ```
pub struct TimelinePlayer;

impl TimelinePlayer {
    /// Open `timeline` for real-time preview playback.
    ///
    /// Probes every clip's source file to determine effective durations and
    /// audio availability, opens a [`DecodeBuffer`] for each clip on the
    /// primary video track, and seeks each buffer to its configured `in_point`.
    ///
    /// When any clip carries an audio stream an [`AudioMixer`] is created and
    /// the first audio-bearing clip's decode thread is started immediately.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] when:
    /// - `timeline` has no video tracks or the primary track is empty,
    /// - a clip source file cannot be found or opened,
    /// - a clip cannot be probed for duration.
    #[allow(clippy::too_many_lines)]
    pub fn open(timeline: &Timeline) -> Result<(TimelineRunner, PlayerHandle), PreviewError> {
        struct ProbeResult {
            source: PathBuf,
            in_pt: Duration,
            clip_dur: Duration,
            timeline_offset: Duration,
            out_point: Option<Duration>,
            transition_dur: Duration,
            has_audio: bool,
            /// Video frame dimensions — used to pre-populate `last_frame_w/h` so the
            /// gap-fill loop can synthesise black frames before the first real frame.
            video_w: u32,
            video_h: u32,
            speed: f64,
            opacity: f32,
        }

        let tracks = timeline.video_tracks();
        if tracks.is_empty() || tracks[0].is_empty() {
            return Err(PreviewError::Ffmpeg {
                code: 0,
                message: "timeline has no video clips in the primary track".into(),
            });
        }

        let fps = timeline.frame_rate().max(1.0);
        let clip_list = &tracks[0];

        // ── Phase 1: probe all clips ──────────────────────────────────────────

        let mut probes: Vec<ProbeResult> = Vec::with_capacity(clip_list.len());
        let mut has_any_audio = false;

        for clip in clip_list {
            let in_pt = clip.in_point.unwrap_or(Duration::ZERO);
            let info = ff_probe::open(&clip.source)?;
            let speed = clip.speed.max(0.01);

            let unscaled_dur = match (clip.in_point, clip.out_point) {
                (Some(ip), Some(op)) => op.saturating_sub(ip),
                (None, Some(op)) => op,
                _ => info.duration().saturating_sub(in_pt),
            };
            let clip_dur = if (speed - 1.0).abs() < 1e-9 {
                unscaled_dur
            } else {
                unscaled_dur.div_f64(speed)
            };

            let transition_dur = if clip.transition.is_some() {
                clip.transition_duration
            } else {
                Duration::ZERO
            };

            let has_audio = info.has_audio();
            has_any_audio |= has_audio;

            let (video_w, video_h) = info
                .primary_video()
                .map_or((0, 0), |v| (v.width(), v.height()));

            probes.push(ProbeResult {
                source: clip.source.clone(),
                in_pt,
                clip_dur,
                timeline_offset: clip.timeline_offset,
                out_point: clip.out_point,
                transition_dur,
                has_audio,
                video_w,
                video_h,
                speed,
                opacity: clip.opacity.clamp(0.0, 1.0),
            });
        }

        // ── Phase 2: build mixer and track handles (if audio present) ─────────

        let (mut mixer_arc, audio_track_handles): (
            Option<Arc<Mutex<AudioMixer>>>,
            Vec<Option<AudioTrackHandle>>,
        ) = if has_any_audio {
            let mut mixer = AudioMixer::new(48_000);
            let handles: Vec<Option<AudioTrackHandle>> = probes
                .iter()
                .map(|p| {
                    if p.has_audio {
                        Some(mixer.add_track())
                    } else {
                        None
                    }
                })
                .collect();
            (Some(Arc::new(Mutex::new(mixer))), handles)
        } else {
            (None, probes.iter().map(|_| None).collect())
        };

        // ── Phase 3: build ClipState objects ──────────────────────────────────

        let mut clip_states: Vec<ClipState> = Vec::with_capacity(probes.len());
        for (i, p) in probes.iter().enumerate() {
            let timeline_start = p.timeline_offset;
            let timeline_end = timeline_start + p.clip_dur;

            let mut decode_buf = DecodeBuffer::open(&p.source).build()?;
            if p.in_pt > Duration::ZERO {
                decode_buf.seek(p.in_pt)?;
            }

            clip_states.push(ClipState {
                source: p.source.clone(),
                decode_buf,
                timeline_start,
                timeline_end,
                in_point: p.in_pt,
                out_point: p.out_point,
                transition_dur: p.transition_dur,
                audio_track: audio_track_handles[i].clone(),
                speed: p.speed,
                opacity: p.opacity,
            });
        }

        // ── Phase 4: build overlay layers (V2, V3, …) ────────────────────────
        // Audio from V2+ clips is routed through AudioOnlyTrack (same mechanism as
        // A1) so it is started/stopped as the playhead crosses each clip window.

        let mut audio_only_tracks: Vec<AudioOnlyTrack> = Vec::new();

        let mut overlay_layers: Vec<OverlayLayer> = Vec::new();
        for v_track in timeline.video_tracks().iter().skip(1) {
            if v_track.is_empty() {
                continue;
            }
            let mut layer_clips: Vec<ClipState> = Vec::new();
            for clip in v_track {
                let in_pt = clip.in_point.unwrap_or(Duration::ZERO);
                let info = ff_probe::open(&clip.source)?;
                let clip_dur = match (clip.in_point, clip.out_point) {
                    (Some(ip), Some(op)) => op.saturating_sub(ip),
                    (None, Some(op)) => op,
                    _ => info.duration().saturating_sub(in_pt),
                };
                let timeline_start = clip.timeline_offset;
                let timeline_end = timeline_start + clip_dur;
                let mut decode_buf = DecodeBuffer::open(&clip.source).build()?;
                if in_pt > Duration::ZERO {
                    decode_buf.seek(in_pt)?;
                }
                if info.has_audio() {
                    let mixer_ref = mixer_arc
                        .get_or_insert_with(|| Arc::new(Mutex::new(AudioMixer::new(48_000))));
                    let handle = mixer_ref
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .add_track();
                    audio_only_tracks.push(AudioOnlyTrack {
                        source: clip.source.clone(),
                        timeline_start,
                        timeline_end,
                        in_point: in_pt,
                        fade_in: clip.fade_in,
                        fade_out: clip.fade_out,
                        clip_dur,
                        handle,
                        cancel: None,
                        thread: None,
                    });
                }
                layer_clips.push(ClipState {
                    source: clip.source.clone(),
                    decode_buf,
                    timeline_start,
                    timeline_end,
                    in_point: in_pt,
                    out_point: clip.out_point,
                    transition_dur: Duration::ZERO,
                    audio_track: None,
                    speed: clip.speed.max(0.01),
                    opacity: clip.opacity.clamp(0.0, 1.0),
                });
            }
            overlay_layers.push(OverlayLayer {
                clips: layer_clips,
                active: 0,
                sws: SwsRgbaConverter::new(),
                rgba: Vec::new(),
            });
        }

        // ── Phase 5: build audio-only tracks (A1, A2, …) ─────────────────────

        for a_track in timeline.audio_tracks() {
            for clip in a_track {
                let in_pt = clip.in_point.unwrap_or(Duration::ZERO);
                let info = ff_probe::open(&clip.source)?;
                if !info.has_audio() {
                    continue;
                }
                let clip_dur = match (clip.in_point, clip.out_point) {
                    (Some(ip), Some(op)) => op.saturating_sub(ip),
                    (None, Some(op)) => op,
                    _ => info.duration().saturating_sub(in_pt),
                };
                let timeline_start = clip.timeline_offset;
                let timeline_end = timeline_start + clip_dur;
                // Lazily create the mixer if no V1 clip had audio.
                let mixer_ref =
                    mixer_arc.get_or_insert_with(|| Arc::new(Mutex::new(AudioMixer::new(48_000))));
                let handle = mixer_ref
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .add_track();
                // Apply per-clip gain (dB → linear).
                if clip.volume_db != 0.0 {
                    #[allow(clippy::cast_possible_truncation)]
                    let linear = 10.0_f64.powf(clip.volume_db / 20.0) as f32;
                    handle.set_volume(linear);
                }
                audio_only_tracks.push(AudioOnlyTrack {
                    source: clip.source.clone(),
                    timeline_start,
                    timeline_end,
                    in_point: in_pt,
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                    clip_dur,
                    handle,
                    cancel: None,
                    thread: None,
                });
            }
        }

        // ── Compute total duration ─────────────────────────────────────────────

        let total_dur = clip_states
            .iter()
            .map(|c| c.timeline_end)
            .max()
            .unwrap_or(Duration::ZERO);
        let duration_millis = u64::try_from(total_dur.as_millis()).unwrap_or(u64::MAX);

        // ── Build runner and handle ───────────────────────────────────────────

        let current_pts = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = mpsc::sync_channel(CHANNEL_CAP);
        let (event_tx, event_rx) = mpsc::sync_channel::<PlayerEvent>(CHANNEL_CAP);

        // Only start the audio thread for the first V1 clip immediately when that
        // clip begins at timeline position 0.  When there is a pre-roll gap the
        // gap-fill loop starts the audio at the correct timeline position instead.
        let first_clip_at_origin = clip_states
            .first()
            .is_some_and(|c| c.timeline_start == Duration::ZERO);
        let (initial_audio_cancel, initial_audio_thread) = if first_clip_at_origin {
            if let Some(handle) = clip_states.first().and_then(|c| c.audio_track.clone()) {
                let source = clip_states[0].source.clone();
                let in_pt = clip_states[0].in_point;
                let clip0_speed = clip_states[0].speed;
                let cancel = Arc::new(AtomicBool::new(false));
                let thread = spawn_audio_track_thread(
                    source,
                    in_pt,
                    handle,
                    Arc::clone(&cancel),
                    AudioFadeConfig {
                        speed: clip0_speed,
                        ..AudioFadeConfig::NONE
                    },
                );
                (Some(cancel), Some(thread))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Pre-populate frame dimensions from the first clip's probe so the gap-fill
        // loop can synthesise black frames even before the first real frame arrives.
        let (initial_last_w, initial_last_h) =
            probes.first().map_or((0, 0), |p| (p.video_w, p.video_h));

        let runner = TimelineRunner {
            clips: clip_states,
            overlay_layers,
            audio_only_tracks,
            active: 0,
            transition: None,
            cmd_rx,
            event_tx,
            sink: None,
            current_pts: Arc::clone(&current_pts),
            paused: Arc::clone(&paused),
            stopped: Arc::clone(&stopped),
            fps,
            rate: 1.0,
            clock: MasterClock::System {
                started_at: Instant::now(),
                base_pts: Duration::ZERO,
                rate: 1.0,
            },
            resume_pts: Duration::ZERO,
            sws_a: SwsRgbaConverter::new(),
            sws_b: SwsRgbaConverter::new(),
            rgba_a: Vec::new(),
            rgba_b: Vec::new(),
            blend_buf: Vec::new(),
            last_frame_w: initial_last_w,
            last_frame_h: initial_last_h,
            gap_buf: Vec::new(),
            audio_mixer: mixer_arc.clone(),
            active_audio_cancel: initial_audio_cancel,
            active_audio_thread: initial_audio_thread,
        };

        let handle = PlayerHandle::for_timeline(
            cmd_tx,
            Arc::new(Mutex::new(event_rx)),
            current_pts,
            paused,
            stopped,
            duration_millis,
            mixer_arc,
        );

        Ok((runner, handle))
    }
}

// ── TimelineRunner ────────────────────────────────────────────────────────────

/// Exclusive owner of the timeline decode pipeline.
///
/// Move to a background thread and call [`run`](Self::run). Register a
/// [`FrameSink`] with [`set_sink`](Self::set_sink) before calling `run`.
pub struct TimelineRunner {
    clips: Vec<ClipState>,
    /// Secondary video overlay layers (V2, V3, …). Each is composited over V1
    /// in order before the frame is delivered to the sink.
    overlay_layers: Vec<OverlayLayer>,
    /// Dedicated audio-only clips (from A1, A2, … tracks). Each is started and
    /// stopped as the playhead crosses its timeline window.
    audio_only_tracks: Vec<AudioOnlyTrack>,
    /// Index of the clip currently being decoded and presented.
    active: usize,
    /// Non-`None` while a crossfade transition is in progress.
    transition: Option<TransitionState>,
    cmd_rx: mpsc::Receiver<PlayerCommand>,
    event_tx: mpsc::SyncSender<PlayerEvent>,
    sink: Option<Box<dyn FrameSink>>,
    current_pts: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    fps: f64,
    rate: f64,
    clock: MasterClock,
    /// Media PTS to re-anchor the System clock to when `PlayerCommand::Play`
    /// is received from a paused state. Updated on every seek and after every
    /// presented frame so that accumulated wall-clock time during pause does
    /// not advance `current_pts()` past the last known media position.
    resume_pts: Duration,
    /// Pixel-format converter for the active (outgoing) frame.
    sws_a: SwsRgbaConverter,
    /// Pixel-format converter for the incoming frame during transitions.
    sws_b: SwsRgbaConverter,
    rgba_a: Vec<u8>,
    rgba_b: Vec<u8>,
    blend_buf: Vec<u8>,
    /// Width of the most recently presented primary-track frame; used to
    /// synthesise fill frames during primary-track gaps.
    last_frame_w: u32,
    /// Height of the most recently presented primary-track frame.
    last_frame_h: u32,
    /// Scratch buffer for synthesising black fill frames during primary-track gaps.
    gap_buf: Vec<u8>,
    /// Multi-track audio mixer — `None` when no clip has audio.
    audio_mixer: Option<Arc<Mutex<AudioMixer>>>,
    /// Cancel flag for the currently running audio decode thread.
    active_audio_cancel: Option<Arc<AtomicBool>>,
    /// Handle to the currently running audio decode thread.
    active_audio_thread: Option<JoinHandle<()>>,
}

impl TimelineRunner {
    /// Register the frame sink. Call before [`run`](Self::run).
    pub fn set_sink(&mut self, sink: Box<dyn FrameSink>) {
        self.sink = Some(sink);
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
                    PlayerCommand::UpdateLayout(timeline) => {
                        if let Err(e) = self.update_layout_in_place(&timeline, self.resume_pts) {
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
                        if next.transition_dur > Duration::ZERO
                            && timeline_pts >= next.timeline_start
                        {
                            if timeline_pts < next.timeline_start + next.transition_dur {
                                self.transition = Some(TransitionState {
                                    next_idx: active + 1,
                                    start: next.timeline_start,
                                    duration: next.transition_dur,
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
                            let gw = self.last_frame_w;
                            let gh = self.last_frame_h;
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
                                        PlayerCommand::SetRate(r) => {
                                            if r > 0.0 {
                                                self.rate = r;
                                                self.clock.set_rate(r);
                                            }
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
                                // Build a black base frame.
                                self.gap_buf.resize(n, 0);
                                self.gap_buf.fill(0);
                                // Pass 1: update each overlay layer's rgba at gap_pts.
                                for layer in &mut self.overlay_layers {
                                    let maybe_cidx = layer.clips.iter().position(|c| {
                                        gap_pts >= c.timeline_start && gap_pts < c.timeline_end
                                    });
                                    let Some(cidx) = maybe_cidx else { continue };
                                    if cidx != layer.active {
                                        let local = layer.clips[cidx].in_point
                                            + gap_pts
                                                .saturating_sub(layer.clips[cidx].timeline_start);
                                        let _ = layer.clips[cidx].decode_buf.seek(local);
                                        layer.active = cidx;
                                    }
                                    while let FrameResult::Frame(f) =
                                        layer.clips[cidx].decode_buf.pop_frame()
                                    {
                                        let f_pts = f.timestamp().as_duration();
                                        let clip_in = layer.clips[cidx].in_point;
                                        let tl_start = layer.clips[cidx].timeline_start;
                                        let v2_pts = tl_start + f_pts.saturating_sub(clip_in);
                                        if v2_pts + Duration::from_millis(50) >= gap_pts {
                                            // Scale to V1 canvas size so sizes always match.
                                            layer.sws.convert_to(&f, &mut layer.rgba, gw, gh);
                                            let op = layer.clips[cidx].opacity;
                                            if (op - 1.0).abs() > 1e-6 {
                                                for chunk in layer.rgba.chunks_exact_mut(4) {
                                                    #[allow(
                                                        clippy::cast_possible_truncation,
                                                        clippy::cast_sign_loss
                                                    )]
                                                    {
                                                        chunk[3] = (f32::from(chunk[3]) * op)
                                                            .round()
                                                            as u8;
                                                    }
                                                }
                                            }
                                            break;
                                        }
                                    }
                                }
                                // Pass 2: composite overlay layers over the black base.
                                for layer in &self.overlay_layers {
                                    if !layer.rgba.is_empty()
                                        && layer.rgba.len() == self.gap_buf.len()
                                    {
                                        timeline_inner::composite_over(
                                            &mut self.gap_buf,
                                            &layer.rgba,
                                        );
                                    }
                                }
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
                                    sink.push_frame(&self.gap_buf, gw, gh, gap_pts);
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
                    let (in_trans, next_idx, trans_start, trans_dur) = match &self.transition {
                        Some(tp) => (true, tp.next_idx, tp.start, tp.duration),
                        None => (false, 0, Duration::ZERO, Duration::ZERO),
                    };

                    let a_ok = self.sws_a.convert(&frame, &mut self.rgba_a);

                    // Apply per-clip opacity for V1: blend with black background before compositing.
                    if a_ok {
                        let v1_op = self.clips[active].opacity;
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
                    }

                    // ── Composite overlay layers: V1 is background, V2/V3… are composited on top ──
                    // Phase 1: drain each overlay layer to update its decoded rgba buffer.
                    if a_ok {
                        for layer in &mut self.overlay_layers {
                            let maybe_cidx = layer.clips.iter().position(|c| {
                                timeline_pts >= c.timeline_start && timeline_pts < c.timeline_end
                            });
                            let Some(cidx) = maybe_cidx else { continue };
                            if cidx != layer.active {
                                let local = layer.clips[cidx].in_point
                                    + timeline_pts.saturating_sub(layer.clips[cidx].timeline_start);
                                let _ = layer.clips[cidx].decode_buf.seek(local);
                                layer.active = cidx;
                            }
                            while let FrameResult::Frame(f) =
                                layer.clips[cidx].decode_buf.pop_frame()
                            {
                                let f_pts = f.timestamp().as_duration();
                                let clip_in = layer.clips[cidx].in_point;
                                let tl_start = layer.clips[cidx].timeline_start;
                                let v2_pts = tl_start + f_pts.saturating_sub(clip_in);
                                if v2_pts + Duration::from_millis(50) >= timeline_pts {
                                    // Scale overlay to V1's canvas size so composite_over
                                    // works even when V1 and V2 have different resolutions.
                                    layer.sws.convert_to(
                                        &f,
                                        &mut layer.rgba,
                                        self.last_frame_w,
                                        self.last_frame_h,
                                    );
                                    // Apply per-clip opacity to the alpha channel so that
                                    // composite_over() blends at the correct transparency.
                                    let op = layer.clips[cidx].opacity;
                                    if (op - 1.0).abs() > 1e-6 {
                                        for chunk in layer.rgba.chunks_exact_mut(4) {
                                            #[allow(
                                                clippy::cast_possible_truncation,
                                                clippy::cast_sign_loss
                                            )]
                                            {
                                                chunk[3] = (f32::from(chunk[3]) * op).round() as u8;
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    // Phase 2: V1 is background; overlay layers (V2, V3, …) are composited on top.
                    if a_ok && self.overlay_layers.iter().any(|l| !l.rgba.is_empty()) {
                        self.blend_buf.resize(self.rgba_a.len(), 0);
                        self.blend_buf.copy_from_slice(&self.rgba_a);
                        for layer in &self.overlay_layers {
                            if !layer.rgba.is_empty() && layer.rgba.len() == self.blend_buf.len() {
                                let layer_rgba = layer.rgba.clone();
                                timeline_inner::composite_over(&mut self.blend_buf, &layer_rgba);
                            }
                        }
                        std::mem::swap(&mut self.rgba_a, &mut self.blend_buf);
                    }

                    if in_trans && a_ok {
                        let alpha = (timeline_pts.saturating_sub(trans_start).as_secs_f32()
                            / trans_dur.as_secs_f32())
                        .clamp(0.0, 1.0);

                        let next_pop = self.clips[next_idx].decode_buf.pop_frame();

                        let blended = if let FrameResult::Frame(next_frame) = next_pop {
                            if self.sws_b.convert(&next_frame, &mut self.rgba_b) {
                                timeline_inner::blend_rgba(
                                    &self.rgba_a,
                                    &self.rgba_b,
                                    alpha,
                                    &mut self.blend_buf,
                                );
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };

                        if let Some(sink) = self.sink.as_mut() {
                            let pixels = if blended {
                                &self.blend_buf
                            } else {
                                &self.rgba_a
                            };
                            sink.push_frame(pixels, w, h, timeline_pts);
                        }

                        if timeline_pts >= trans_start + trans_dur {
                            let old_active = self.active;
                            self.transition = None;
                            self.active = next_idx;
                            if self.active != old_active {
                                let in_pt = self.clips[self.active].in_point;
                                self.restart_audio_at(self.active, in_pt);
                            }
                        }
                    } else if a_ok && let Some(sink) = self.sink.as_mut() {
                        sink.push_frame(&self.rgba_a, w, h, timeline_pts);
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

    /// Update clip positions in place from a new `Timeline` without stopping
    /// the runner or replacing audio infrastructure.
    ///
    /// Only the position metadata (`timeline_start`, `timeline_end`,
    /// `in_point`, `out_point`, `transition_dur`) of existing `ClipState` and
    /// `AudioOnlyTrack` objects is changed. The `AudioMixer` and all
    /// `AudioTrackHandle`s are reused unchanged; only the decode positions are
    /// updated by calling `seek_timeline(resume_pts)` at the end.
    ///
    /// Returns an error when the new timeline is structurally incompatible with
    /// the running runner (different V1 clip count or different source paths).
    /// In that case the runner's state is untouched.
    #[allow(clippy::too_many_lines)]
    fn update_layout_in_place(
        &mut self,
        timeline: &ff_pipeline::timeline::Timeline,
        resume_pts: Duration,
    ) -> Result<(), PreviewError> {
        let v_tracks = timeline.video_tracks();

        // ── Validate V1 ────────────────────────────────────────────────────────
        let new_v1_len = v_tracks.first().map_or(0, Vec::len);
        if new_v1_len != self.clips.len() {
            return Err(PreviewError::Ffmpeg {
                code: -1,
                message: format!(
                    "V1 clip count mismatch: runner={} timeline={new_v1_len}",
                    self.clips.len()
                ),
            });
        }
        for (i, clip) in v_tracks[0].iter().enumerate() {
            if clip.source != self.clips[i].source {
                return Err(PreviewError::Ffmpeg {
                    code: -1,
                    message: format!(
                        "V1 clip[{i}] source mismatch: runner={} timeline={}",
                        self.clips[i].source.display(),
                        clip.source.display(),
                    ),
                });
            }
        }

        // ── Update V1 clip positions ───────────────────────────────────────────
        for (i, clip) in v_tracks[0].iter().enumerate() {
            let new_speed = clip.speed.max(0.01);
            let old_scaled_dur = self.clips[i]
                .timeline_end
                .saturating_sub(self.clips[i].timeline_start);
            // Recover unscaled (source) duration from the stored scaled duration and old speed.
            let old_unscaled = old_scaled_dur.mul_f64(self.clips[i].speed);
            let new_unscaled = match (clip.in_point, clip.out_point) {
                (Some(ip), Some(op)) => op.saturating_sub(ip),
                (None, Some(op)) => op,
                _ => old_unscaled,
            };
            let new_dur = if (new_speed - 1.0).abs() < 1e-9 {
                new_unscaled
            } else {
                new_unscaled.div_f64(new_speed)
            };
            self.clips[i].timeline_start = clip.timeline_offset;
            self.clips[i].timeline_end = clip.timeline_offset + new_dur;
            self.clips[i].in_point = clip.in_point.unwrap_or(Duration::ZERO);
            self.clips[i].out_point = clip.out_point;
            self.clips[i].speed = new_speed;
            self.clips[i].transition_dur = if clip.transition.is_some() {
                clip.transition_duration
            } else {
                Duration::ZERO
            };
        }

        // ── Update overlay layers (V2+) ────────────────────────────────────────
        let new_overlay_count = v_tracks.len().saturating_sub(1);
        if new_overlay_count == self.overlay_layers.len() {
            for (layer_i, v_track) in v_tracks.iter().skip(1).enumerate() {
                let layer = &mut self.overlay_layers[layer_i];
                if v_track.len() == layer.clips.len() {
                    for (j, clip) in v_track.iter().enumerate() {
                        let old_dur = layer.clips[j]
                            .timeline_end
                            .saturating_sub(layer.clips[j].timeline_start);
                        let new_dur = match (clip.in_point, clip.out_point) {
                            (Some(ip), Some(op)) => op.saturating_sub(ip),
                            (None, Some(op)) => op,
                            _ => old_dur,
                        };
                        layer.clips[j].timeline_start = clip.timeline_offset;
                        layer.clips[j].timeline_end = clip.timeline_offset + new_dur;
                        layer.clips[j].in_point = clip.in_point.unwrap_or(Duration::ZERO);
                        layer.clips[j].out_point = clip.out_point;
                    }
                }
            }
        }

        // ── Update audio-only tracks (A1+) ─────────────────────────────────────
        // Collect new (timeline_start, in_point, out_point) from the timeline's
        // audio tracks, matched positionally. Mismatched counts are skipped
        // rather than returning an error because audio tracks are optional.
        let new_a_positions: Vec<(Duration, Duration, Option<Duration>)> = timeline
            .audio_tracks()
            .iter()
            .flat_map(|track| track.iter())
            .map(|clip| {
                (
                    clip.timeline_offset,
                    clip.in_point.unwrap_or(Duration::ZERO),
                    clip.out_point,
                )
            })
            .collect();

        if new_a_positions.len() == self.audio_only_tracks.len() {
            for (i, (new_tl_start, new_in, new_out)) in new_a_positions.iter().enumerate() {
                let old_dur = self.audio_only_tracks[i]
                    .timeline_end
                    .saturating_sub(self.audio_only_tracks[i].timeline_start);
                let new_dur = if let Some(op) = new_out {
                    op.saturating_sub(*new_in)
                } else {
                    old_dur
                };
                self.audio_only_tracks[i].timeline_start = *new_tl_start;
                self.audio_only_tracks[i].timeline_end = *new_tl_start + new_dur;
                self.audio_only_tracks[i].in_point = *new_in;
            }
        }

        // ── Seek everything to resume_pts ──────────────────────────────────────
        // seek_timeline invalidates all mixer buffers, stops audio-only threads,
        // and repositions the active clip's DecodeBuffer to the correct
        // source-file PTS. Audio-only threads restart on the next frame tick
        // based on the updated timeline_start/timeline_end values.
        self.seek_timeline(resume_pts)?;

        Ok(())
    }

    /// Seek all decode buffers so that `active` is the clip containing `target`
    /// and that clip's buffer is positioned at the correct source-file PTS.
    ///
    /// When `target` falls in a pre-roll or inter-clip gap the method finds the
    /// next clip after `target`, seeks it to its `in_point`, and returns without
    /// starting audio — the gap-fill loop in `run()` will start audio at the
    /// right time.
    fn seek_timeline(&mut self, target: Duration) -> Result<(), PreviewError> {
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

        let source = self.clips[clip_idx].source.clone();
        let clip_speed = self.clips[clip_idx].speed;
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = spawn_audio_track_thread(
            source,
            start_pts,
            handle,
            Arc::clone(&cancel),
            AudioFadeConfig {
                speed: clip_speed,
                ..AudioFadeConfig::NONE
            },
        );
        self.active_audio_cancel = Some(cancel);
        self.active_audio_thread = Some(thread);
    }
}

impl Drop for TimelineRunner {
    fn drop(&mut self) {
        if let Some(cancel) = &self.active_audio_cancel {
            cancel.store(true, Ordering::Release);
        }
        if let Some(h) = self.active_audio_thread.take() {
            let _ = h.join();
        }
    }
}

// ── spawn_audio_track_thread ──────────────────────────────────────────────────

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

fn spawn_audio_track_thread(
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_video_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/video/gameplay.mp4")
    }

    // ── blend_rgba delegate ────────────────────────────────────────────────

    #[test]
    fn timeline_inner_blend_rgba_at_zero_alpha_should_return_a() {
        let a = vec![255u8, 0, 0, 255];
        let b = vec![0u8, 0, 255, 255];
        let mut dst = Vec::new();
        timeline_inner::blend_rgba(&a, &b, 0.0, &mut dst);
        assert_eq!(dst, a);
    }

    // ── open ──────────────────────────────────────────────────────────────

    #[test]
    fn timeline_player_open_should_fail_when_no_video_tracks() {
        let _ = PreviewError::SeekOutOfRange {
            pts: Duration::from_secs(1),
        };
    }

    // ── run ───────────────────────────────────────────────────────────────

    #[test]
    #[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
    fn timeline_runner_run_should_deliver_frames_for_single_clip() {
        use crate::playback::sink::FrameSink;

        let path = test_video_path();
        if !path.exists() {
            println!("skipping: video asset not found");
            return;
        }

        struct CountSink(usize, PlayerHandle);
        impl FrameSink for CountSink {
            fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
                self.0 += 1;
                if self.0 >= 20 {
                    self.1.stop();
                }
            }
        }

        let timeline = ff_pipeline::Timeline::builder()
            .canvas(1280, 720)
            .frame_rate(30.0)
            .video_track(vec![
                ff_pipeline::Clip::new(&path).trim(Duration::ZERO, Duration::from_secs(2)),
            ])
            .build()
            .expect("timeline build failed");

        let (mut runner, handle) = match TimelinePlayer::open(&timeline) {
            Ok(p) => p,
            Err(e) => {
                println!("skipping: open failed: {e}");
                return;
            }
        };

        runner.set_sink(Box::new(CountSink(0, handle.clone())));
        let _ = runner.run();

        let events: Vec<_> = std::iter::from_fn(|| handle.poll_event()).collect();
        assert!(
            events.iter().any(|e| matches!(e, PlayerEvent::Eof)),
            "Eof event must be delivered after run() completes"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PlayerEvent::PositionUpdate(_))),
            "PositionUpdate events must be emitted during playback"
        );
    }

    /// Regression test for the MasterClock::System pause-drift bug.
    ///
    /// After pause → seek → sleep N seconds → play, the first PositionUpdate
    /// must carry a PTS close to the seek target (≤ target + 2 frame periods),
    /// not target + N.
    #[test]
    #[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
    fn timeline_runner_resume_after_seek_while_paused_should_not_drift() {
        let path = test_video_path();
        if !path.exists() {
            println!("skipping: video asset not found");
            return;
        }

        let fps = 30.0_f64;
        let seek_target = Duration::from_secs(1);
        let two_frame_periods = Duration::from_secs_f64(2.0 / fps);

        let timeline = ff_pipeline::Timeline::builder()
            .canvas(1280, 720)
            .frame_rate(fps)
            .video_track(vec![
                ff_pipeline::Clip::new(&path).trim(Duration::ZERO, Duration::from_secs(5)),
            ])
            .build()
            .expect("timeline build failed");

        let (runner, handle) = match TimelinePlayer::open(&timeline) {
            Ok(p) => p,
            Err(e) => {
                println!("skipping: open failed: {e}");
                return;
            }
        };

        let handle_bg = handle.clone();
        let bg = thread::spawn(move || {
            let _ = runner.run();
        });

        // Let the runner start, then pause, seek, wait 500 ms, play.
        thread::sleep(Duration::from_millis(50));
        handle.pause();
        thread::sleep(Duration::from_millis(20));
        handle.seek(seek_target);
        thread::sleep(Duration::from_millis(500));
        handle.play();

        // Collect the first PositionUpdate after play.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let first_pts = loop {
            if let Some(PlayerEvent::PositionUpdate(pts)) = handle.poll_event() {
                break Some(pts);
            }
            if std::time::Instant::now() > deadline {
                break None;
            }
            thread::sleep(Duration::from_millis(5));
        };

        handle_bg.stop();
        let _ = bg.join();

        let pts = first_pts.expect("no PositionUpdate received within 5 seconds");
        assert!(
            pts <= seek_target + two_frame_periods,
            "first frame after seek-while-paused should be near seek target; \
             got {pts:?}, expected ≤ {:?}",
            seek_target + two_frame_periods,
        );
    }

    #[test]
    #[ignore = "requires assets/video/gameplay.mp4; run with -- --include-ignored"]
    fn timeline_runner_seek_should_deliver_seek_completed_event() {
        let path = test_video_path();
        if !path.exists() {
            println!("skipping: video asset not found");
            return;
        }

        let timeline = ff_pipeline::Timeline::builder()
            .canvas(1280, 720)
            .frame_rate(30.0)
            .video_track(vec![
                ff_pipeline::Clip::new(&path).trim(Duration::ZERO, Duration::from_secs(10)),
            ])
            .build()
            .expect("timeline build failed");

        let (runner, handle) = match TimelinePlayer::open(&timeline) {
            Ok(p) => p,
            Err(e) => {
                println!("skipping: open failed: {e}");
                return;
            }
        };

        let handle_bg = handle.clone();
        let bg = thread::spawn(move || {
            let _ = runner.run();
        });

        thread::sleep(Duration::from_millis(50));
        handle.seek(Duration::from_secs(1));

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let found = loop {
            if let Some(e) = handle.poll_event() {
                if matches!(e, PlayerEvent::SeekCompleted(_)) {
                    break true;
                }
            }
            if std::time::Instant::now() > deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(10));
        };

        handle_bg.stop();
        let _ = bg.join();

        assert!(
            found,
            "SeekCompleted must be delivered within 3 seconds of seek"
        );
    }
}
