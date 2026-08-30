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
    AnimatedValue, AnimationTrack, FilterGraph, FilterStep, MultiTrackAudioMixer,
    MultiTrackComposer, ProxySource, VideoLayer,
};
use ff_format::{AudioFrame, ChannelLayout};

use crate::clip::Clip;
use crate::derive;
use crate::error::TimelineError;
use crate::ids::{ClipId, EffectId, TrackId};
use crate::marker::Marker;
use crate::track::{AudioProperty, Track, VideoProperty};
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Timeline {
    pub(crate) canvas_width: u32,
    pub(crate) canvas_height: u32,
    /// `true` when the caller set the canvas via [`TimelineBuilder::canvas`] (as
    /// opposed to it being auto-probed from the first clip). Lets consumers such
    /// as the real-time preview know a deliberate output aspect was requested.
    pub(crate) canvas_explicit: bool,
    pub(crate) frame_rate: f64,
    /// `video_tracks[track_idx].clips[clip_idx]`; track 0 = bottom layer.
    pub(crate) video_tracks: Vec<Track>,
    pub(crate) audio_tracks: Vec<Track>,
    /// Next [`ClipId`](crate::ClipId) value to hand out. Monotonic, never reused;
    /// stamped onto clips as they are added. `1` for a fresh document (`0` = unset).
    pub(crate) next_clip_id: u64,
    /// Next [`TrackId`](crate::TrackId) value to hand out (see `next_clip_id`).
    pub(crate) next_track_id: u64,
    /// Next [`MarkerId`](crate::MarkerId) value to hand out (see `next_clip_id`).
    pub(crate) next_marker_id: u64,
    /// Next [`GroupId`](crate::GroupId) value to hand out (see `next_clip_id`).
    pub(crate) next_group_id: u64,
    /// Next [`EffectId`](crate::EffectId) value to hand out (see `next_clip_id`).
    /// Document-wide: effect ids are unique across all clips.
    pub(crate) next_effect_id: u64,
    /// Editorial markers on the timeline. Metadata only — they do not affect
    /// derivation, render, or preview. Addressed by [`MarkerId`](crate::MarkerId).
    pub(crate) markers: Vec<Marker>,
    /// Optional `lavfi` filtergraph string composited as the topmost video layer.
    ///
    /// When set, a [`VideoLayer`] whose source is
    /// [`LayerSource::Lavfi`](ff_filter::LayerSource) is added above all regular
    /// video tracks. Use `FFmpeg` `drawtext` syntax to render text titles:
    ///
    /// ```text
    /// color=s=1920x1080:c=black@0.0,drawtext=text='Hello':fontsize=48:fontcolor=white
    /// ```
    pub(crate) lavfi_overlay: Option<String>,
    /// Timeline-level (master bus) audio effect chain applied to the final mix on
    /// render. Empty by default (no processing).
    ///
    /// Applied after the multi-track mix, so it operates on the whole program's
    /// audio — the natural place for loudness normalization
    /// ([`FilterStep::LoudnessNormalize`]). Per-track (pre-mix) effects are a
    /// separate feature (see issue #1446).
    ///
    /// Persisted by the `serde` feature (#1452). Compositor-internal steps
    /// (`Blend` / `Composite` / `AlphaMatte`) are not serialized.
    pub(crate) audio_filter: Vec<FilterStep>,
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
    pub fn video_tracks(&self) -> &[Track] {
        &self.video_tracks
    }

    /// Returns a slice of all audio tracks.
    pub fn audio_tracks(&self) -> &[Track] {
        &self.audio_tracks
    }

    /// Returns the timeline's editorial markers.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
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
        self.render_inner(output, config, |_| true, false)
    }

    /// Like [`render`](Self::render) but forces the CPU `MultiTrackComposer` export
    /// path, bypassing the GPU export even when an adapter is available.
    ///
    /// This is the force-CPU override of the GPU-default export; the output is
    /// identical in shape to [`render`](Self::render). Without the `gpu` feature it
    /// is behaviourally identical to [`render`](Self::render).
    ///
    /// # Errors
    ///
    /// Same as [`render`](Self::render).
    pub fn render_forcing_cpu(
        self,
        output: impl AsRef<Path>,
        config: EncoderConfig,
    ) -> Result<(), TimelineError> {
        self.render_inner(output, config, |_| true, true)
    }

    /// Renders the timeline to an output file, invoking `on_progress` after
    /// each encoded video frame.
    ///
    /// Track-level automation (see [`Track::automation`](crate::Track::automation),
    /// set via [`TimelineBuilder::video_animation`] / [`TimelineBuilder::audio_animation`])
    /// is forwarded to the corresponding
    /// [`VideoLayer`] / [`AudioTrack`](ff_filter::AudioTrack) fields before the filter graphs are built.
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
    /// - [`TimelineError::GeneratedSourceNeedsDuration`] — a generated (Text/Solid)
    ///   clip on an active track has no `out_point` to bound its duration
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
        self.render_inner(output, config, on_progress, false)
    }

    /// Like [`render_with_progress`](Self::render_with_progress) but forces the CPU
    /// `MultiTrackComposer` export path (the force-CPU override of the GPU-default
    /// export). Without the `gpu` feature it is identical to
    /// [`render_with_progress`](Self::render_with_progress).
    ///
    /// # Errors
    ///
    /// Same as [`render_with_progress`](Self::render_with_progress).
    pub fn render_with_progress_forcing_cpu(
        self,
        output: impl AsRef<Path>,
        config: EncoderConfig,
        on_progress: impl Fn(&Progress) -> bool + Send,
    ) -> Result<(), TimelineError> {
        self.render_inner(output, config, on_progress, true)
    }

    /// The shared export driver behind the four public `render*` entry points:
    /// builds the video composition, audio mix, and encoder, then drains them.
    /// `force_cpu` bypasses the GPU export path (see
    /// [`render_forcing_cpu`](Self::render_forcing_cpu)); it is a no-op difference
    /// without the `gpu` feature.
    fn render_inner(
        self,
        output: impl AsRef<Path>,
        config: EncoderConfig,
        on_progress: impl Fn(&Progress) -> bool + Send,
        force_cpu: bool,
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
            .flat_map(|track| track.clips.iter())
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
            next_clip_id: _,
            next_track_id: _,
            next_marker_id: _,
            next_group_id: _,
            next_effect_id: _,
            markers: _,
            lavfi_overlay,
            audio_filter,
        } = self;

        let nv = video_tracks.len();
        let na = audio_tracks.len();

        // Solo is scoped per media list; a track is active unless disabled, muted,
        // or shadowed by a solo elsewhere in its list. Computed once here and
        // reused by the pre-check and both derivation loops.
        let any_video_solo = video_tracks.iter().any(|t| t.solo);
        let any_audio_solo = audio_tracks.iter().any(|t| t.solo);

        // 1. Pre-check: sources of active tracks must exist on disk. Inactive
        //    tracks contribute nothing to the render, so an offline source on a
        //    disabled/muted/soloed-out track must not fail the whole export.
        for (track, any_solo) in video_tracks
            .iter()
            .map(|t| (t, any_video_solo))
            .chain(audio_tracks.iter().map(|t| (t, any_audio_solo)))
        {
            if !track.is_active(any_solo) {
                continue;
            }
            for clip in &track.clips {
                match clip.source_path() {
                    // File source: it must exist on disk.
                    Some(path) => {
                        if !path.exists() {
                            return Err(TimelineError::ClipNotFound {
                                path: path.to_string_lossy().into_owned(),
                            });
                        }
                    }
                    // Generated (Text/Solid) source: infinite, so an out_point is
                    // required to bound its duration. Only out_point matters — the
                    // derive emits `Trim { end }` from it (in_point may be unset).
                    None => {
                        if clip.out_point.is_none() {
                            return Err(TimelineError::GeneratedSourceNeedsDuration);
                        }
                    }
                }
            }
        }

        // GPU export decision (Br4, #1627): an eligible single-track hard-cut
        // timeline with an available adapter composites on the GPU; otherwise the CPU
        // MultiTrackComposer path below runs unchanged. Decided here so the CPU
        // composition graph is not built when the GPU export path will run. No
        // adapter, force-CPU, or an ineligible timeline all fall back to CPU.
        #[cfg(feature = "gpu")]
        let gpu_export: Option<(usize, crate::gpu_compositor::GpuCompositor)> = if force_cpu {
            None
        } else {
            crate::gpu_export::eligible_track(
                &video_tracks,
                lavfi_overlay.as_deref(),
                any_video_solo,
                (canvas_width, canvas_height),
                frame_rate,
            )
            .and_then(|idx| crate::gpu_compositor::GpuCompositor::new().map(|core| (idx, core)))
        };

        // The CPU composition graph is skipped when the GPU export path will run.
        let build_cpu_video = {
            #[cfg(feature = "gpu")]
            {
                gpu_export.is_none()
            }
            #[cfg(not(feature = "gpu"))]
            {
                let _ = force_cpu;
                true
            }
        };

        // 2. Build video composition graph (CPU path).
        let mut video_graph = None;
        if build_cpu_video && !video_tracks.is_empty() {
            // Per-track end-offset (seconds) of the last clip, used to compute
            // the xfade `offset` arg when the next clip has a transition.
            let mut prev_end_by_track: HashMap<usize, f64> = HashMap::new();

            // Generate the canvas/conform at the timeline rate — the same rate
            // the encoder uses below. A hardcoded mismatch stretches the video
            // relative to the audio for non-30fps timelines.
            let mut composer =
                MultiTrackComposer::new(canvas_width, canvas_height).frame_rate(frame_rate);
            // Inactive tracks (disabled, muted, or shadowed by a solo elsewhere in
            // this list) contribute no layers. The enumerate index is preserved so
            // the per-track cross-fade offset bookkeeping (`prev_end_by_track`)
            // stays aligned; track-level automation now lives on the track itself.
            for (track_idx, track) in video_tracks.iter().enumerate() {
                if !track.is_active(any_video_solo) {
                    continue;
                }
                for (clip_idx, clip) in track.clips.iter().enumerate() {
                    let prev_end =
                        (clip_idx > 0).then(|| *prev_end_by_track.get(&track_idx).unwrap_or(&0.0));

                    // When a proxy is set, probe the original source resolution so
                    // the decoded proxy frames can be scaled back up to full size.
                    // If the probe fails the proxy is ignored (original used directly).
                    // A proxy is meaningful only for a file source (generated
                    // sources are rendered at canvas size, nothing to probe).
                    let proxy = clip.proxy.as_ref().zip(clip.source_path()).and_then(
                        |(proxy_path, src)| match VideoDecoder::open(src).build() {
                            Ok(dec) => Some(ProxySource {
                                path: proxy_path.clone(),
                                width: dec.width(),
                                height: dec.height(),
                            }),
                            Err(e) => {
                                log::warn!(
                                    "proxy ignored: cannot probe source {} resolution: {e}",
                                    src.display()
                                );
                                None
                            }
                        },
                    );

                    // Per-clip editorial interpretation → layer lives in `derive`.
                    composer = composer.add_layer(derive::video_layer(
                        clip,
                        track_idx,
                        &track.automation,
                        canvas_width,
                        canvas_height,
                        prev_end,
                        proxy,
                    ));

                    // Track how many seconds this clip contributes, so the next
                    // transition on the same track can compute the correct offset.
                    let end_secs = match clip.duration() {
                        Some(d) => d.as_secs_f64(),
                        None => clip
                            .source_path()
                            .and_then(|src| VideoDecoder::open(src).build().ok())
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
                use ff_filter::{BlendMode, CompositeOp, LayerSource};
                composer = composer.add_layer(VideoLayer {
                    source: LayerSource::Lavfi(lavfi_str.clone()),
                    proxy: None,
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
        //
        //    Two paths share step 7's drain:
        //    * Fast path (no active audio track carries a pre-mix effect chain):
        //      a single source-only mixer over every active clip, streamed to the
        //      encoder (low memory) — the historical behaviour.
        //    * Per-track path (some active track has `audio_effects`): each track
        //      is sub-mixed and run through its own push/pull graph before the
        //      master mix, so a two-pass step (loudness normalization) there
        //      actually fires. Built in step 7 from `audio_tracks`.
        let has_audio = audio_tracks.iter().any(|t| t.is_active(any_audio_solo));
        let has_track_effects = audio_tracks
            .iter()
            .any(|t| t.is_active(any_audio_solo) && !t.audio_effects.is_empty());
        let mut audio_graph = None;
        if has_audio && !has_track_effects {
            let mut mixer = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo);
            // Honor mute/solo/enabled: an inactive audio track is silent.
            for track in &audio_tracks {
                if !track.is_active(any_audio_solo) {
                    continue;
                }
                for clip in &track.clips {
                    // Generated (Text/Solid) clips carry no audio; skip them so a
                    // non-File clip never yields an empty-path audio source.
                    if clip.source_path().is_none() {
                        continue;
                    }
                    mixer = mixer.add_track(derive::audio_track(
                        clip,
                        &track.automation,
                        audio_fade_out_eff_dur(clip),
                    ));
                }
            }
            audio_graph = Some(mixer.build().map_err(TimelineError::Filter)?);
        }

        // Timeline-level (master bus) audio effect chain, applied post-mix. Built
        // whenever there is audio to process (both paths route their mixed output
        // through it). A two-pass step (`LoudnessNormalize`) works here because a
        // builder-made `FilterGraph` carries the `steps` its push/pull path keys
        // off — unlike the source-only mixer graph, where it would be inert.
        let master_audio = if has_audio && !audio_filter.is_empty() {
            let mut builder = FilterGraph::builder();
            for step in &audio_filter {
                builder = builder.add_step(step.clone());
            }
            Some(builder.build().map_err(TimelineError::Filter)?)
        } else {
            None
        };

        // 5. Build encoder.
        let hw = hwaccel_to_hardware_encoder(config.hardware);
        let mut enc_builder = VideoEncoder::create(output)
            .video(canvas_width, canvas_height, frame_rate)
            .video_codec(config.video_codec)
            .bitrate_mode(config.bitrate_mode)
            .hardware_encoder(hw);
        if has_audio {
            enc_builder = enc_builder.audio(48_000, 2).audio_codec(config.audio_codec);
        }
        let mut encoder = enc_builder.build().map_err(TimelineError::Encode)?;

        let start = Instant::now();

        // 6. Drain video → encoder: the GPU export path when eligible, else the CPU
        //    composition graph. Both push to the same unchanged encoder.
        #[cfg(feature = "gpu")]
        if let Some((idx, mut core)) = gpu_export {
            log::info!("export compositor path=gpu");
            crate::gpu_export::drain_video_gpu(
                &video_tracks[idx],
                (canvas_width, canvas_height),
                frame_rate,
                &mut encoder,
                &mut core,
                &on_progress,
                start,
                total_frames,
            )?;
        } else if let Some(vgraph) = video_graph {
            log::info!("export compositor path=cpu");
            drain_composited_graph(
                vgraph,
                &mut encoder,
                &on_progress,
                start,
                total_frames,
                frame_rate,
            )?;
        }
        #[cfg(not(feature = "gpu"))]
        if let Some(vgraph) = video_graph {
            drain_composited_graph(
                vgraph,
                &mut encoder,
                &on_progress,
                start,
                total_frames,
                frame_rate,
            )?;
        }

        // 7. Drain audio graph → (optional master bus) → encoder.
        //    tick() advances the audio animation clock by the actual duration
        //    of each chunk so PTS stays sample-accurate.
        if let Some(mut agraph) = audio_graph {
            if let Some(mut master) = master_audio {
                // Route the mix through the master effect chain, interleaving pulls
                // so a plain-node chain streams (low memory) rather than buffering
                // the whole program. A two-pass step (loudness normalization) buffers
                // internally and emits nothing until `flush_audio` signals EOF, so
                // the interleaved pull naturally degrades to buffer-all for it; the
                // trailing flush + drain then emits the processed output.
                let mut audio_pts = Duration::ZERO;
                loop {
                    agraph.tick(audio_pts);
                    match agraph.pull_audio().map_err(TimelineError::Filter)? {
                        Some(frame) => {
                            audio_pts += frame.duration();
                            master
                                .push_audio(0, &frame)
                                .map_err(TimelineError::Filter)?;
                            while let Some(out) =
                                master.pull_audio().map_err(TimelineError::Filter)?
                            {
                                encoder.push_audio(&out).map_err(TimelineError::Encode)?;
                            }
                        }
                        None => break,
                    }
                }
                master.flush_audio();
                while let Some(frame) = master.pull_audio().map_err(TimelineError::Filter)? {
                    encoder.push_audio(&frame).map_err(TimelineError::Encode)?;
                }
            } else {
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
        } else if has_track_effects {
            // Per-track path: each track's audio is sub-mixed and run through its
            // own push/pull effect graph (so a two-pass step fires), then summed.
            let mixed = mix_tracks_with_effects(&audio_tracks, any_audio_solo)?;
            // Consume `mixed` by value so each frame drops right after it is pushed
            // downstream, freeing the mix buffer as we go (the master bus keeps its
            // own copy for a two-pass step, so holding `mixed` too would double it).
            if let Some(mut master) = master_audio {
                for frame in mixed {
                    master
                        .push_audio(0, &frame)
                        .map_err(TimelineError::Filter)?;
                }
                master.flush_audio();
                while let Some(frame) = master.pull_audio().map_err(TimelineError::Filter)? {
                    encoder.push_audio(&frame).map_err(TimelineError::Encode)?;
                }
            } else {
                for frame in mixed {
                    encoder.push_audio(&frame).map_err(TimelineError::Encode)?;
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

/// Drains a built CPU composition [`FilterGraph`] to the encoder (the historical
/// export video loop). `tick()` runs before each pull so per-frame animation entries
/// update the filter parameters; `on_progress` is invoked after each push and
/// returning `false` cancels with [`TimelineError::Cancelled`].
fn drain_composited_graph(
    mut vgraph: FilterGraph,
    encoder: &mut VideoEncoder,
    on_progress: &(impl Fn(&Progress) -> bool + Send),
    start: Instant,
    total_frames: Option<u64>,
    frame_rate: f64,
) -> Result<(), TimelineError> {
    let mut video_idx: u32 = 0;
    loop {
        #[allow(clippy::cast_precision_loss)] // frame index fits comfortably in f64 mantissa
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
    Ok(())
}

/// Resolves a clip's effective duration for a fade-out start offset, probing the
/// source only when a fade-out actually needs it. `None` = no fade-out, or the
/// duration could not be determined.
fn audio_fade_out_eff_dur(clip: &Clip) -> Option<Duration> {
    if clip.fade_out == Duration::ZERO {
        return None;
    }
    clip.duration().or_else(|| {
        clip.source_path()
            .and_then(|src| VideoDecoder::open(src).build().ok())
            .map(|d| {
                let total = d.duration();
                match clip.in_point {
                    Some(ip) => total.saturating_sub(ip),
                    None => total,
                }
            })
    })
}

/// Pulls a source-only audio graph to end-of-stream into a frame buffer, ticking
/// the animation clock by each chunk's duration so any volume automation stays
/// sample-accurate.
fn drain_source_audio(graph: &mut FilterGraph) -> Result<Vec<AudioFrame>, TimelineError> {
    let mut out = Vec::new();
    let mut pts = Duration::ZERO;
    loop {
        graph.tick(pts);
        match graph.pull_audio().map_err(TimelineError::Filter)? {
            Some(frame) => {
                pts += frame.duration();
                out.push(frame);
            }
            None => break,
        }
    }
    Ok(out)
}

/// The per-track (pre-mix) audio path: sub-mix each active track's clips, run the
/// track's [`audio_effects`](Track::audio_effects) chain through its own push/pull
/// [`FilterGraph`] (so a two-pass step such as loudness normalization fires), then
/// sum the processed tracks with an additive `amix`. Returns the mixed program
/// audio (before the timeline master bus).
fn mix_tracks_with_effects(
    audio_tracks: &[Track],
    any_audio_solo: bool,
) -> Result<Vec<AudioFrame>, TimelineError> {
    let mut track_buffers: Vec<Vec<AudioFrame>> = Vec::new();
    for track in audio_tracks {
        if !track.is_active(any_audio_solo) {
            continue;
        }
        // Sub-mix this track's audio-bearing clips (generated clips carry no audio).
        let mut sub = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo);
        let mut has_clip = false;
        for clip in &track.clips {
            if clip.source_path().is_none() {
                continue;
            }
            sub = sub.add_track(derive::audio_track(
                clip,
                &track.automation,
                audio_fade_out_eff_dur(clip),
            ));
            has_clip = true;
        }
        if !has_clip {
            continue;
        }
        let mut sub_graph = sub.build().map_err(TimelineError::Filter)?;

        // No effect chain: the track's contribution is the raw sub-mix. Otherwise
        // route the sub-mix through the track's push/pull effect graph.
        let processed = if track.audio_effects.is_empty() {
            drain_source_audio(&mut sub_graph)?
        } else {
            let mut fx_builder = FilterGraph::builder();
            for step in &track.audio_effects {
                fx_builder = fx_builder.add_step(step.clone());
            }
            let mut fx = fx_builder.build().map_err(TimelineError::Filter)?;
            let mut pts = Duration::ZERO;
            loop {
                sub_graph.tick(pts);
                match sub_graph.pull_audio().map_err(TimelineError::Filter)? {
                    Some(frame) => {
                        pts += frame.duration();
                        fx.push_audio(0, &frame).map_err(TimelineError::Filter)?;
                    }
                    None => break,
                }
            }
            fx.flush_audio();
            let mut out = Vec::new();
            while let Some(frame) = fx.pull_audio().map_err(TimelineError::Filter)? {
                out.push(frame);
            }
            out
        };
        if !processed.is_empty() {
            track_buffers.push(processed);
        }
    }

    // Sum the processed tracks. One (or zero) track needs no mix; several are
    // combined with an additive `amix`, pushed slot-by-slot in frame-index
    // lockstep so the inputs stay aligned and no input reaches EOF early.
    Ok(match track_buffers.len() {
        0 => Vec::new(),
        1 => track_buffers.into_iter().next().unwrap_or_default(),
        n => {
            let mut amix = FilterGraph::builder()
                .amix(n)
                .build()
                .map_err(TimelineError::Filter)?;
            let max_len = track_buffers.iter().map(Vec::len).max().unwrap_or(0);
            let mut mixed = Vec::new();
            for i in 0..max_len {
                for (slot, buf) in track_buffers.iter().enumerate() {
                    if let Some(frame) = buf.get(i) {
                        amix.push_audio(slot, frame)
                            .map_err(TimelineError::Filter)?;
                    }
                }
                while let Some(frame) = amix.pull_audio().map_err(TimelineError::Filter)? {
                    mixed.push(frame);
                }
            }
            amix.flush_audio();
            while let Some(frame) = amix.pull_audio().map_err(TimelineError::Filter)? {
                mixed.push(frame);
            }
            mixed
        }
    })
}

/// Builder for [`Timeline`].
///
/// Obtain one via [`Timeline::builder()`].
pub struct TimelineBuilder {
    canvas_width: Option<u32>,
    canvas_height: Option<u32>,
    frame_rate: Option<f64>,
    video_tracks: Vec<Track>,
    audio_tracks: Vec<Track>,
    /// See [`TimelineBuilder::lavfi_overlay`].
    lavfi_overlay: Option<String>,
    /// See [`TimelineBuilder::audio_filter`].
    audio_filter: Vec<FilterStep>,
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
            lavfi_overlay: None,
            audio_filter: Vec::new(),
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

    /// Appends a video track holding `clips` (default flags, no name). Track 0
    /// (first call) is the bottom layer.
    ///
    /// Use [`video_track_with`](Self::video_track_with) to append a track with a
    /// name or mute/solo/enabled/lock flags set.
    #[must_use]
    pub fn video_track(self, clips: Vec<Clip>) -> Self {
        self.video_track_with(Track::new(clips))
    }

    /// Appends a preconfigured video [`Track`] (name, mute/solo/enabled/lock).
    #[must_use]
    pub fn video_track_with(self, track: Track) -> Self {
        let mut video_tracks = self.video_tracks;
        video_tracks.push(track);
        Self {
            video_tracks,
            ..self
        }
    }

    /// Appends an audio track holding `clips` (default flags, no name).
    #[must_use]
    pub fn audio_track(self, clips: Vec<Clip>) -> Self {
        self.audio_track_with(Track::new(clips))
    }

    /// Appends a preconfigured audio [`Track`] (name, mute/solo/enabled/lock).
    #[must_use]
    pub fn audio_track_with(self, track: Track) -> Self {
        let mut audio_tracks = self.audio_tracks;
        audio_tracks.push(track);
        Self {
            audio_tracks,
            ..self
        }
    }

    /// Sets a video-layer animation on the video track at `track_index` (its
    /// position among the video tracks added so far).
    ///
    /// The animation is stored on the [`Track`] itself (see
    /// [`TrackAutomation`](crate::TrackAutomation)), so it stays with that track
    /// through reordering or removal. Add the video track first; an out-of-range
    /// `track_index` is ignored with a `log::warn!`.
    #[must_use]
    pub fn video_animation(
        mut self,
        track_index: usize,
        property: VideoProperty,
        animation: AnimationTrack<f64>,
    ) -> Self {
        if let Some(track) = self.video_tracks.get_mut(track_index) {
            track.automation.set_video(property, animation);
        } else {
            log::warn!("video_animation ignored: no video track at index={track_index}");
        }
        self
    }

    /// Sets an audio-track animation on the audio track at `track_index` (its
    /// position among the audio tracks added so far).
    ///
    /// The animation is stored on the [`Track`] itself (see
    /// [`TrackAutomation`](crate::TrackAutomation)), so it stays with that track
    /// through reordering or removal. Add the audio track first; an out-of-range
    /// `track_index` is ignored with a `log::warn!`.
    #[must_use]
    pub fn audio_animation(
        mut self,
        track_index: usize,
        property: AudioProperty,
        animation: AnimationTrack<f64>,
    ) -> Self {
        if let Some(track) = self.audio_tracks.get_mut(track_index) {
            track.automation.set_audio(property, animation);
        } else {
            log::warn!("audio_animation ignored: no audio track at index={track_index}");
        }
        self
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

    /// Sets a timeline-level (master bus) audio effect chain applied to the final
    /// mix on render.
    ///
    /// The steps run in order on the whole program's mixed audio, after the
    /// multi-track mix and before the encoder — the natural place for loudness
    /// normalization ([`FilterStep::LoudnessNormalize`]). When not set (the
    /// default, an empty chain) the audio path is unchanged. Per-track (pre-mix)
    /// effects are a separate feature (see issue #1446).
    #[must_use]
    pub fn audio_filter(self, steps: Vec<FilterStep>) -> Self {
        Self {
            audio_filter: steps,
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

        // Stamp stable ids from monotonic counters (0 = unset; ids start at 1),
        // video tracks first. The final counter values are stored so later edits
        // (`AddClip` / `AddTrack`) keep minting fresh, never-reused ids.
        let mut next_track_id: u64 = 1;
        let mut next_clip_id: u64 = 1;
        let mut next_effect_id: u64 = 1;
        let mut video_tracks = self.video_tracks;
        let mut audio_tracks = self.audio_tracks;
        for track in video_tracks.iter_mut().chain(audio_tracks.iter_mut()) {
            track.id = TrackId::from_raw(next_track_id);
            next_track_id += 1;
            for clip in &mut track.clips {
                clip.id = ClipId::from_raw(next_clip_id);
                next_clip_id += 1;
                for effect in &mut clip.effects {
                    effect.id = EffectId::from_raw(next_effect_id);
                    next_effect_id += 1;
                }
            }
        }

        Ok(Timeline {
            canvas_width,
            canvas_height,
            canvas_explicit,
            frame_rate,
            video_tracks,
            audio_tracks,
            next_clip_id,
            next_track_id,
            next_marker_id: 1,
            next_group_id: 1,
            next_effect_id,
            markers: Vec::new(),
            lavfi_overlay: self.lavfi_overlay,
            audio_filter: self.audio_filter,
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

        // Probe the first video clip when it is file-backed. A leading generated
        // (Text/Solid) clip has no file to probe (`source_path()` is `None`), so
        // the canvas falls through to the 1920x1080@30 default.
        if need_probe
            && let Some(source) = self
                .video_tracks
                .first()
                .and_then(|t| t.clips.first())
                .and_then(|c| c.source_path())
        {
            if !source.exists() {
                return Err(TimelineError::ClipNotFound {
                    path: source.to_string_lossy().into_owned(),
                });
            }
            let vdec = VideoDecoder::open(source).build()?;
            let w = self.canvas_width.unwrap_or_else(|| vdec.width());
            let h = self.canvas_height.unwrap_or_else(|| vdec.height());
            let fps = self.frame_rate.unwrap_or_else(|| vdec.frame_rate());
            return Ok((w, h, fps));
        }

        // All values explicit, no video tracks (audio-only), or a leading
        // generated clip with no file to probe — fall back for absent values.
        Ok((
            self.canvas_width.unwrap_or(1920),
            self.canvas_height.unwrap_or(1080),
            self.frame_rate.unwrap_or(30.0),
        ))
    }
}

// Timeline -> Scene derivation (real-time preview)

/// Projects one video clip into a [`ScenePlacement`](ff_preview::ScenePlacement).
/// `is_base` selects the V1 base track, where a crossfade transition contributes a
/// `xfade_dur`; overlays force zero (matching the compositor).
#[cfg(feature = "preview")]
fn video_placement(
    clip: &Clip,
    is_base: bool,
    automation: &crate::track::TrackAutomation,
    canvas_width: u32,
    canvas_height: u32,
) -> ff_preview::ScenePlacement {
    let xfade_dur = if is_base && clip.transition.is_some() {
        clip.transition_duration
    } else {
        Duration::ZERO
    };
    // Carry the transition kind (not just its duration) so preview renders the
    // actual xfade kind; overlays force no transition (matching the compositor).
    let xfade_kind = if is_base { clip.transition } else { None };
    ff_preview::ScenePlacement {
        // File clips decode; generated (Text/Solid) clips are rendered by the runner
        // via ff-filter's SolidSource/TextSource (the same filters export uses).
        source: match &clip.source {
            crate::clip::ClipSource::File(path) => ff_preview::SceneSource::File(path.clone()),
            crate::clip::ClipSource::Text(spec) => ff_preview::SceneSource::Text(spec.clone()),
            crate::clip::ClipSource::Solid(color) => ff_preview::SceneSource::Solid(*color),
        },
        offset: clip.offset,
        in_point: clip.in_point.unwrap_or(Duration::ZERO),
        out_point: clip.out_point,
        speed: clip.speed.max(0.01),
        xfade_dur,
        xfade_kind,
        opacity: clip.opacity.clamp(0.0, 1.0),
        // The single derive: preview and export build their video layers from the
        // same `avio::derive`, so the timeline-level animations (scale/rotation and
        // the opacity/x/y track-level fallbacks) reach the preview too.
        layer: crate::derive::realtime_descriptor(clip, automation, canvas_width, canvas_height),
        fade_in: clip.fade_in,
        fade_out: clip.fade_out,
        // V1 clip audio has no dedicated audio-track counterpart in export (which
        // mixes only `audio_tracks`), so its volume is the per-clip merge only.
        volume: crate::derive::audio_volume(clip, &crate::track::TrackAutomation::default()),
        // The same shared derive as export, so preview pitch matches export.
        pitch: crate::derive::audio_pitch(clip),
        // The pan 3-way merge, same shared derive as export.
        pan: crate::derive::audio_pan(clip, &crate::track::TrackAutomation::default()),
    }
}

/// Projects one audio-only clip into a [`SceneAudioPlacement`](ff_preview::SceneAudioPlacement).
/// `automation` is the audio track's [`TrackAutomation`](crate::track::TrackAutomation).
#[cfg(feature = "preview")]
fn audio_placement(
    clip: &Clip,
    automation: &crate::track::TrackAutomation,
) -> ff_preview::SceneAudioPlacement {
    ff_preview::SceneAudioPlacement {
        source: clip
            .source_path()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        offset: clip.offset,
        in_point: clip.in_point.unwrap_or(Duration::ZERO),
        out_point: clip.out_point,
        speed: clip.speed.max(0.01),
        fade_in: clip.fade_in,
        fade_out: clip.fade_out,
        // The single derive: the volume 3-way merge (incl. the track-level
        // `volume` automation) reaches preview, matching export.
        volume: crate::derive::audio_volume(clip, automation),
        // The same shared derive as export, so preview pitch matches export.
        pitch: crate::derive::audio_pitch(clip),
        // The pan 3-way merge (incl. the track-level `pan` automation), matching export.
        pan: crate::derive::audio_pan(clip, automation),
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
        // Inactive tracks (disabled, muted, or shadowed by a solo elsewhere in the
        // list) project no placements, but keep their slot so the base-track
        // (index 0) rule stays aligned; track-level automation lives on the track.
        let any_video_solo = self.video_tracks.iter().any(|t| t.solo);
        let video_tracks = self
            .video_tracks
            .iter()
            .enumerate()
            .map(|(track_idx, track)| ff_preview::SceneVideoTrack {
                placements: if track.is_active(any_video_solo) {
                    track
                        .clips
                        .iter()
                        .map(|clip| {
                            video_placement(
                                clip,
                                track_idx == 0,
                                &track.automation,
                                self.canvas_width,
                                self.canvas_height,
                            )
                        })
                        .collect()
                } else {
                    Vec::new()
                },
            })
            .collect();

        let any_audio_solo = self.audio_tracks.iter().any(|t| t.solo);
        let audio_tracks = self
            .audio_tracks
            .iter()
            .map(|track| ff_preview::SceneAudioTrack {
                placements: if track.is_active(any_audio_solo) {
                    track
                        .clips
                        .iter()
                        .map(|clip| audio_placement(clip, &track.automation))
                        .collect()
                } else {
                    Vec::new()
                },
            })
            .collect();

        ff_preview::Scene {
            fps: self.frame_rate().max(1.0),
            canvas: self.explicit_canvas(),
            lavfi_overlay: self.lavfi_overlay.clone(),
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
        assert!(
            matches!(&base.source, ff_preview::SceneSource::File(p) if p.to_str() == Some("a.mp4"))
        );
        assert_eq!(base.offset, Duration::from_millis(500));
        assert_eq!(base.in_point, Duration::from_secs(1));
        assert_eq!(base.out_point, Some(Duration::from_secs(3)));
        assert!((base.speed - 2.0).abs() < f64::EPSILON);
        assert!((base.opacity - 0.5).abs() < f32::EPSILON);
        assert_eq!(base.xfade_dur, Duration::ZERO, "clip 0 has no transition");
        assert_eq!(base.xfade_kind, None, "clip 0 has no transition kind");
        assert!(matches!(base.volume, AnimatedValue::Track(_)));
        assert!(
            matches!(base.layer.opacity, AnimatedValue::Static(v) if (v - 0.5).abs() < f64::EPSILON)
        );

        let base1 = &scene.video_tracks[0].placements[1];
        assert_eq!(base1.xfade_dur, Duration::from_millis(750));
        assert_eq!(
            base1.xfade_kind,
            Some(XfadeTransition::Fade),
            "base track carries the xfade kind"
        );

        let overlay = &scene.video_tracks[1].placements[0];
        assert_eq!(
            overlay.xfade_dur,
            Duration::ZERO,
            "overlay transitions must project as zero"
        );
        assert_eq!(
            overlay.xfade_kind, None,
            "overlay transition kind is forced off"
        );

        let audio = &scene.audio_tracks[0].placements[0];
        assert_eq!(audio.source.to_str(), Some("music.mp3"));
        assert_eq!(audio.fade_in, Duration::from_millis(200));
        assert_eq!(audio.fade_out, Duration::from_millis(300));
        assert!(matches!(audio.volume, AnimatedValue::Static(v) if (v + 6.0).abs() < f64::EPSILON));
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_route_timeline_animations_into_the_preview_layer() {
        use ff_filter::{AnimatedValue, Easing, FilterStep, Keyframe};

        // Track 1 (overlay) gets timeline scale_x + rotation animations, and an opacity
        // animation the neutral clip should fall back to (the 3-way merge). The single
        // derive must route all three into the preview placement's layer: opacity as an
        // animated scalar, scale/rotation as self-animating effect steps (ADR-0005).
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("base.mp4")])
            .video_track(vec![Clip::new("overlay.mp4")])
            .video_animation(
                1,
                VideoProperty::ScaleX,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
            )
            .video_animation(
                1,
                VideoProperty::Rotation,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 45.0, Easing::Linear)),
            )
            .video_animation(
                1,
                VideoProperty::Opacity,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 1.0, Easing::Linear)),
            )
            .build()
            .unwrap();

        let scene = timeline.to_scene();
        let overlay = &scene.video_tracks[1].placements[0];
        // Opacity animates on the scalar (send_command); scale/rotation move to
        // self-animating effect steps with a neutralized scalar.
        assert!(matches!(overlay.layer.opacity, AnimatedValue::Track(_)));
        assert!(
            matches!(overlay.layer.scale_x, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9)
        );
        assert!(matches!(overlay.layer.rotation, AnimatedValue::Static(v) if v.abs() < 1e-9));
        assert!(
            overlay
                .layer
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::ScaleAnimated { .. }))
        );
        assert!(
            overlay
                .layer
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::RotateAnimated { .. }))
        );
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_carry_lavfi_overlay() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .lavfi_overlay("color=s=1920x1080:c=black@0.0")
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        assert_eq!(
            scene.lavfi_overlay.as_deref(),
            Some("color=s=1920x1080:c=black@0.0")
        );
    }

    #[test]
    fn timeline_default_audio_filter_should_be_empty() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .build()
            .unwrap();
        assert!(timeline.audio_filter.is_empty());
    }

    #[test]
    fn timeline_builder_audio_filter_should_set_chain() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .audio_filter(vec![FilterStep::Volume(-6.0)])
            .build()
            .unwrap();
        assert_eq!(timeline.audio_filter.len(), 1);
        assert!(matches!(
            timeline.audio_filter[0],
            FilterStep::Volume(v) if (v - (-6.0)).abs() < 1e-9
        ));
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_carry_audio_speed_and_merged_volume() {
        use ff_filter::{Easing, Keyframe};

        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4")])
            .audio_track(vec![Clip::new("a.mp3").with_speed(2.0)]) // neutral volume
            .audio_animation(
                0,
                AudioProperty::Volume,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, -3.0, Easing::Linear)),
            )
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        let audio = &scene.audio_tracks[0].placements[0];
        assert!((audio.speed - 2.0).abs() < f64::EPSILON);
        // The neutral clip volume falls back to the audio track's `volume` automation.
        assert!(matches!(audio.volume, AnimatedValue::Track(_)));
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_carry_pitch() {
        use ff_filter::{Easing, Keyframe};

        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4").with_pitch(3.0)])
            .audio_track(vec![Clip::new("a.mp3").with_pitch(1.0).with_pitch_track(
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 5.0, Easing::Linear)),
            )])
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        // Video-clip audio carries the static per-clip pitch.
        assert!((scene.video_tracks[0].placements[0].pitch - 3.0).abs() < f64::EPSILON);
        // Audio-only clip: a set pitch_track wins over the static pitch, at t=0.
        assert!((scene.audio_tracks[0].placements[0].pitch - 5.0).abs() < f64::EPSILON);
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_carry_clip_pan() {
        use ff_filter::{Easing, Keyframe};

        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4").pan(-0.4)])
            .audio_track(vec![Clip::new("a.mp3")]) // center clip pan
            .audio_animation(
                0,
                AudioProperty::Pan,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.6, Easing::Linear)),
            )
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        // Video-clip audio carries the static per-clip pan.
        assert!(matches!(
            scene.video_tracks[0].placements[0].pan,
            AnimatedValue::Static(x) if (x + 0.4).abs() < 1e-9
        ));
        // Audio-only clip: the center clip pan falls back to the track's pan automation.
        assert!(matches!(
            scene.audio_tracks[0].placements[0].pan,
            AnimatedValue::Track(_)
        ));
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_project_text_and_solid_as_scene_source() {
        use ff_format::{Color, TextSpec};

        // #1615: generated (Text/Solid) clips project as SceneSource variants (not an
        // empty path) so the runner renders them; File clips stay File; timeline order
        // is preserved.
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("base.mp4"),
                Clip::text(TextSpec::new("Title")).trim(Duration::ZERO, Duration::from_secs(2)),
                Clip::solid(Color::rgb(255, 0, 0)).trim(Duration::ZERO, Duration::from_secs(1)),
            ])
            .build()
            .unwrap();
        let placements = &timeline.to_scene().video_tracks[0].placements;
        assert!(matches!(
            &placements[0].source,
            ff_preview::SceneSource::File(p) if p.to_str() == Some("base.mp4")
        ));
        assert!(matches!(
            &placements[1].source,
            ff_preview::SceneSource::Text(spec) if spec.text == "Title"
        ));
        assert!(matches!(
            &placements[2].source,
            ff_preview::SceneSource::Solid(c) if *c == Color::rgb(255, 0, 0)
        ));
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_drop_disabled_video_track_placements() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track_with(Track::new(vec![Clip::new("base.mp4")]).enabled(false))
            .video_track(vec![Clip::new("overlay.mp4")])
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        assert_eq!(scene.video_tracks.len(), 2, "track slots are preserved");
        assert!(
            scene.video_tracks[0].placements.is_empty(),
            "a disabled track projects no placements"
        );
        assert_eq!(scene.video_tracks[1].placements.len(), 1);
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_drop_muted_video_track_placements() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track_with(Track::new(vec![Clip::new("base.mp4")]).muted(true))
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        assert!(scene.video_tracks[0].placements.is_empty());
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_solo_should_keep_only_soloed_video_tracks() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("base.mp4")]) // not soloed
            .video_track_with(Track::new(vec![Clip::new("overlay.mp4")]).soloed(true))
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        assert!(
            scene.video_tracks[0].placements.is_empty(),
            "a non-soloed track is shadowed when another is soloed"
        );
        assert_eq!(scene.video_tracks[1].placements.len(), 1);
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_drop_muted_audio_track_placements() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4")])
            .audio_track_with(Track::new(vec![Clip::new("a.mp3")]).muted(true))
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        assert!(scene.audio_tracks[0].placements.is_empty());
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_should_drop_disabled_audio_track_placements() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4")])
            .audio_track_with(Track::new(vec![Clip::new("a.mp3")]).enabled(false))
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        assert!(scene.audio_tracks[0].placements.is_empty());
    }

    #[cfg(feature = "preview")]
    #[test]
    fn to_scene_solo_should_keep_only_soloed_audio_tracks() {
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4")])
            .audio_track(vec![Clip::new("a.mp3")]) // not soloed
            .audio_track_with(Track::new(vec![Clip::new("b.mp3")]).soloed(true))
            .build()
            .unwrap();
        let scene = timeline.to_scene();
        assert!(
            scene.audio_tracks[0].placements.is_empty(),
            "a non-soloed audio track is shadowed when another is soloed"
        );
        assert_eq!(scene.audio_tracks[1].placements.len(), 1);
    }

    #[test]
    fn timeline_builder_should_err_when_no_tracks() {
        let result = Timeline::builder().build();
        assert!(matches!(result, Err(TimelineError::NoInput)));
    }

    #[test]
    fn render_should_reject_generated_clip_without_out_point() {
        use ff_format::TextSpec;
        // A Text clip with no out_point is infinite; the render pre-check must
        // reject it before touching FFmpeg (deterministic on any machine).
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::text(TextSpec::new("no out_point"))])
            .build()
            .unwrap();
        let out = std::env::temp_dir().join("avio_generated_no_outpoint_test.mp4");
        let result = timeline.render(out, EncoderConfig::builder().build());
        assert!(matches!(
            result,
            Err(TimelineError::GeneratedSourceNeedsDuration)
        ));
    }

    #[test]
    fn render_should_accept_generated_clip_with_out_point_only() {
        use ff_format::Color;
        // Only out_point bounds a generated source (in_point may be unset); the
        // derive emits `Trim { end }` from it. The pre-check must NOT reject this
        // valid, bounded clip. Deterministic on any machine: whatever the render
        // outcome, it must not be GeneratedSourceNeedsDuration.
        let mut clip = Clip::solid(Color::rgb(0, 0, 0));
        clip.out_point = Some(Duration::from_secs(1));
        assert!(clip.in_point.is_none());
        let timeline = Timeline::builder()
            .canvas(160, 90)
            .frame_rate(30.0)
            .video_track(vec![clip])
            .build()
            .unwrap();
        let out = std::env::temp_dir().join("avio_generated_outpoint_only_test.mp4");
        let result = timeline.render(out, EncoderConfig::builder().build());
        assert!(
            !matches!(result, Err(TimelineError::GeneratedSourceNeedsDuration)),
            "an out_point-only generated clip must not be rejected"
        );
    }

    #[test]
    fn build_should_default_canvas_when_first_clip_is_generated() {
        use ff_format::Color;
        // A leading generated clip has no file to probe, so the canvas falls
        // through to the 1920x1080@30 default without any I/O.
        let timeline = Timeline::builder()
            .video_track(vec![
                Clip::solid(Color::rgb(0, 0, 0)).trim(Duration::ZERO, Duration::from_secs(1)),
            ])
            .build()
            .unwrap();
        assert_eq!(timeline.canvas_width, 1920);
        assert_eq!(timeline.canvas_height, 1080);
        assert!((timeline.frame_rate - 30.0).abs() < f64::EPSILON);
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
            .video_animation(0, VideoProperty::Opacity, track)
            .build()
            .unwrap();

        assert!(
            timeline.video_tracks[0].automation.opacity.is_some(),
            "the opacity animation lives on the track"
        );
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
            .audio_animation(0, AudioProperty::Volume, track)
            .build()
            .unwrap();

        assert!(
            timeline.audio_tracks[0].automation.volume.is_some(),
            "the volume animation lives on the track"
        );
    }

    #[test]
    fn reordering_tracks_should_not_misalign_automation() {
        use ff_filter::{Easing, Keyframe};

        // Track 0 carries an opacity animation; track 1 does not.
        let mut timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .video_track(vec![Clip::new("b.mp4")])
            .video_animation(
                0,
                VideoProperty::Opacity,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
            )
            .build()
            .unwrap();

        // Reorder the two tracks. With index-keyed automation this would misalign;
        // with on-track automation the animation moves with its track.
        timeline.video_tracks.swap(0, 1);

        assert!(
            timeline.video_tracks[1].automation.opacity.is_some(),
            "the animated track keeps its automation after reordering"
        );
        assert!(
            timeline.video_tracks[0].automation.opacity.is_none(),
            "the un-animated track gains no automation after reordering"
        );
    }

    #[test]
    fn removing_a_track_should_drop_only_its_automation() {
        use crate::{Command, apply};
        use ff_filter::{Easing, Keyframe};

        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .video_track(vec![Clip::new("b.mp4")])
            .video_animation(
                0,
                VideoProperty::Opacity,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
            )
            .video_animation(
                1,
                VideoProperty::Rotation,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 45.0, Easing::Linear)),
            )
            .build()
            .unwrap();

        let removed = timeline.video_tracks[0].id;
        let kept_rotation_present = timeline.video_tracks[1].automation.rotation.is_some();
        let out = apply(&timeline, &Command::RemoveTrack { track: removed }).unwrap();

        assert_eq!(out.video_tracks.len(), 1, "one track removed");
        assert!(
            out.video_tracks[0].automation.rotation.is_some() && kept_rotation_present,
            "the surviving track keeps its own automation"
        );
        assert!(
            out.video_tracks[0].automation.opacity.is_none(),
            "the removed track's automation is gone (only its automation dropped)"
        );
    }

    #[test]
    fn video_animation_out_of_range_index_should_be_ignored() {
        use ff_filter::{Easing, Keyframe};

        // Only one video track exists; addressing index 5 must be ignored (a
        // warn, not a panic) and leave no automation behind.
        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .video_animation(
                5,
                VideoProperty::Opacity,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
            )
            .build()
            .unwrap();
        assert!(
            timeline.video_tracks[0].automation.opacity.is_none(),
            "an out-of-range track_index sets no automation"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn track_automation_should_round_trip_through_serde() {
        use ff_filter::{Easing, Keyframe};

        let timeline = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .video_animation(
                0,
                VideoProperty::Opacity,
                AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
            )
            .build()
            .unwrap();

        let json = serde_json::to_string(&timeline).unwrap();
        let back: Timeline = serde_json::from_str(&json).unwrap();
        assert!(
            back.video_tracks[0].automation.opacity.is_some(),
            "track automation must survive a serde round-trip"
        );
    }
}
