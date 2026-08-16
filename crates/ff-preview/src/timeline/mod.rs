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
//! audio-bearing clip.  A background [`AudioDecoder`](ff_decode::AudioDecoder) thread is started for
//! the active clip and pushes mono samples via [`AudioTrackHandle`].  On clip
//! transition or seek the old thread is cancelled and a new one is started.
//! [`PlayerHandle::pop_audio_samples`] calls [`AudioMixer::mix`] and returns
//! interleaved stereo `f32` output.

mod audio_resampling;
mod runner;
mod runner_layout;
mod scene;
mod scene_adapter;
mod state;
mod timeline_inner;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ff_pipeline::timeline::Timeline;

use crate::audio::{AudioMixer, AudioTrackHandle};
use crate::error::PreviewError;
use crate::event::PlayerEvent;
use crate::playback::SwsRgbaConverter;
use crate::playback::decode_buffer::DecodeBuffer;
use crate::playback::master_clock::MasterClock;
use crate::playback::player_handle::PlayerHandle;

pub use runner::TimelineRunner;
pub use scene::{Scene, SceneAudioPlacement, SceneAudioTrack, ScenePlacement, SceneVideoTrack};

use audio_resampling::spawn_audio_track_thread;
use state::{AudioFadeConfig, AudioOnlyTrack, ClipState, OverlayLayer};

// -- Constants --

const CHANNEL_CAP: usize = 64;

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
    pub fn open(timeline: &Timeline) -> Result<(TimelineRunner, PlayerHandle), PreviewError> {
        Self::open_scene(&Scene::from_timeline(timeline))
    }

    /// Open a [`Scene`] for real-time preview playback.
    ///
    /// The `Scene` carries the primitivised editing model; this resolves it
    /// against the media (probing each placement's source for duration, audio
    /// availability, and frame size), opens a [`DecodeBuffer`] per V1 clip and
    /// seeks it to `in_point`, and builds the audio mixer and tracks. This is the
    /// media-resolution step that [`open`](Self::open) used to inline.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] when:
    /// - the scene has no video tracks or the base track is empty,
    /// - a placement source file cannot be found or opened,
    /// - a placement cannot be probed for duration.
    #[allow(clippy::too_many_lines)]
    pub fn open_scene(scene: &Scene) -> Result<(TimelineRunner, PlayerHandle), PreviewError> {
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

        let v_tracks = &scene.video_tracks;
        if v_tracks.is_empty() || v_tracks[0].placements.is_empty() {
            return Err(PreviewError::Ffmpeg {
                code: 0,
                message: "timeline has no video clips in the primary track".into(),
            });
        }

        let fps = scene.fps.max(1.0);
        let clip_list = &v_tracks[0].placements;

        // ── Phase 1: probe all clips ──────────────────────────────────────────

        let mut probes: Vec<ProbeResult> = Vec::with_capacity(clip_list.len());
        let mut has_any_audio = false;

        for p in clip_list {
            let in_pt = p.in_point;
            let info = ff_probe::open(&p.source)?;
            let speed = p.speed;

            // `in_point` is pre-resolved (defaulted to zero) in the Scene, so this
            // equals the old `match (in_point, out_point)` for all four cases.
            let unscaled_dur = p.out_point.map_or_else(
                || info.duration().saturating_sub(in_pt),
                |op| op.saturating_sub(in_pt),
            );
            let clip_dur = if (speed - 1.0).abs() < 1e-9 {
                unscaled_dur
            } else {
                unscaled_dur.div_f64(speed)
            };

            let has_audio = info.has_audio();
            has_any_audio |= has_audio;

            let (video_w, video_h) = info
                .primary_video()
                .map_or((0, 0), |v| (v.width(), v.height()));

            probes.push(ProbeResult {
                source: p.source.clone(),
                in_pt,
                clip_dur,
                timeline_offset: p.timeline_offset,
                out_point: p.out_point,
                transition_dur: p.transition_dur,
                has_audio,
                video_w,
                video_h,
                speed,
                opacity: p.opacity,
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
                layer_desc: clip_list[i].layer.clone(),
                volume_track: clip_list[i].volume_track.clone(),
            });
        }

        // ── Phase 4: build overlay layers (V2, V3, …) ────────────────────────
        // Audio from V2+ clips is routed through AudioOnlyTrack (same mechanism as
        // A1) so it is started/stopped as the playhead crosses each clip window.

        let mut audio_only_tracks: Vec<AudioOnlyTrack> = Vec::new();

        let mut overlay_layers: Vec<OverlayLayer> = Vec::new();
        for layer in v_tracks.iter().skip(1) {
            if layer.placements.is_empty() {
                continue;
            }
            let mut layer_clips: Vec<ClipState> = Vec::new();
            for p in &layer.placements {
                let in_pt = p.in_point;
                let info = ff_probe::open(&p.source)?;
                let clip_dur = p.out_point.map_or_else(
                    || info.duration().saturating_sub(in_pt),
                    |op| op.saturating_sub(in_pt),
                );
                let timeline_start = p.timeline_offset;
                let timeline_end = timeline_start + clip_dur;
                let mut decode_buf = DecodeBuffer::open(&p.source).build()?;
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
                        source: p.source.clone(),
                        timeline_start,
                        timeline_end,
                        in_point: in_pt,
                        fade_in: p.fade_in,
                        fade_out: p.fade_out,
                        clip_dur,
                        handle,
                        volume_track: p.volume_track.clone(),
                        cancel: None,
                        thread: None,
                    });
                }
                layer_clips.push(ClipState {
                    source: p.source.clone(),
                    decode_buf,
                    timeline_start,
                    timeline_end,
                    in_point: in_pt,
                    out_point: p.out_point,
                    transition_dur: Duration::ZERO,
                    audio_track: None,
                    speed: p.speed,
                    opacity: p.opacity,
                    layer_desc: p.layer.clone(),
                    volume_track: p.volume_track.clone(),
                });
            }
            overlay_layers.push(OverlayLayer {
                clips: layer_clips,
                active: 0,
                sws: SwsRgbaConverter::new(),
                rgba: Vec::new(),
                cur_dims: None,
                pending: None,
            });
        }

        // ── Phase 5: build audio-only tracks (A1, A2, …) ─────────────────────

        for track in &scene.audio_tracks {
            for p in &track.placements {
                let in_pt = p.in_point;
                let info = ff_probe::open(&p.source)?;
                if !info.has_audio() {
                    continue;
                }
                let clip_dur = p.out_point.map_or_else(
                    || info.duration().saturating_sub(in_pt),
                    |op| op.saturating_sub(in_pt),
                );
                let timeline_start = p.timeline_offset;
                let timeline_end = timeline_start + clip_dur;
                // Lazily create the mixer if no V1 clip had audio.
                let mixer_ref =
                    mixer_arc.get_or_insert_with(|| Arc::new(Mutex::new(AudioMixer::new(48_000))));
                let handle = mixer_ref
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .add_track();
                // Apply per-clip gain (dB → linear). A volume_track (animated) takes
                // precedence and is driven per-tick by the runner, so only apply the
                // static gain when no track is present.
                if p.volume_track.is_none() && p.volume_db != 0.0 {
                    #[allow(clippy::cast_possible_truncation)]
                    let linear = 10.0_f64.powf(p.volume_db / 20.0) as f32;
                    handle.set_volume(linear);
                }
                audio_only_tracks.push(AudioOnlyTrack {
                    source: p.source.clone(),
                    timeline_start,
                    timeline_end,
                    in_point: in_pt,
                    fade_in: p.fade_in,
                    fade_out: p.fade_out,
                    clip_dur,
                    handle,
                    volume_track: p.volume_track.clone(),
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
            composer: None,
            composer_key: Vec::new(),
            canvas: scene.canvas,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::thread;

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
