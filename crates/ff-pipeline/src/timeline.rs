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
    AnimatedValue, AnimationTrack, AudioTrack, FilterStep, MultiTrackAudioMixer,
    MultiTrackComposer, ProxySource, VideoLayer,
};
use ff_format::ChannelLayout;

use crate::clip::Clip;
use crate::encoder_config::EncoderConfig;
use crate::error::PipelineError;
use crate::pipeline::hwaccel_to_hardware_encoder;
use crate::progress::Progress;

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
/// use ff_pipeline::{Clip, Timeline};
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
    /// - [`PipelineError::ClipNotFound`] — a clip's source file is missing
    /// - [`PipelineError::Encode`] — encoder failure
    /// - [`PipelineError::Filter`] — filter graph construction failure
    /// - [`PipelineError::TimelineRenderFailed`] — other structural failure
    pub fn render(
        self,
        output: impl AsRef<Path>,
        config: EncoderConfig,
    ) -> Result<(), PipelineError> {
        self.render_with_progress(output, config, |_| true)
    }

    /// Renders the timeline to an output file, invoking `on_progress` after
    /// each encoded video frame.
    ///
    /// Animation tracks registered via [`TimelineBuilder::video_animation`] and
    /// [`TimelineBuilder::audio_animation`] are forwarded to the corresponding
    /// [`VideoLayer`] / [`AudioTrack`] fields before the filter graphs are built.
    /// Unrecognised animation keys are ignored and logged as `warn!`.
    ///
    /// `on_progress` receives a [`Progress`] reference after every video frame.
    /// Returning `false` cancels the render and returns
    /// [`PipelineError::Cancelled`]. Audio-only timelines do not invoke the
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
    /// - [`PipelineError::ClipNotFound`] — a clip's source file is missing
    /// - [`PipelineError::Cancelled`] — `on_progress` returned `false`
    /// - [`PipelineError::Encode`] — encoder failure
    /// - [`PipelineError::Filter`] — filter graph construction failure
    /// - [`PipelineError::TimelineRenderFailed`] — other structural failure
    pub fn render_with_progress(
        self,
        output: impl AsRef<Path>,
        config: EncoderConfig,
        on_progress: impl Fn(&Progress) -> bool + Send,
    ) -> Result<(), PipelineError> {
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
                    return Err(PipelineError::ClipNotFound {
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

        // Helper: look up a video-layer animated value by track index + property.
        let va = |track_idx: usize, prop: &str, default: f64| -> AnimatedValue<f64> {
            let key = format!("video_{track_idx}_{prop}");
            video_animations
                .get(&key)
                .cloned()
                .map_or(AnimatedValue::Static(default), AnimatedValue::Track)
        };

        // Helper: look up an audio-track animated value by track index + property.
        let aa = |track_idx: usize, prop: &str, default: f64| -> AnimatedValue<f64> {
            let key = format!("audio_{track_idx}_{prop}");
            audio_animations
                .get(&key)
                .cloned()
                .map_or(AnimatedValue::Static(default), AnimatedValue::Track)
        };

        // 3. Build video composition graph.
        let mut video_graph = None;
        if !video_tracks.is_empty() {
            // Per-track end-offset (seconds) of the last clip, used to compute
            // the xfade `offset` arg when the next clip has a transition.
            let mut prev_end_by_track: HashMap<usize, f64> = HashMap::new();

            let mut composer = MultiTrackComposer::new(canvas_width, canvas_height);
            for (track_idx, track) in video_tracks.iter().enumerate() {
                for clip in track {
                    // Wire xfade from the preceding clip on this track.
                    if let Some(kind) = clip.transition {
                        let dur_secs = clip.transition_duration.as_secs_f64();
                        let prev_end = *prev_end_by_track.get(&track_idx).unwrap_or(&0.0);
                        composer = composer.join_with_dissolve(prev_end, dur_secs, kind);
                    }

                    let mut layer_effects: Vec<FilterStep> = Vec::new();
                    if (clip.speed - 1.0).abs() > 1e-9 {
                        layer_effects.push(FilterStep::Speed { factor: clip.speed });
                    }
                    #[allow(clippy::float_cmp)]
                    let neutral =
                        clip.brightness == 0.0 && clip.contrast == 1.0 && clip.saturation == 1.0;
                    if !neutral {
                        layer_effects.push(FilterStep::Eq {
                            brightness: clip.brightness,
                            contrast: clip.contrast,
                            saturation: clip.saturation,
                        });
                    }

                    // Per-clip opacity overrides the track-level animation when not 1.0.
                    #[allow(clippy::float_cmp)]
                    let clip_opacity = if clip.opacity != 1.0 {
                        AnimatedValue::Static(f64::from(clip.opacity))
                    } else {
                        va(track_idx, "opacity", 1.0)
                    };

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

                    composer = composer.add_layer(VideoLayer {
                        source: clip.source.clone(),
                        proxy,
                        input_format: None,
                        x: va(track_idx, "x", 0.0),
                        y: va(track_idx, "y", 0.0),
                        scale_x: va(track_idx, "scale_x", 1.0),
                        scale_y: va(track_idx, "scale_y", 1.0),
                        rotation: va(track_idx, "rotation", 0.0),
                        opacity: clip_opacity,
                        blend_mode: clip.blend_mode,
                        z_order: u32::try_from(track_idx).unwrap_or(u32::MAX),
                        time_offset: clip.timeline_offset,
                        in_point: clip.in_point,
                        out_point: clip.out_point,
                        in_transition: None, // set by join_with_dissolve via add_layer
                        effects: layer_effects,
                    });

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
                use ff_filter::BlendMode;
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
                    z_order: u32::try_from(nv).unwrap_or(u32::MAX),
                    time_offset: Duration::ZERO,
                    in_point: None,
                    out_point: None,
                    in_transition: None,
                    effects: vec![],
                });
            }

            video_graph = Some(composer.build().map_err(PipelineError::Filter)?);
        }

        // 4. Build audio mix graph.
        let mut audio_graph = None;
        if !audio_tracks.is_empty() {
            let mut mixer = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo);
            for (track_idx, track) in audio_tracks.iter().enumerate() {
                for clip in track {
                    // Per-clip volume_db overrides the track-level animation when non-zero.
                    // This lets callers set independent gain on each clip without needing
                    // one audio track per clip.
                    let volume = if clip.volume_db == 0.0 {
                        aa(track_idx, "volume", 0.0)
                    } else {
                        AnimatedValue::Static(clip.volume_db)
                    };

                    let mut effects: Vec<FilterStep> = Vec::new();

                    if (clip.speed - 1.0).abs() > 1e-9 {
                        effects.push(FilterStep::Speed { factor: clip.speed });
                    }

                    if clip.fade_in > Duration::ZERO {
                        effects.push(FilterStep::AFadeIn {
                            start: 0.0,
                            duration: clip.fade_in.as_secs_f64(),
                        });
                    }

                    if clip.fade_out > Duration::ZERO {
                        // Resolve effective duration to compute the afade start offset.
                        let eff_dur: Option<Duration> = clip.duration().or_else(|| {
                            VideoDecoder::open(&clip.source).build().ok().map(|d| {
                                let total = d.duration();
                                match clip.in_point {
                                    Some(ip) => total.saturating_sub(ip),
                                    None => total,
                                }
                            })
                        });
                        match eff_dur {
                            Some(dur) if dur > clip.fade_out => {
                                // saturating_sub is safe: the guard ensures dur > fade_out.
                                let start = dur.saturating_sub(clip.fade_out).as_secs_f64();
                                effects.push(FilterStep::AFadeOut {
                                    start,
                                    duration: clip.fade_out.as_secs_f64(),
                                });
                            }
                            Some(_) => {
                                log::warn!(
                                    "fade_out ({:.3}s) >= clip duration — skipping fade_out \
                                     for {}",
                                    clip.fade_out.as_secs_f64(),
                                    clip.source.display(),
                                );
                            }
                            None => {
                                log::warn!(
                                    "cannot determine clip duration — skipping fade_out \
                                     for {}",
                                    clip.source.display(),
                                );
                            }
                        }
                    }

                    mixer = mixer.add_track(AudioTrack {
                        source: clip.source.clone(),
                        volume,
                        pan: aa(track_idx, "pan", 0.0),
                        time_offset: clip.timeline_offset,
                        in_point: clip.in_point,
                        out_point: clip.out_point,
                        effects,
                        sample_rate: 48_000,
                        channel_layout: ff_format::ChannelLayout::Stereo,
                    });
                }
            }
            audio_graph = Some(mixer.build().map_err(PipelineError::Filter)?);
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
        let mut encoder = enc_builder.build().map_err(PipelineError::Encode)?;

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
                match vgraph.pull_video().map_err(PipelineError::Filter)? {
                    Some(frame) => {
                        encoder.push_video(&frame).map_err(PipelineError::Encode)?;
                        video_idx = video_idx.saturating_add(1);
                        let progress = Progress {
                            frames_processed: u64::from(video_idx),
                            total_frames,
                            elapsed: start.elapsed(),
                        };
                        if !on_progress(&progress) {
                            return Err(PipelineError::Cancelled);
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
                match agraph.pull_audio().map_err(PipelineError::Filter)? {
                    Some(frame) => {
                        let chunk_dur = frame.duration();
                        encoder.push_audio(&frame).map_err(PipelineError::Encode)?;
                        audio_pts += chunk_dur;
                    }
                    None => break,
                }
            }
        }

        // 8. Flush encoder.
        encoder.finish().map_err(PipelineError::Encode)?;

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
    /// - [`PipelineError::NoInput`] — both track lists are empty
    /// - [`PipelineError::ClipNotFound`] — canvas/fps auto-probe needed but
    ///   the first video clip's source file does not exist
    /// - [`PipelineError::Decode`] — the first video clip could not be opened
    pub fn build(self) -> Result<Timeline, PipelineError> {
        if self.video_tracks.is_empty() && self.audio_tracks.is_empty() {
            return Err(PipelineError::NoInput);
        }

        let (canvas_width, canvas_height, frame_rate) = self.resolve_canvas_and_fps()?;

        Ok(Timeline {
            canvas_width,
            canvas_height,
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
    fn resolve_canvas_and_fps(&self) -> Result<(u32, u32, f64), PipelineError> {
        let need_probe = self.canvas_width.is_none()
            || self.canvas_height.is_none()
            || self.frame_rate.is_none();

        if need_probe && let Some(first_clip) = self.video_tracks.first().and_then(|t| t.first()) {
            if !first_clip.source.exists() {
                return Err(PipelineError::ClipNotFound {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn timeline_builder_should_err_when_no_tracks() {
        let result = Timeline::builder().build();
        assert!(matches!(result, Err(PipelineError::NoInput)));
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
