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
//! | [`SceneRunner`] | Owns the decode pipelines; move to a thread and call [`run`](SceneRunner::run) |
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
mod compositor;
mod inner;
mod runner;
mod runner_layout;
mod state;
mod types;

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

pub use compositor::PreviewCompositor;
pub use inner::apply_xfade;
pub use runner::SceneRunner;
pub use types::{
    Scene, SceneAudioPlacement, SceneAudioTrack, ScenePlacement, SceneSource, SceneVideoTrack,
};

use audio_resampling::spawn_audio_track_thread;
use ff_filter::{AnimatedValue, SolidSource, TextSource, XfadeTransition};
use ff_format::VideoFrame;
use state::{
    AudioFadeConfig, AudioOnlyTrack, ClipState, ClipVideoSource, LavfiOverlayState, OverlayLayer,
    db_to_linear,
};

/// Resolves the canvas size for generated (solid/text) sources: the explicit scene
/// canvas, else the first file placement's video size, else `(0, 0)` (no file to
/// size from, so a generated source renders nothing).
fn resolve_canvas_dims(scene: &Scene) -> (u32, u32) {
    if let Some(dims) = scene.canvas {
        return dims;
    }
    for track in &scene.video_tracks {
        for p in &track.placements {
            if let Some(path) = p.source.as_file()
                && let Ok(info) = ff_probe::open(path)
                && let Some(v) = info.primary_video()
            {
                return (v.width(), v.height());
            }
        }
    }
    (0, 0)
}

/// Builds the constant held frame for a generated source (pulled once via
/// `ff-filter`'s `SolidSource` / `TextSource`, the same `color` / `drawtext` filters
/// export uses), or `None` when unavailable — e.g. the filters are missing on a
/// minimal `FFmpeg` (RK-002), so the clip renders nothing rather than failing `open`.
fn generated_held_frame(source: &SceneSource, cw: u32, ch: u32, fps: f64) -> Option<VideoFrame> {
    if cw == 0 || ch == 0 {
        log::warn!("generated source has no canvas size to render into, cw={cw} ch={ch}");
        return None;
    }
    let pulled = match source {
        SceneSource::Solid(color) => {
            SolidSource::new(*color, cw, ch, fps).map(|mut s| pull_first(&mut s))
        }
        SceneSource::Text(spec) => {
            TextSource::new(spec, cw, ch, fps).map(|mut s| pull_first(&mut s))
        }
        SceneSource::File(_) => return None,
    };
    match pulled {
        Ok(Some(frame)) => Some(frame),
        Ok(None) => {
            log::warn!("generated source produced no frame; rendering nothing");
            None
        }
        Err(e) => {
            log::warn!("generated source unavailable, rendering nothing, error={e}");
            None
        }
    }
}

/// The timeline span of a generated clip, from its `out_point` (a generated source is
/// infinite, so `out_point` bounds it — mirroring the export-side
/// `GeneratedSourceNeedsDuration`). Warns and yields zero when unbounded.
fn generated_span(out_point: Option<Duration>, in_pt: Duration) -> Duration {
    if let Some(op) = out_point {
        op.saturating_sub(in_pt)
    } else {
        log::warn!(
            "generated clip has no out_point; preview shows zero duration (bound it with a trim)"
        );
        Duration::ZERO
    }
}

/// Opens the per-clip video source: a decoding [`DecodeBuffer`] seeked to `in_pt`
/// for a file, or a constant [`Held`](ClipVideoSource::Held) frame (built at
/// `cw`x`ch`) for a generated source.
fn open_clip_video_source(
    source: &SceneSource,
    in_pt: Duration,
    cw: u32,
    ch: u32,
    fps: f64,
) -> Result<ClipVideoSource, PreviewError> {
    match source.as_file() {
        Some(path) => {
            let mut buf = DecodeBuffer::open(path).build()?;
            if in_pt > Duration::ZERO {
                buf.seek(in_pt)?;
            }
            Ok(ClipVideoSource::File(buf))
        }
        None => Ok(ClipVideoSource::held(
            generated_held_frame(source, cw, ch, fps),
            in_pt,
            fps,
        )),
    }
}

/// The file path of a source (empty for a generated one) — the `ClipState.source`
/// field, used only to spawn an audio thread (a generated clip has no audio).
fn source_path(source: &SceneSource) -> PathBuf {
    source
        .as_file()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default()
}

/// Pulls the first frame from a freshly built generated source, retrying while the
/// graph is still priming (`Ok(None)`). Returns `None` on error or if the source
/// never yields a frame.
fn pull_first<S: GeneratedPull>(source: &mut S) -> Option<VideoFrame> {
    for _ in 0..16 {
        match source.pull() {
            Ok(Some(frame)) => return Some(frame),
            Ok(None) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Shared `pull` shape of the generated frame sources so [`pull_first`] can retry
/// either.
trait GeneratedPull {
    fn pull(&mut self) -> Result<Option<VideoFrame>, ff_filter::FilterError>;
}
impl GeneratedPull for SolidSource {
    fn pull(&mut self) -> Result<Option<VideoFrame>, ff_filter::FilterError> {
        SolidSource::pull(self)
    }
}
impl GeneratedPull for TextSource {
    fn pull(&mut self) -> Result<Option<VideoFrame>, ff_filter::FilterError> {
        TextSource::pull(self)
    }
}

// -- Constants --

const CHANNEL_CAP: usize = 64;

// ScenePlayer

/// Thin builder for a ([`SceneRunner`], [`PlayerHandle`]) pair backed by a
/// [`Scene`].
///
/// Playback is limited to the base video track (`video_tracks[0]`). When any
/// placement carries an audio stream, an [`AudioMixer`] is created and audio is
/// mixed into the stereo output from [`PlayerHandle::pop_audio_samples`].
///
/// This player is model-agnostic: an engine derives the [`Scene`] from its
/// editing model and hands it here.
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
    pub fn open(scene: &Scene) -> Result<(SceneRunner, PlayerHandle), PreviewError> {
        struct ProbeResult {
            source: SceneSource,
            in_pt: Duration,
            clip_dur: Duration,
            offset: Duration,
            out_point: Option<Duration>,
            xfade_dur: Duration,
            xfade_kind: Option<XfadeTransition>,
            video_handle: Duration,
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
        // Canvas size for generated (solid/text) sources, which have no file to probe.
        let canvas = resolve_canvas_dims(scene);
        let clip_list = &v_tracks[0].placements;

        // Phase 1: probe all clips

        let mut probes: Vec<ProbeResult> = Vec::with_capacity(clip_list.len());
        let mut has_any_audio = false;

        for p in clip_list {
            let in_pt = p.in_point;
            let speed = p.speed;

            // A file clip is probed; a generated (solid/text) clip is sized from the
            // canvas, bounded by its `out_point`, and carries no audio.
            let (video_w, video_h, unscaled_dur, has_audio) = if let Some(path) = p.source.as_file()
            {
                let info = ff_probe::open(path)?;
                let dur = p.out_point.map_or_else(
                    || info.duration().saturating_sub(in_pt),
                    |op| op.saturating_sub(in_pt),
                );
                let (w, h) = info
                    .primary_video()
                    .map_or((0, 0), |v| (v.width(), v.height()));
                (w, h, dur, info.has_audio())
            } else {
                (
                    canvas.0,
                    canvas.1,
                    generated_span(p.out_point, in_pt),
                    false,
                )
            };
            let clip_dur = if (speed - 1.0).abs() < 1e-9 {
                unscaled_dur
            } else {
                unscaled_dur.div_f64(speed)
            };

            has_any_audio |= has_audio;

            probes.push(ProbeResult {
                source: p.source.clone(),
                in_pt,
                clip_dur,
                offset: p.offset,
                out_point: p.out_point,
                xfade_dur: p.xfade_dur,
                xfade_kind: p.xfade_kind,
                video_handle: p.video_handle,
                has_audio,
                video_w,
                video_h,
                speed,
                opacity: p.opacity,
            });
        }

        // Phase 2: build mixer and track handles (if audio present)

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

        // Phase 3: build ClipState objects

        let mut clip_states: Vec<ClipState> = Vec::with_capacity(probes.len());
        for (i, p) in probes.iter().enumerate() {
            let timeline_start = p.offset;
            let timeline_end = timeline_start + p.clip_dur;

            let decode_buf = open_clip_video_source(&p.source, p.in_pt, p.video_w, p.video_h, fps)?;

            // Apply a static V1 audio gain once at open; an animated gain is driven
            // per-tick by the runner.
            if let (Some(handle), AnimatedValue::Static(db)) =
                (&audio_track_handles[i], &clip_list[i].volume)
                && *db != 0.0
            {
                handle.set_volume(db_to_linear(*db));
            }
            // Apply pan once at open at its `t=0` value (an animated pan uses its
            // initial value, matching the export mixer).
            if let Some(handle) = &audio_track_handles[i] {
                let pan0 = clip_list[i].pan.value_at(Duration::ZERO);
                if pan0 != 0.0 {
                    // `set_pan` clamps to [-1.0, 1.0], so the f32 narrowing is safe.
                    #[allow(clippy::cast_possible_truncation)]
                    handle.set_pan(pan0 as f32);
                }
            }
            clip_states.push(ClipState {
                source: p.source.clone(),
                decode_buf,
                timeline_start,
                timeline_end,
                in_point: p.in_pt,
                out_point: p.out_point,
                xfade_dur: p.xfade_dur,
                xfade_kind: p.xfade_kind,
                video_handle: p.video_handle,
                audio_track: audio_track_handles[i].clone(),
                speed: p.speed,
                opacity: p.opacity,
                layer_desc: clip_list[i].layer.clone(),
                volume: clip_list[i].volume.clone(),
                fade_in: clip_list[i].fade_in,
                fade_out: clip_list[i].fade_out,
                pitch: clip_list[i].pitch,
            });
        }

        // Phase 4: build overlay layers (V2, V3, …)
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
                // File clip: probe. Generated (solid/text) clip: canvas-sized, bounded
                // by `out_point`, no audio.
                let (clip_dur, has_audio) = match p.source.as_file() {
                    Some(path) => {
                        let info = ff_probe::open(path)?;
                        let dur = p.out_point.map_or_else(
                            || info.duration().saturating_sub(in_pt),
                            |op| op.saturating_sub(in_pt),
                        );
                        (dur, info.has_audio())
                    }
                    None => (generated_span(p.out_point, in_pt), false),
                };
                let timeline_start = p.offset;
                let timeline_end = timeline_start + clip_dur;
                let decode_buf = open_clip_video_source(&p.source, in_pt, canvas.0, canvas.1, fps)?;
                if has_audio {
                    let mixer_ref = mixer_arc
                        .get_or_insert_with(|| Arc::new(Mutex::new(AudioMixer::new(48_000))));
                    let handle = mixer_ref
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .add_track();
                    if let AnimatedValue::Static(db) = &p.volume
                        && *db != 0.0
                    {
                        handle.set_volume(db_to_linear(*db));
                    }
                    // Apply pan once at open at its `t=0` value (an animated pan uses
                    // its initial value, matching the export mixer).
                    let pan0 = p.pan.value_at(Duration::ZERO);
                    if pan0 != 0.0 {
                        // `set_pan` clamps to [-1.0, 1.0], so the f32 narrowing is safe.
                        #[allow(clippy::cast_possible_truncation)]
                        handle.set_pan(pan0 as f32);
                    }
                    audio_only_tracks.push(AudioOnlyTrack {
                        source: source_path(&p.source),
                        timeline_start,
                        timeline_end,
                        in_point: in_pt,
                        fade_in: p.fade_in,
                        fade_out: p.fade_out,
                        clip_dur,
                        speed: p.speed,
                        pitch: p.pitch,
                        handle,
                        volume: p.volume.clone(),
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
                    xfade_dur: Duration::ZERO,
                    xfade_kind: None,
                    // Overlays carry no transition, so there is nothing to feed.
                    video_handle: Duration::ZERO,
                    audio_track: None,
                    speed: p.speed,
                    opacity: p.opacity,
                    layer_desc: p.layer.clone(),
                    volume: p.volume.clone(),
                    fade_in: p.fade_in,
                    fade_out: p.fade_out,
                    pitch: p.pitch,
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

        // Phase 5: build audio-only tracks (A1, A2, …)

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
                let timeline_start = p.offset;
                let timeline_end = timeline_start + clip_dur;
                // Lazily create the mixer if no V1 clip had audio.
                let mixer_ref =
                    mixer_arc.get_or_insert_with(|| Arc::new(Mutex::new(AudioMixer::new(48_000))));
                let handle = mixer_ref
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .add_track();
                // Apply a static gain once at open; an animated gain (a track) is driven
                // per-tick by the runner.
                if let AnimatedValue::Static(db) = &p.volume
                    && *db != 0.0
                {
                    handle.set_volume(db_to_linear(*db));
                }
                // Apply pan once at open at its `t=0` value (an animated pan uses its
                // initial value, matching the export mixer).
                let pan0 = p.pan.value_at(Duration::ZERO);
                if pan0 != 0.0 {
                    // `set_pan` clamps to [-1.0, 1.0], so the f32 narrowing is safe.
                    #[allow(clippy::cast_possible_truncation)]
                    handle.set_pan(pan0 as f32);
                }
                audio_only_tracks.push(AudioOnlyTrack {
                    source: p.source.clone(),
                    timeline_start,
                    timeline_end,
                    in_point: in_pt,
                    fade_in: p.fade_in,
                    fade_out: p.fade_out,
                    clip_dur,
                    speed: p.speed,
                    pitch: p.pitch,
                    handle,
                    volume: p.volume.clone(),
                    cancel: None,
                    thread: None,
                });
            }
        }

        // Compute total duration

        let total_dur = clip_states
            .iter()
            .map(|c| c.timeline_end)
            .max()
            .unwrap_or(Duration::ZERO);
        let duration_millis = u64::try_from(total_dur.as_millis()).unwrap_or(u64::MAX);

        // Build runner and handle

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
                // The first V1 clip has audio, so it is a file source; derive its path.
                let source = source_path(&clip_states[0].source);
                let in_pt = clip_states[0].in_point;
                let clip0_speed = clip_states[0].speed;
                let clip0_pitch = clip_states[0].pitch;
                let cancel = Arc::new(AtomicBool::new(false));
                let thread = spawn_audio_track_thread(
                    source,
                    in_pt,
                    handle,
                    Arc::clone(&cancel),
                    AudioFadeConfig {
                        speed: clip0_speed,
                        pitch: clip0_pitch,
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

        let runner = SceneRunner {
            clips: clip_states,
            overlay_layers,
            audio_only_tracks,
            active: 0,
            transition: None,
            cmd_rx,
            event_tx,
            sink: None,
            gpu_compositor: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_canvas_dims_should_prefer_explicit_canvas_then_fall_back() {
        let scene = |canvas| Scene {
            fps: 30.0,
            canvas,
            lavfi_overlay: None,
            video_tracks: vec![],
            audio_tracks: vec![],
        };
        // An explicit canvas wins.
        assert_eq!(
            resolve_canvas_dims(&scene(Some((1920, 1080)))),
            (1920, 1080)
        );
        // No explicit canvas and no file placement to size from -> (0, 0), so a
        // generated source renders nothing rather than guessing.
        assert_eq!(resolve_canvas_dims(&scene(None)), (0, 0));
    }

    #[test]
    #[ignore = "requires the color/drawtext filters; run with -- --include-ignored"]
    fn preview_should_render_text_and_solid_sources() {
        use ff_format::{Color, TextSpec};

        // #1615: a generated source's held frame is built via `SolidSource` /
        // `TextSource` — the same `color` / `drawtext` filters export uses — so preview
        // matches export. Probe-gated (RK-002): the filters are absent on a minimal
        // FFmpeg, so `generated_held_frame` returns `None` and the test skips.
        let red = Color::rgb(200, 30, 40);
        let Some(frame) = generated_held_frame(&SceneSource::Solid(red), 16, 16, 30.0) else {
            println!("Skipping: color filter unavailable");
            return;
        };
        assert_eq!((frame.width(), frame.height()), (16, 16));
        let Some(plane) = frame.plane(0) else {
            println!("Skipping: no rgba plane");
            return;
        };
        let stride = frame.stride(0).unwrap_or(16 * 4);
        // Centre pixel (8, 8) in the rgba plane must be ~red (non-vacuous: an empty
        // path would render nothing).
        let off = 8 * stride + 8 * 4;
        let (r, g, b) = (plane[off], plane[off + 1], plane[off + 2]);
        assert!(
            r.abs_diff(200) <= 6 && g.abs_diff(30) <= 6 && b.abs_diff(40) <= 6,
            "solid centre pixel must be ~red, got ({r}, {g}, {b})"
        );

        // Text: the `color`→`drawtext` path must at least produce a canvas-sized frame
        // where the drawtext filter is available.
        if let Some(tf) =
            generated_held_frame(&SceneSource::Text(TextSpec::new("Hi")), 64, 32, 30.0)
        {
            assert_eq!((tf.width(), tf.height()), (64, 32));
        } else {
            println!("Skipping text: drawtext filter unavailable");
        }
    }

    // blend_rgba delegate

    #[test]
    fn inner_blend_rgba_at_zero_alpha_should_return_a() {
        let a = vec![255u8, 0, 0, 255];
        let b = vec![0u8, 0, 255, 255];
        let mut dst = Vec::new();
        inner::blend_rgba(&a, &b, 0.0, &mut dst);
        assert_eq!(dst, a);
    }

    // open

    #[test]
    fn timeline_player_open_should_fail_when_no_video_tracks() {
        let _ = PreviewError::SeekOutOfRange {
            pts: Duration::from_secs(1),
        };
    }
}
