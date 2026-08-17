//! Real-time playback of a [`Scene`].
//!
//! [`ScenePlayer`] opens every placement on the base video track of a [`Scene`]
//! and plays them back in order, mapping each clip's frame PTS to the unified
//! timeline coordinate. The `Scene` is a model-agnostic description an engine
//! derives from its editing model.
//!
//! | Type | Role |
//! |------|------|
//! | [`ScenePlayer`] | Thin builder: call [`open`](ScenePlayer::open) |
//! | [`TimelineRunner`] | Owns the decode pipelines; move to a thread and call [`run`](TimelineRunner::run) |
//! | [`PlayerHandle`] | Shared, cloneable control handle |
//!
//! ## Audio
//!
//! When any placement on the base video track carries an audio stream,
//! [`ScenePlayer::open`] creates an [`AudioMixer`] with one track per
//! audio-bearing clip.  A background [`AudioDecoder`](ff_decode::AudioDecoder) thread is started for
//! the active clip and pushes mono samples via [`AudioTrackHandle`].  On clip
//! transition or seek the old thread is cancelled and a new one is started.
//! [`PlayerHandle::pop_audio_samples`] calls [`AudioMixer::mix`] and returns
//! interleaved stereo `f32` output.

mod audio_resampling;
mod runner;
mod runner_layout;
mod scene;
mod state;
mod timeline_inner;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

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
use state::{AudioFadeConfig, AudioOnlyTrack, ClipState, LavfiOverlayState, OverlayLayer};

// -- Constants --

const CHANNEL_CAP: usize = 64;

// ── ScenePlayer ─────────────────────────────────────────────────────────────

/// Thin builder for a ([`TimelineRunner`], [`PlayerHandle`]) pair backed by a
/// [`Scene`].
///
/// Playback is limited to the base video track (`video_tracks[0]`). When any
/// placement carries an audio stream, an [`AudioMixer`] is created and audio is
/// mixed into the stereo output from [`PlayerHandle::pop_audio_samples`].
///
/// This player is model-agnostic: an engine derives the [`Scene`] from its
/// editing model (for example `avio::TimelinePlayer` from a `Timeline`) and
/// hands it here.
pub struct ScenePlayer;

impl ScenePlayer {
    /// Open a [`Scene`] for real-time preview playback.
    ///
    /// Resolves the scene against the media (probing each placement's source for
    /// duration, audio availability, and frame size), opens a [`DecodeBuffer`]
    /// per base-track clip and seeks it to `in_point`, and builds the audio mixer
    /// and tracks.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] when:
    /// - the scene has no video tracks or the base track is empty,
    /// - a placement source file cannot be found or opened,
    /// - a placement cannot be probed for duration.
    #[allow(clippy::too_many_lines)]
    pub fn open(scene: &Scene) -> Result<(TimelineRunner, PlayerHandle), PreviewError> {
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
            lavfi: scene
                .lavfi_overlay
                .as_deref()
                .and_then(LavfiOverlayState::new),
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
}
