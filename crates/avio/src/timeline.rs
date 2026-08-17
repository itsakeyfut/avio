//! Timeline data type for multi-track composition.
//!
//! This module provides [`Timeline`] and [`TimelineBuilder`], which represent
//! an ordered layout of [`Clip`] instances across video and audio tracks.
//! `Timeline` holds no `FFmpeg` context; all rendering is done in
//! [`Timeline::render()`].

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use ff_decode::VideoDecoder;
use ff_encode::VideoEncoder;
use ff_filter::{
    AnimatedValue, AnimationTrack, MultiTrackAudioMixer, MultiTrackComposer, ProxySource,
    VideoLayer,
};
use ff_format::ChannelLayout;

use crate::clip::Clip;
use crate::derive;
use crate::error::TimelineError;
use ff_pipeline::EncoderConfig;
use ff_pipeline::Progress;
use ff_pipeline::pipeline::hwaccel_to_hardware_encoder;

/// An ordered layout of [`Clip`] instances across video and audio tracks.
///
/// `Timeline` is a plain Rust value type — it holds no `FFmpeg` context.
/// All rendering happens in [`Timeline::render()`].
///
/// # Construction
///
/// Use [`Timeline::builder()`] to obtain a [`TimelineBuilder`].
///
/// # Examples
///
/// ```
/// use avio::{Clip, Timeline};
/// use std::time::Duration;
///
/// let clip = Clip::new("intro.mp4")
///     .trim(Duration::from_secs(0), Duration::from_secs(5));
///
/// let result = Timeline::builder()
///     .canvas(1920, 1080)
///     .frame_rate(30.0)
///     .video_track(vec![clip])
///     .build();
///
/// assert!(result.is_ok());
/// ```
#[derive(Debug, Clone)]
pub struct Timeline {
    pub(crate) canvas_width: u32,
    pub(crate) canvas_height: u32,
    /// `true` when the caller set the canvas via [`TimelineBuilder::canvas`] (as
    /// opposed to it being auto-probed from the first clip). Lets consumers such
    /// as the real-time preview know a deliberate output aspect was requested.
    pub(crate) canvas_explicit: bool,
    pub(crate) frame_rate: f64,
    /// `video_tracks[track_idx][clip_idx]`; track 0 = bottom layer.
    pub(crate) video_tracks: Vec<Vec<Clip>>,
    pub(crate) audio_tracks: Vec<Vec<Clip>>,
    /// Animation tracks for video layer properties.
    ///
    /// Key format: `"video_{track_index}_{property}"`, e.g. `"video_0_opacity"`.
    ///
    /// Supported properties: `x`, `y`, `scale_x`, `scale_y`, `rotation`, `opacity`.
    pub(crate) video_animations: HashMap<String, AnimationTrack<f64>>,
    /// Animation tracks for audio track properties.
    ///
    /// Key format: `"audio_{track_index}_{property}"`, e.g. `"audio_1_volume"`.
    ///
    /// Supported properties: `volume`, `pan`.
    pub(crate) audio_animations: HashMap<String, AnimationTrack<f64>>,
    /// Optional `lavfi` filtergraph string composited as the topmost video layer.
    ///
    /// When set, a [`VideoLayer`] with `input_format = Some("lavfi")` is added above
    /// all regular video tracks. Use `FFmpeg` `drawtext` syntax to render text titles:
    ///
    /// ```text
    /// color=s=1920x1080:c=black@0.0,drawtext=text='Hello':fontsize=48:fontcolor=white
    /// ```
    pub(crate) lavfi_overlay: Option<String>,
}

impl Timeline {
    /// Returns a new [`TimelineBuilder`].
    pub fn builder() -> TimelineBuilder {
        TimelineBuilder::new()
    }

    /// Returns the canvas width in pixels.
    pub fn canvas_width(&self) -> u32 {
        self.canvas_width
    }

    /// Returns the canvas height in pixels.
    pub fn canvas_height(&self) -> u32 {
        self.canvas_height
    }

    /// Returns the canvas dimensions **only when explicitly set** via
    /// [`TimelineBuilder::canvas`], or `None` when they were auto-probed from the
    /// first clip. Consumers that reframe to a deliberate output aspect (e.g. the
    /// real-time preview) use this to distinguish an intended canvas from a default.
    pub fn explicit_canvas(&self) -> Option<(u32, u32)> {
        if self.canvas_explicit {
            Some((self.canvas_width, self.canvas_height))
        } else {
            None
        }
    }

    /// Returns the frame rate in frames per second.
    pub fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    /// Returns a slice of all video tracks.
    pub fn video_tracks(&self) -> &[Vec<Clip>] {
        &self.video_tracks
    }

    /// Returns a slice of all audio tracks.
    pub fn audio_tracks(&self) -> &[Vec<Clip>] {
        &self.audio_tracks
    }

    /// Renders the timeline to an output file.
    ///
    /// Convenience wrapper around [`render_with_progress`](Self::render_with_progress)
    /// that discards progress notifications.
    ///
    /// # Errors
    ///
    /// - [`TimelineError::ClipNotFound`] — a clip's source file is missing
    /// - [`TimelineError::Encode`] — encoder failure
    /// - [`TimelineError::Filter`] — filter graph construction failure
    /// - [`TimelineError::TimelineRenderFailed`] — other structural failure
    pub fn render(
        self,
        output: impl AsRef<Path>,
        config: EncoderConfig,
    ) -> Result<(), TimelineError> {
        self.render_with_progress(output, config, |_| true)
    }

    /// Renders the timeline to an output file, invoking `on_progress` after
    /// each encoded video frame.
    ///
    /// Animation tracks registered via [`TimelineBuilder::video_animation`] and
    /// [`TimelineBuilder::audio_animation`] are forwarded to the corresponding
    /// [`VideoLayer`] / [`AudioTrack`](ff_filter::AudioTrack) fields before the filter graphs are built.
    /// Unrecognised animation keys are ignored and logged as `warn!`.
    ///
    /// `on_progress` receives a [`Progress`] reference after every video frame.
    /// Returning `false` cancels the render and returns
    /// [`TimelineError::Cancelled`]. Audio-only timelines do not invoke the
    /// callback (there are no video frames to report).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let timeline = Timeline::builder()
    ///     .canvas(1920, 1080)
    ///     .frame_rate(30.0)
    ///     .video_track(vec![Clip::new("input.mp4")])
    ///     .build()?;
    ///
    /// timeline.render_with_progress("output.mp4", EncoderConfig::default(), |p| {
    ///     println!("frame {} / {:?}", p.frames_processed, p.total_frames);
    ///     true // return false to cancel
    /// })?;
    /// ```
    ///
    /// # Errors
    ///
    /// - [`TimelineError::ClipNotFound`] — a clip's source file is missing
    /// - [`TimelineError::Cancelled`] — `on_progress` returned `false`
    /// - [`TimelineError::Encode`] — encoder failure
    /// - [`TimelineError::Filter`] — filter graph construction failure
    /// - [`TimelineError::TimelineRenderFailed`] — other structural failure
    pub fn render_with_progress(
        self,
        output: impl AsRef<Path>,
        config: EncoderConfig,
        on_progress: impl Fn(&Progress) -> bool + Send,
    ) -> Result<(), TimelineError> {
        let output = output.as_ref();

        // Compute total expected video frame count from clips with known durations.
        // `None` when any clip runs to end-of-file (out_point not set).
        // Sum clip durations; short-circuits to None if any clip has no out_point.
        // frame_rate and total_dur are always non-negative; max(0.0) + round()
        // guarantees the value fits in u64 for any realistic frame count.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let total_frames: Option<u64> = self
            .video_tracks
            .iter()
            .flat_map(|track| track.iter())
            .map(Clip::duration)
            .try_fold(Duration::ZERO, |acc, dur| dur.map(|d| acc + d))
            .map(|total_dur| (total_dur.as_secs_f64() * self.frame_rate).round().max(0.0) as u64);

        let Timeline {
            canvas_width,
            canvas_height,
            canvas_explicit: _,
            frame_rate,
            video_tracks,
            audio_tracks,
            video_animations,
            audio_animations,
            lavfi_overlay,
        } = self;

        let nv = video_tracks.len();
        let na = audio_tracks.len();

        // 1. Pre-check: all clip sources must exist on disk.
        for track in video_tracks.iter().chain(audio_tracks.iter()) {
            for clip in track {
                if !clip.source.exists() {
                    return Err(TimelineError::ClipNotFound {
                        path: clip.source.to_string_lossy().into_owned(),
                    });
                }
            }
        }

        // 2. Warn on unrecognised animation keys.
        let valid_video_props = ["x", "y", "scale_x", "scale_y", "rotation", "opacity"];
        for key in video_animations.keys() {
            let parts: Vec<&str> = key.splitn(3, '_').collect();
            let ok = parts.len() == 3
                && parts[0] == "video"
                && parts[1].parse::<usize>().is_ok()
                && valid_video_props.contains(&parts[2]);
            if !ok {
                log::warn!("unknown animation key key={key}");
            }
        }

        let valid_audio_props = ["volume", "pan"];
        for key in audio_animations.keys() {
            let parts: Vec<&str> = key.splitn(3, '_').collect();
            let ok = parts.len() == 3
                && parts[0] == "audio"
                && parts[1].parse::<usize>().is_ok()
                && valid_audio_props.contains(&parts[2]);
            if !ok {
                log::warn!("unknown animation key key={key}");
            }
        }

        // 3. Build video composition graph.
        let mut video_graph = None;
        if !video_tracks.is_empty() {
            // Per-track end-offset (seconds) of the last clip, used to compute
            // the xfade `offset` arg when the next clip has a transition.
            let mut prev_end_by_track: HashMap<usize, f64> = HashMap::new();

            // Generate the canvas/conform at the timeline rate — the same rate
            // the encoder uses below. A hardcoded mismatch stretches the video
            // relative to the audio for non-30fps timelines.
            let mut composer =
                MultiTrackComposer::new(canvas_width, canvas_height).frame_rate(frame_rate);
            for (track_idx, track) in video_tracks.iter().enumerate() {
                for (clip_idx, clip) in track.iter().enumerate() {
                    let prev_end =
                        (clip_idx > 0).then(|| *prev_end_by_track.get(&track_idx).unwrap_or(&0.0));

                    // When a proxy is set, probe the original source resolution so
                    // the decoded proxy frames can be scaled back up to full size.
                    // If the probe fails the proxy is ignored (original used directly).
                    let proxy =
                        clip.proxy.as_ref().and_then(|proxy_path| {
                            match VideoDecoder::open(&clip.source).build() {
                                Ok(dec) => Some(ProxySource {
                                    path: proxy_path.clone(),
                                    width: dec.width(),
                                    height: dec.height(),
                                }),
                                Err(e) => {
                                    log::warn!(
                                        "proxy ignored: cannot probe source {} resolution: {e}",
                                        clip.source.display()
                                    );
                                    None
                                }
                            }
                        });

                    // Per-clip editorial interpretation → layer lives in `derive`.
                    composer = composer.add_layer(derive::video_layer(
                        clip,
                        track_idx,
                        &video_animations,
                        prev_end,
                        proxy,
                    ));

                    // Track how many seconds this clip contributes, so the next
                    // transition on the same track can compute the correct offset.
                    let end_secs = match clip.duration() {
                        Some(d) => d.as_secs_f64(),
                        None => VideoDecoder::open(&clip.source)
                            .build()
                            .ok()
                            .map_or(0.0, |d| {
                                let total = d.duration().as_secs_f64();
                                match clip.in_point {
                                    Some(ip) => (total - ip.as_secs_f64()).max(0.0),
                                    None => total,
                                }
                            }),
                    };
                    prev_end_by_track.insert(track_idx, end_secs);
                }
            }
            // Lavfi overlay sits above all regular tracks.
            if let Some(ref lavfi_str) = lavfi_overlay {
                use ff_filter::{BlendMode, CompositeOp};
                composer = composer.add_layer(VideoLayer {
                    source: std::path::PathBuf::from(lavfi_str),
                    proxy: None,
                    input_format: Some("lavfi".to_string()),
                    x: AnimatedValue::Static(0.0),
                    y: AnimatedValue::Static(0.0),
                    scale_x: AnimatedValue::Static(1.0),
                    scale_y: AnimatedValue::Static(1.0),
                    rotation: AnimatedValue::Static(0.0),
                    opacity: AnimatedValue::Static(1.0),
                    blend_mode: BlendMode::Normal,
                    composite_op: CompositeOp::Over,
                    effects: vec![],
                });
            }

            video_graph = Some(composer.build().map_err(TimelineError::Filter)?);
        }

        // 4. Build audio mix graph.
        let mut audio_graph = None;
        if !audio_tracks.is_empty() {
            let mut mixer = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo);
            for (track_idx, track) in audio_tracks.iter().enumerate() {
                for clip in track {
                    // Resolve the effective clip duration only when a fade-out
                    // needs it to compute its start offset (probing is I/O; the
                    // pure per-clip build lives in `derive`).
                    let fade_out_eff_dur = if clip.fade_out > Duration::ZERO {
                        clip.duration().or_else(|| {
                            VideoDecoder::open(&clip.source).build().ok().map(|d| {
                                let total = d.duration();
                                match clip.in_point {
                                    Some(ip) => total.saturating_sub(ip),
                                    None => total,
                                }
                            })
                        })
                    } else {
                        None
                    };
                    mixer = mixer.add_track(derive::audio_track(
                        clip,
                        track_idx,
                        &audio_animations,
                        fade_out_eff_dur,
                    ));
                }
            }
            audio_graph = Some(mixer.build().map_err(TimelineError::Filter)?);
        }

        // 5. Build encoder.
        let hw = hwaccel_to_hardware_encoder(config.hardware);
        let mut enc_builder = VideoEncoder::create(output)
            .video(canvas_width, canvas_height, frame_rate)
            .video_codec(config.video_codec)
            .bitrate_mode(config.bitrate_mode)
            .hardware_encoder(hw);
        if audio_graph.is_some() {
            enc_builder = enc_builder.audio(48_000, 2).audio_codec(config.audio_codec);
        }
        let mut encoder = enc_builder.build().map_err(TimelineError::Encode)?;

        let start = Instant::now();

        // 6. Drain video graph → encoder.
        //    tick() must be called before each pull so that animation entries
        //    registered on the graph update the filter parameters for that frame.
        //    on_progress is invoked after each push; returning false cancels.
        if let Some(mut vgraph) = video_graph {
            let mut video_idx: u32 = 0;
            loop {
                #[allow(clippy::cast_precision_loss)]
                // frame index fits comfortably in f64 mantissa
                let pts = Duration::from_secs_f64(f64::from(video_idx) / frame_rate);
                vgraph.tick(pts);
                match vgraph.pull_video().map_err(TimelineError::Filter)? {
                    Some(frame) => {
                        encoder.push_video(&frame).map_err(TimelineError::Encode)?;
                        video_idx = video_idx.saturating_add(1);
                        let progress = Progress {
                            frames_processed: u64::from(video_idx),
                            total_frames,
                            elapsed: start.elapsed(),
                        };
                        if !on_progress(&progress) {
                            return Err(TimelineError::Cancelled);
                        }
                    }
                    None => break,
                }
            }
        }

        // 7. Drain audio graph → encoder.
        //    tick() advances the audio animation clock by the actual duration
        //    of each chunk so PTS stays sample-accurate.
        if let Some(mut agraph) = audio_graph {
            let mut audio_pts = Duration::ZERO;
            loop {
                agraph.tick(audio_pts);
                match agraph.pull_audio().map_err(TimelineError::Filter)? {
                    Some(frame) => {
                        let chunk_dur = frame.duration();
                        encoder.push_audio(&frame).map_err(TimelineError::Encode)?;
                        audio_pts += chunk_dur;
                    }
                    None => break,
                }
            }
        }

        // 8. Flush encoder.
        encoder.finish().map_err(TimelineError::Encode)?;

        log::info!(
            "timeline render complete output={} video_tracks={nv} audio_tracks={na}",
            output.display()
        );
        Ok(())
    }
}

/// Builder for [`Timeline`].
///
/// Obtain one via [`Timeline::builder()`].
pub struct TimelineBuilder {
    canvas_width: Option<u32>,
    canvas_height: Option<u32>,
    frame_rate: Option<f64>,
    video_tracks: Vec<Vec<Clip>>,
    audio_tracks: Vec<Vec<Clip>>,
    video_animations: HashMap<String, AnimationTrack<f64>>,
    audio_animations: HashMap<String, AnimationTrack<f64>>,
    /// See [`TimelineBuilder::lavfi_overlay`].
    lavfi_overlay: Option<String>,
}

impl Default for TimelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TimelineBuilder {
    /// Creates a new builder with no tracks and no canvas/frame-rate set.
    pub fn new() -> Self {
        Self {
            canvas_width: None,
            canvas_height: None,
            frame_rate: None,
            video_tracks: Vec::new(),
            audio_tracks: Vec::new(),
            video_animations: HashMap::new(),
            audio_animations: HashMap::new(),
            lavfi_overlay: None,
        }
    }

    /// Sets the output canvas dimensions in pixels.
    #[must_use]
    pub fn canvas(self, width: u32, height: u32) -> Self {
        Self {
            canvas_width: Some(width),
            canvas_height: Some(height),
            ..self
        }
    }

    /// Sets the output frame rate in frames per second.
    #[must_use]
    pub fn frame_rate(self, fps: f64) -> Self {
        Self {
            frame_rate: Some(fps),
            ..self
        }
    }

    /// Appends a video track. Track 0 (first call) is the bottom layer.
    #[must_use]
    pub fn video_track(self, clips: Vec<Clip>) -> Self {
        let mut video_tracks = self.video_tracks;
        video_tracks.push(clips);
        Self {
            video_tracks,
            ..self
        }
    }

    /// Appends an audio track.
    #[must_use]
    pub fn audio_track(self, clips: Vec<Clip>) -> Self {
        let mut audio_tracks = self.audio_tracks;
        audio_tracks.push(clips);
        Self {
            audio_tracks,
            ..self
        }
    }

    /// Registers a video-layer animation track.
    ///
    /// Key format: `"video_{track_index}_{property}"`, e.g. `"video_0_opacity"`.
    ///
    /// Supported properties: `x`, `y`, `scale_x`, `scale_y`, `rotation`, `opacity`.
    /// Unrecognised keys are stored but emit `log::warn!` during [`Timeline::render()`].
    #[must_use]
    pub fn video_animation(self, key: impl Into<String>, track: AnimationTrack<f64>) -> Self {
        let mut video_animations = self.video_animations;
        video_animations.insert(key.into(), track);
        Self {
            video_animations,
            ..self
        }
    }

    /// Registers an audio-track animation track.
    ///
    /// Key format: `"audio_{track_index}_{property}"`, e.g. `"audio_0_volume"`.
    ///
    /// Supported properties: `volume`, `pan`.
    /// Unrecognised keys are stored but emit `log::warn!` during [`Timeline::render()`].
    #[must_use]
    pub fn audio_animation(self, key: impl Into<String>, track: AnimationTrack<f64>) -> Self {
        let mut audio_animations = self.audio_animations;
        audio_animations.insert(key.into(), track);
        Self {
            audio_animations,
            ..self
        }
    }

    /// Sets an `FFmpeg` `lavfi` filtergraph string that is composited as the topmost
    /// video layer during rendering.
    ///
    /// The string is interpreted by `FFmpeg`'s `lavfi` virtual demuxer via the `movie`
    /// filter's `format_name=lavfi` option. Use `drawtext` to render text titles, or
    /// chain multiple filter expressions with `,`:
    ///
    /// ```ignore
    /// builder.lavfi_overlay(
    ///     "color=s=1920x1080:c=black@0.0,\
    ///      drawtext=text='Hello World':fontsize=48:fontcolor=white:\
    ///      x=(w-text_w)/2:y=(h-text_h)/2"
    /// )
    /// ```
    ///
    /// When not set (the default) no overlay is added and the rendering path is unchanged.
    #[must_use]
    pub fn lavfi_overlay(self, filter: impl Into<String>) -> Self {
        Self {
            lavfi_overlay: Some(filter.into()),
            ..self
        }
    }

    /// Builds the [`Timeline`].
    ///
    /// # Errors
    ///
    /// - [`TimelineError::NoInput`] — both track lists are empty
    /// - [`TimelineError::ClipNotFound`] — canvas/fps auto-probe needed but
    ///   the first video clip's source file does not exist
    /// - [`TimelineError::Decode`] — the first video clip could not be opened
    pub fn build(self) -> Result<Timeline, TimelineError> {
        if self.video_tracks.is_empty() && self.audio_tracks.is_empty() {
            return Err(TimelineError::NoInput);
        }

        let canvas_explicit = self.canvas_width.is_some() && self.canvas_height.is_some();
        let (canvas_width, canvas_height, frame_rate) = self.resolve_canvas_and_fps()?;

        Ok(Timeline {
            canvas_width,
            canvas_height,
            canvas_explicit,
            frame_rate,
            video_tracks: self.video_tracks,
            audio_tracks: self.audio_tracks,
            video_animations: self.video_animations,
            audio_animations: self.audio_animations,
            lavfi_overlay: self.lavfi_overlay,
        })
    }

    /// Resolves canvas dimensions and frame rate.
    ///
    /// When all three values are explicitly set, returns them directly.
    /// Otherwise probes the first video clip with `VideoDecoder`. For
    /// audio-only timelines (no video tracks) falls back to 1920×1080 @ 30 fps.
    fn resolve_canvas_and_fps(&self) -> Result<(u32, u32, f64), TimelineError> {
        let need_probe = self.canvas_width.is_none()
            || self.canvas_height.is_none()
            || self.frame_rate.is_none();

        if need_probe && let Some(first_clip) = self.video_tracks.first().and_then(|t| t.first()) {
            if !first_clip.source.exists() {
                return Err(TimelineError::ClipNotFound {
                    path: first_clip.source.to_string_lossy().into_owned(),
                });
            }
            let vdec = VideoDecoder::open(&first_clip.source).build()?;
            let w = self.canvas_width.unwrap_or_else(|| vdec.width());
            let h = self.canvas_height.unwrap_or_else(|| vdec.height());
            let fps = self.frame_rate.unwrap_or_else(|| vdec.frame_rate());
            return Ok((w, h, fps));
        }

        // All values explicit, or no video tracks (audio-only) — fall back for absent values.
        Ok((
            self.canvas_width.unwrap_or(1920),
            self.canvas_height.unwrap_or(1080),
            self.frame_rate.unwrap_or(30.0),
        ))
    }
}

// ── Timeline -> Scene derivation (real-time preview) ────────────────────────────

/// Projects one video clip into a [`ScenePlacement`](ff_preview::ScenePlacement).
/// `is_base` selects the V1 base track, where a crossfade transition contributes a
/// `transition_dur`; overlays force zero (matching the compositor).
#[cfg(feature = "preview")]
fn video_placement(
    clip: &Clip,
    track_idx: usize,
    is_base: bool,
    animations: &HashMap<String, AnimationTrack<f64>>,
) -> ff_preview::ScenePlacement {
    let transition_dur = if is_base && clip.transition.is_some() {
        clip.transition_duration
    } else {
        Duration::ZERO
    };
    ff_preview::ScenePlacement {
        source: clip.source.clone(),
        timeline_offset: clip.timeline_offset,
        in_point: clip.in_point.unwrap_or(Duration::ZERO),
        out_point: clip.out_point,
        speed: clip.speed.max(0.01),
        transition_dur,
        opacity: clip.opacity.clamp(0.0, 1.0),
        // The single derive: preview and export build their video layers from the
        // same `avio::derive`, so the timeline-level animations (scale/rotation and
        // the opacity/x/y track-level fallbacks) reach the preview too.
        layer: crate::derive::realtime_descriptor(clip, track_idx, animations),
        fade_in: clip.fade_in,
        fade_out: clip.fade_out,
        volume_db: clip.volume_db,
        volume_track: clip.volume_track.clone(),
    }
}

/// Projects one audio-only clip into a [`SceneAudioPlacement`](ff_preview::SceneAudioPlacement).
#[cfg(feature = "preview")]
fn audio_placement(clip: &Clip) -> ff_preview::SceneAudioPlacement {
    ff_preview::SceneAudioPlacement {
        source: clip.source.clone(),
        timeline_offset: clip.timeline_offset,
        in_point: clip.in_point.unwrap_or(Duration::ZERO),
        out_point: clip.out_point,
        fade_in: clip.fade_in,
        fade_out: clip.fade_out,
        volume_db: clip.volume_db,
        volume_track: clip.volume_track.clone(),
    }
}

#[cfg(feature = "preview")]
impl Timeline {
    /// Projects this timeline into a primitive [`Scene`](ff_preview::Scene) for the
    /// real-time preview runner. This is a pure model projection — no probing or
    /// I/O; media-dependent resolution (durations, audio presence, frame size)
    /// happens later in [`ScenePlayer::open`](ff_preview::ScenePlayer::open).
    ///
    /// Video track `0` is the V1 base (crossfade transitions apply); tracks `1..`
    /// are overlays (transitions forced off, matching the compositor).
    #[must_use]
    pub fn to_scene(&self) -> ff_preview::Scene {
        let video_tracks = self
            .video_tracks()
            .iter()
            .enumerate()
            .map(|(track_idx, track)| ff_preview::SceneVideoTrack {
                placements: track
                    .iter()
                    .map(|clip| {
                        video_placement(clip, track_idx, track_idx == 0, &self.video_animations)
                    })
                    .collect(),
            })
            .collect();

        let audio_tracks = self
            .audio_tracks()
            .iter()
            .map(|track| ff_preview::SceneAudioTrack {
                placements: track.iter().map(audio_placement).collect(),
            })
            .collect();

        ff_preview::Scene {
            fps: self.frame_rate().max(1.0),
            canvas: self.explicit_canvas(),
            video_tracks,
            audio_tracks,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[cfg(feature = "preview")]
    #[test]
    fn timeline_to_scene_should_project_clip_fields() {
        use ff_filter::XfadeTransition;

        let timeline = Timeline::builder()
            // Explicit canvas + fps so build() does not probe the fake sources.
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("a.mp4")
                    .trim(Duration::from_secs(1), Duration::from_secs(3))
                    .offset(Duration::from_millis(500))
                    .with_opacity(0.5)
                    .with_speed(2.0)
                    .with_volume_track(AnimationTrack::new()),
                Clip::new("b.mp4")
                    .with_transition(XfadeTransition::Fade, Duration::from_millis(750)),
            ])
            .video_track(vec![
                // Overlay: a transition here must project as zero.
                Clip::new("overlay.mp4")
                    .with_transition(XfadeTransition::Fade, Duration::from_millis(400)),
            ])
            .audio_track(vec![
                Clip::new("music.mp3")
                    .with_fade_in(Duration::from_millis(200))
                    .with_fade_out(Duration::from_millis(300))
                    .volume(-6.0),
            ])
            .build()
            .unwrap();

        let scene = timeline.to_scene();

        assert!((scene.fps - 30.0).abs() < f64::EPSILON);
        assert_eq!(scene.canvas, Some((1920, 1080)));
        assert_eq!(scene.video_tracks.len(), 2);
        assert_eq!(scene.audio_tracks.len(), 1);

        let base = &scene.video_tracks[0].placements[0];
        assert_eq!(base.source.to_str(), Some("a.mp4"));
        assert_eq!(base.timeline_offset, Duration::from_millis(500));
        assert_eq!(base.in_point, Duration::from_secs(1));
        assert_eq!(base.out_point, Some(Duration::from_secs(3)));
        assert!((base.speed - 2.0).abs() < f64::EPSILON);
        assert!((base.opacity - 0.5).abs() < f32::EPSILON);
        assert_eq!(
            base.transition_dur,
            Duration::ZERO,
            "clip 0 has no transition"
        );
        assert!(base.volume_track.is_some());
        assert!(
            matches!(base.layer.opacity, AnimatedValue::Static(v) if (v - 0.5).abs() < f64::EPSILON)
        );

        let base1 = &scene.video_tracks[0].placements[1];
        assert_eq!(base1.transition_dur, Duration::from_millis(750));

        let overlay = &scene.video_tracks[1].placements[0];
        assert_eq!(
            overlay.transition_dur,
            Duration::ZERO,
            "overlay transitions must project as zero"
        );

        let audio = &scene.audio_tracks[0].placements[0];
        assert_eq!(audio.source.to_str(), Some("music.mp3"));
        assert_eq!(audio.fade_in, Duration::from_millis(200));
        assert_eq!(audio.fade_out, Duration::from_millis(300));
        assert!((audio.volume_db - (-6.0)).abs() < f64::EPSILON);
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_route_timeline_animations_into_the_preview_layer() {
        use ff_filter::{AnimatedValue, Easing, Keyframe};

        // Track 1 (overlay) gets timeline scale_x + rotation animations, and an opacity
        // animation the neutral clip should fall back to (the 3-way merge). The single
        // derive must route all three into the preview placement's layer.
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("base.mp4")])
            .video_track(vec![Clip::new("overlay.mp4")])
            .video_animation(
                "video_1_scale_x",
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
            )
            .video_animation(
                "video_1_rotation",
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 45.0, Easing::Linear)),
            )
            .video_animation(
                "video_1_opacity",
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 1.0, Easing::Linear)),
            )
            .build()
            .unwrap();

        let scene = timeline.to_scene();
        let overlay = &scene.video_tracks[1].placements[0];
        assert!(matches!(overlay.layer.scale_x, AnimatedValue::Track(_)));
        assert!(matches!(overlay.layer.rotation, AnimatedValue::Track(_)));
        assert!(matches!(overlay.layer.opacity, AnimatedValue::Track(_)));
    }

    #[test]
    fn timeline_builder_should_err_when_no_tracks() {
        let result = Timeline::builder().build();
        assert!(matches!(result, Err(TimelineError::NoInput)));
    }

    #[test]
    fn timeline_builder_should_succeed_with_video_track() {
        let clip = Clip::new("video.mp4");
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![clip])
            .build()
            .unwrap();

        assert_eq!(timeline.canvas_width, 1920);
        assert_eq!(timeline.canvas_height, 1080);
        assert!((timeline.frame_rate - 30.0).abs() < f64::EPSILON);
        assert_eq!(timeline.video_tracks.len(), 1);
        assert!(timeline.audio_tracks.is_empty());
    }

    #[test]
    fn timeline_builder_should_store_video_animation_track() {
        use ff_filter::{AnimationTrack, Easing, Keyframe};
        use std::time::Duration;

        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 1.0_f64, Easing::Linear))
            .push(Keyframe::new(
                Duration::from_secs(2),
                0.0_f64,
                Easing::Linear,
            ));

        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("video.mp4")])
            .video_animation("video_0_opacity", track)
            .build()
            .unwrap();

        assert_eq!(timeline.video_animations.len(), 1);
        assert!(timeline.video_animations.contains_key("video_0_opacity"));
    }

    #[test]
    fn timeline_builder_should_store_audio_animation_track() {
        use ff_filter::{AnimationTrack, Easing, Keyframe};
        use std::time::Duration;

        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.0_f64, Easing::Linear))
            .push(Keyframe::new(
                Duration::from_secs(2),
                -6.0_f64,
                Easing::Linear,
            ));

        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .audio_track(vec![Clip::new("audio.mp4")])
            .audio_animation("audio_0_volume", track)
            .build()
            .unwrap();

        assert_eq!(timeline.audio_animations.len(), 1);
        assert!(timeline.audio_animations.contains_key("audio_0_volume"));
    }
}
