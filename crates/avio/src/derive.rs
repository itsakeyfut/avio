//! Pure per-clip derivation for the export path.
//!
//! [`Timeline::render`](crate::Timeline::render) interprets each [`Clip`]'s
//! editorial fields into an `ff-filter` [`VideoLayer`] / [`AudioTrack`] for the
//! offline compositor/mixer. That interpretation — the leading trim/offset/speed
//! filter steps, the per-clip effect chain, the trailing cross-fade, and the
//! static-vs-keyframe-vs-track-animation merge — lives here as pure functions so
//! it has exactly one home and can be unit-tested without `FFmpeg`. The `render`
//! caller keeps the I/O (probing source resolution and durations).
//!
//! The compositing part of this interpretation (the per-clip effect chain) is
//! shared with the preview path via [`Clip::video_effect_chain`] /
//! [`Clip::realtime_layer_descriptor`]. The remaining export-only concepts that
//! preview does not yet honour are the `derive(model, t)` unification target of
//! #1351 (see `docs/specs/engine-and-primitives.md` §2.1).

use std::path::Path;
use std::time::Duration;

use ff_filter::{
    AnimatedValue, AnimationTrack, AudioTrack, BlendMode, CompositeOp, FilterStep, Keyframe,
    LayerSource, PitchAlgo, ProxySource, RealtimeLayerDescriptor, ScaleAlgorithm, VideoLayer,
};
use ff_format::ChannelLayout;

use crate::clip::{Clip, ClipSource, FitMode};
use crate::track::TrackAutomation;

/// Resolves a track-level automation slot: `AnimatedValue::Track` when the track
/// carries an animation for the property, else `Static(default)`.
fn track_anim(slot: Option<&AnimationTrack<f64>>, default: f64) -> AnimatedValue<f64> {
    slot.map_or(AnimatedValue::Static(default), |t| {
        AnimatedValue::Track(t.clone())
    })
}

/// The per-clip video transform shared by the export [`VideoLayer`] and the
/// preview [`RealtimeLayerDescriptor`]: the 3-way-merged opacity/position, the
/// scale/rotation, and the blend/composite operators. This is the single
/// interpretation both executors consume; only the temporal steps (trim/offset/
/// speed) and the cross-fade differ between the two paths.
struct VideoTransform {
    opacity: AnimatedValue<f64>,
    x: AnimatedValue<f64>,
    y: AnimatedValue<f64>,
    scale_x: AnimatedValue<f64>,
    scale_y: AnimatedValue<f64>,
    rotation: AnimatedValue<f64>,
    blend_mode: BlendMode,
    composite_op: CompositeOp,
}

/// Computes the shared [`VideoTransform`]: a per-clip keyframe track wins, then a
/// static non-neutral value, then any timeline track-level animation.
fn video_transform(clip: &Clip, automation: &TrackAutomation) -> VideoTransform {
    #[allow(clippy::float_cmp)]
    let opacity = if let Some(track) = &clip.opacity_track {
        AnimatedValue::Track(track.clone())
    } else if clip.opacity != 1.0 {
        AnimatedValue::Static(f64::from(clip.opacity))
    } else {
        track_anim(automation.opacity.as_ref(), 1.0)
    };
    #[allow(clippy::float_cmp)]
    let x = if let Some(track) = &clip.x_track {
        AnimatedValue::Track(track.clone())
    } else if clip.x != 0.0 {
        AnimatedValue::Static(clip.x)
    } else {
        track_anim(automation.x.as_ref(), 0.0)
    };
    #[allow(clippy::float_cmp)]
    let y = if let Some(track) = &clip.y_track {
        AnimatedValue::Track(track.clone())
    } else if clip.y != 0.0 {
        AnimatedValue::Static(clip.y)
    } else {
        track_anim(automation.y.as_ref(), 0.0)
    };
    // The uniform per-clip `scale` drives both axes; a per-clip track wins, then a
    // static non-neutral value, then the per-axis timeline animation (kept
    // independent for `scale_x` vs `scale_y` in the fallback).
    #[allow(clippy::float_cmp)]
    let scale_x = if let Some(track) = &clip.scale_track {
        AnimatedValue::Track(track.clone())
    } else if clip.scale != 1.0 {
        AnimatedValue::Static(clip.scale)
    } else {
        track_anim(automation.scale_x.as_ref(), 1.0)
    };
    #[allow(clippy::float_cmp)]
    let scale_y = if let Some(track) = &clip.scale_track {
        AnimatedValue::Track(track.clone())
    } else if clip.scale != 1.0 {
        AnimatedValue::Static(clip.scale)
    } else {
        track_anim(automation.scale_y.as_ref(), 1.0)
    };
    #[allow(clippy::float_cmp)]
    let rotation = if let Some(track) = &clip.rotation_track {
        AnimatedValue::Track(track.clone())
    } else if clip.rotation != 0.0 {
        AnimatedValue::Static(clip.rotation)
    } else {
        track_anim(automation.rotation.as_ref(), 0.0)
    };
    VideoTransform {
        opacity,
        x,
        y,
        scale_x,
        scale_y,
        rotation,
        blend_mode: clip.blend_mode,
        composite_op: clip.composite_op,
    }
}

/// Multiplies every keyframe value by `dim`, converting a scale **factor** track
/// into a **pixel** track for [`FilterStep::ScaleAnimated`] (which sizes in pixels,
/// while the static compositor node uses `canvas * factor`). Timestamps and easing
/// are preserved so the animation shape is unchanged.
fn scale_track_pixels(track: &AnimationTrack<f64>, dim: u32) -> AnimationTrack<f64> {
    let mut out = AnimationTrack::new();
    for kf in track.keyframes() {
        out = out.push(Keyframe::new(
            kf.timestamp,
            kf.value * f64::from(dim),
            kf.easing.clone(),
        ));
    }
    out
}

/// Maps a scale-factor [`AnimatedValue`] to pixels (`value * dim`), keeping the
/// `Static`/`Track` shape.
fn to_pixels(value: &AnimatedValue<f64>, dim: u32) -> AnimatedValue<f64> {
    match value {
        AnimatedValue::Static(v) => AnimatedValue::Static(v * f64::from(dim)),
        AnimatedValue::Track(t) => AnimatedValue::Track(scale_track_pixels(t, dim)),
    }
}

/// The per-frame geometry: when the merged scale or rotation is animated, returns the
/// self-animating [`FilterStep::ScaleAnimated`]/[`RotateAnimated`] steps to splice
/// into the effect chain plus the **neutralized** static scalars (`scale=1.0` /
/// `rotation=0.0`) so the compositor's static transform node is skipped and the
/// animation is applied once, per frame, identically in preview and export
/// (ADR-0005). A non-animated axis returns no step and its scalar unchanged.
fn animated_geometry(
    transform: &VideoTransform,
    canvas_width: u32,
    canvas_height: u32,
) -> (
    Vec<FilterStep>,
    AnimatedValue<f64>,
    AnimatedValue<f64>,
    AnimatedValue<f64>,
) {
    let mut steps = Vec::new();
    let mut scale_x = transform.scale_x.clone();
    let mut scale_y = transform.scale_y.clone();
    let mut rotation = transform.rotation.clone();

    // Rotation is emitted BEFORE scale: `RotateAnimated` supersamples around a stable
    // input size, so it must run on the un-scaled frame; an animated `ScaleAnimated`
    // then varies the output size, which the downstream `overlay` handles. (Scale is
    // uniform for the clip `scale`, so the two commute.)
    if matches!(transform.rotation, AnimatedValue::Track(_)) {
        steps.push(FilterStep::RotateAnimated {
            // `angle` is degrees (matching the compositor's static rotate node).
            angle: transform.rotation.clone(),
            fill_color: "black".to_string(),
        });
        rotation = AnimatedValue::Static(0.0);
    }
    // Scale animates as a whole (both axes) whenever either axis is a track, so the
    // single `ScaleAnimated` owns both dimensions and both scalars neutralize.
    if matches!(transform.scale_x, AnimatedValue::Track(_))
        || matches!(transform.scale_y, AnimatedValue::Track(_))
    {
        steps.push(FilterStep::ScaleAnimated {
            width: to_pixels(&transform.scale_x, canvas_width),
            height: to_pixels(&transform.scale_y, canvas_height),
            algorithm: ScaleAlgorithm::Bicubic,
        });
        scale_x = AnimatedValue::Static(1.0);
        scale_y = AnimatedValue::Static(1.0);
    }
    (steps, scale_x, scale_y, rotation)
}

/// Maps a clip's [`FitMode`] to the canvas-relative framing [`FilterStep`], or
/// `None` for [`FitMode::None`] (native size, no framing). Shared by the export
/// [`video_layer`] and preview [`realtime_descriptor`] paths so both frame a clip
/// against the canvas identically. `cw`/`ch` are the project canvas dimensions.
fn fit_step(clip: &Clip, cw: u32, ch: u32) -> Option<FilterStep> {
    if cw == 0 || ch == 0 {
        return None; // no canvas to frame against
    }
    match clip.fit {
        FitMode::None => None,
        FitMode::Stretch => Some(FilterStep::Scale {
            width: cw,
            height: ch,
            algorithm: ScaleAlgorithm::Bilinear,
        }),
        FitMode::Fit => Some(FilterStep::FitToAspect {
            width: cw,
            height: ch,
            color: "black".to_string(),
        }),
        FitMode::Fill => Some(FilterStep::FillToAspect {
            width: cw,
            height: ch,
        }),
    }
}

/// Derives the export [`VideoLayer`] for one clip.
///
/// `canvas_width`/`canvas_height` are the project canvas dimensions (for the
/// [`fit`](Clip::fit) framing step). `prev_end` is the preceding clip's
/// end-seconds on this track (for the cross-fade offset); `None` marks the
/// track's first clip — a transition on it is ignored with a warning. `proxy` is
/// the caller-probed proxy source.
pub(crate) fn video_layer(
    clip: &Clip,
    track_idx: usize,
    automation: &TrackAutomation,
    canvas_width: u32,
    canvas_height: u32,
    prev_end: Option<f64>,
    proxy: Option<ProxySource>,
) -> VideoLayer {
    // Timeline trim + placement are emitted as leading filter steps so they
    // precede timing-sensitive effects (Speed), matching the compositor node
    // order (trim → setpts=PTS-STARTPTS → setpts=PTS+offset).
    let mut layer_effects: Vec<FilterStep> = Vec::new();
    if clip.in_point.is_some() || clip.out_point.is_some() {
        layer_effects.push(FilterStep::Trim {
            start: clip.in_point.map(|d| d.as_secs_f64()),
            end: clip.out_point.map(|d| d.as_secs_f64()),
        });
        layer_effects.push(FilterStep::ResetPts);
    }
    if clip.offset > Duration::ZERO {
        layer_effects.push(FilterStep::OffsetPts {
            seconds: clip.offset.as_secs_f64(),
        });
    }
    if (clip.speed - 1.0).abs() > 1e-9 {
        layer_effects.push(FilterStep::Speed { factor: clip.speed });
    }
    // Per-frame scale/rotation: when the model animates them, splice self-animating
    // steps here — after the temporal placement (`Trim`/`ResetPts`/`OffsetPts`/`Speed`),
    // so the `t`-expression sees timeline time — and neutralize the static layer
    // transform below to avoid double-application (ADR-0005).
    let transform = video_transform(clip, automation);
    let (geometry, scale_x, scale_y, rotation) =
        animated_geometry(&transform, canvas_width, canvas_height);
    layer_effects.extend(geometry);
    // Frame the source to the project canvas (cover/contain/stretch) before the
    // colour/effect chain; `FitMode::None` emits nothing (native size).
    if let Some(step) = fit_step(clip, canvas_width, canvas_height) {
        layer_effects.push(step);
    }
    // Colour-correction (eq) + caller-attached per-clip video effects, shared
    // with the preview path via `Clip::video_effect_chain`.
    layer_effects.extend(clip.video_effect_chain());

    // Cross-fade from the preceding clip on this track, emitted as a trailing
    // step (after the layer's other effects, matching the compositor wiring).
    if let Some(kind) = clip.transition {
        match prev_end {
            Some(prev_end) => {
                let dur_secs = clip.transition_duration.as_secs_f64();
                layer_effects.push(FilterStep::XFade {
                    transition: kind,
                    duration: dur_secs,
                    offset: (prev_end - dur_secs).max(0.0),
                });
            }
            None => {
                log::warn!(
                    "transition on a track's first clip ignored (no preceding clip to cross-fade from) track={track_idx}"
                );
            }
        }
    }

    // Pure model→primitive source mapping (no FFmpeg translation here).
    let source = match &clip.source {
        ClipSource::File(path) => LayerSource::File(path.clone()),
        ClipSource::Text(spec) => LayerSource::Text(spec.clone()),
        ClipSource::Solid(color) => LayerSource::Solid(*color),
    };
    VideoLayer {
        source,
        proxy,
        x: transform.x,
        y: transform.y,
        // Neutralized when scale/rotation is animated (the animation lives in
        // `effects` as `ScaleAnimated`/`RotateAnimated`); unchanged when static.
        scale_x,
        scale_y,
        rotation,
        opacity: transform.opacity,
        blend_mode: transform.blend_mode,
        composite_op: transform.composite_op,
        effects: layer_effects,
    }
}

/// Derives the preview [`RealtimeLayerDescriptor`] for one clip.
///
/// Uses the same [`VideoTransform`] as [`video_layer`] (so both paths share one
/// interpretation) and the same canvas [`fit`](Clip::fit) framing step
/// (`canvas_width`/`canvas_height`), but otherwise carries only the per-clip
/// effect chain: the temporal steps (trim/offset/speed) and the cross-fade are
/// omitted because the preview runner realises them from `ScenePlacement` (in/out
/// points, speed, transition).
///
/// The timeline-level `lavfi_overlay` and a clip's generated (Text/Solid) source
/// are not projected here (preview drops them today); closing that is C4d, not this
/// backbone.
pub(crate) fn realtime_descriptor(
    clip: &Clip,
    automation: &TrackAutomation,
    canvas_width: u32,
    canvas_height: u32,
) -> RealtimeLayerDescriptor {
    let transform = video_transform(clip, automation);
    // Per-frame scale/rotation: the same self-animating steps the export path emits.
    // The preview runner stamps each pushed frame with the timeline PTS, so the
    // `t`-expression evaluates at the same time as export (ADR-0005). Placed before
    // the fit/colour chain, matching the export layer order.
    let (geometry, scale_x, scale_y, rotation) =
        animated_geometry(&transform, canvas_width, canvas_height);
    let mut effects = geometry;
    // Frame to the canvas (same step as the export path) before the colour chain,
    // so preview and export share one framing interpretation.
    if let Some(step) = fit_step(clip, canvas_width, canvas_height) {
        effects.push(step);
    }
    effects.extend(clip.video_effect_chain());
    RealtimeLayerDescriptor {
        effects,
        opacity: transform.opacity,
        x: transform.x,
        y: transform.y,
        // Neutralized when animated (the animation lives in `effects`); else unchanged.
        scale_x,
        scale_y,
        rotation,
        blend_mode: transform.blend_mode,
        composite_op: transform.composite_op,
    }
}

/// The 3-way-merged audio volume (dB): a per-clip volume track wins, then a static
/// non-zero `volume_db`, then the track-level `volume` automation. This is
/// the single interpretation the export [`audio_track`] and the preview audio
/// projection ([`Timeline::to_scene`](crate::Timeline::to_scene)) share.
#[allow(clippy::float_cmp)]
pub(crate) fn audio_volume(clip: &Clip, automation: &TrackAutomation) -> AnimatedValue<f64> {
    match &clip.volume_track {
        Some(track) => AnimatedValue::Track(track.clone()),
        None if clip.volume_db != 0.0 => AnimatedValue::Static(clip.volume_db),
        None => track_anim(automation.volume.as_ref(), 0.0),
    }
}

/// The 3-way-merged audio pan (`-1.0` left .. `+1.0` right): a non-zero static
/// `Clip.pan` wins, then the track-level `pan` automation, else center (`0.0`).
/// Mirrors [`audio_volume`] and is shared by the export [`audio_track`] and the
/// preview projection ([`Timeline::to_scene`](crate::Timeline::to_scene)) so they
/// cannot diverge.
#[allow(clippy::float_cmp)]
pub(crate) fn audio_pan(clip: &Clip, automation: &TrackAutomation) -> AnimatedValue<f64> {
    if clip.pan != 0.0 {
        AnimatedValue::Static(clip.pan)
    } else {
        track_anim(automation.pan.as_ref(), 0.0)
    }
}

/// The per-clip pitch (semitones): a set `pitch_track` evaluated at its `t=0`
/// value, else the static `Clip.pitch`. Per-sample pitch automation is a deferred
/// primitive capability (see ADR-0002). Shared by the export [`audio_track`] and
/// the preview projection ([`Timeline::to_scene`](crate::Timeline::to_scene)) so
/// they cannot diverge.
pub(crate) fn audio_pitch(clip: &Clip) -> f64 {
    clip.pitch_track
        .as_ref()
        .map_or(clip.pitch, |t| t.value_at(Duration::ZERO))
}

/// Derives the export [`AudioTrack`] for one clip.
///
/// `fade_out_eff_dur` is the caller-resolved effective clip duration, used only
/// to compute the fade-out start offset (`None` = could not be determined).
pub(crate) fn audio_track(
    clip: &Clip,
    automation: &TrackAutomation,
    fade_out_eff_dur: Option<Duration>,
) -> AudioTrack {
    let volume = audio_volume(clip, automation);
    // Generated (Text/Solid) clips carry no audio and are skipped by the render's
    // audio loop before reaching here, so a File source is expected; the fallback
    // to an empty path is defensive only.
    let source_path = clip
        .source_path()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    // Timeline trim + placement first, then speed/fade/effect steps, matching
    // the mixer node order (atrim → asetpts=PTS-STARTPTS → adelay).
    let mut effects: Vec<FilterStep> = Vec::new();
    if clip.in_point.is_some() || clip.out_point.is_some() {
        effects.push(FilterStep::ATrim {
            start: clip.in_point.map(|d| d.as_secs_f64()),
            end: clip.out_point.map(|d| d.as_secs_f64()),
        });
        effects.push(FilterStep::AResetPts);
    }
    if clip.offset > Duration::ZERO {
        // `as_millis()` matches the old inline `adelay` (integer ms); offset
        // magnitudes are far below f64's exact-integer range.
        #[allow(clippy::cast_precision_loss)]
        let ms = clip.offset.as_millis() as f64;
        effects.push(FilterStep::AudioDelay { ms });
    }
    if (clip.speed - 1.0).abs() > 1e-9 {
        effects.push(FilterStep::Speed { factor: clip.speed });
    }
    // Per-clip pitch shift (semitones), via the shared `audio_pitch` so export and
    // preview cannot diverge on the value.
    let pitch = audio_pitch(clip);
    if pitch.abs() > 1e-9 {
        #[allow(clippy::cast_possible_truncation)]
        effects.push(FilterStep::PitchShift {
            semitones: pitch as f32,
            // The model does not yet select a pitch backend; export uses the
            // always-available signal path.
            algo: PitchAlgo::Signal,
        });
    }
    if clip.fade_in > Duration::ZERO {
        effects.push(FilterStep::AFadeIn {
            start: 0.0,
            duration: clip.fade_in.as_secs_f64(),
        });
    }
    if clip.fade_out > Duration::ZERO {
        match fade_out_eff_dur {
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
                    "fade_out ({:.3}s) >= clip duration — skipping fade_out for {}",
                    clip.fade_out.as_secs_f64(),
                    source_path.display(),
                );
            }
            None => {
                log::warn!(
                    "cannot determine clip duration — skipping fade_out for {}",
                    source_path.display(),
                );
            }
        }
    }
    // Caller-attached per-clip audio effects run last.
    effects.extend(clip.audio_effects.iter().cloned());

    AudioTrack {
        source: source_path,
        volume,
        pan: audio_pan(clip, automation),
        effects,
        sample_rate: 48_000,
        channel_layout: ChannelLayout::Stereo,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ff_filter::{Easing, Keyframe, XfadeTransition};

    use super::*;

    fn no_anim() -> TrackAutomation {
        TrackAutomation::default()
    }

    // video_layer

    #[test]
    fn video_layer_file_clip_should_map_to_file_source() {
        let clip = Clip::new("a.mp4");
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        match layer.source {
            LayerSource::File(path) => assert_eq!(path.to_str(), Some("a.mp4")),
            other => panic!("expected File source, got {other:?}"),
        }
    }

    #[test]
    fn video_layer_text_clip_should_map_to_text_source() {
        use ff_format::TextSpec;
        let clip = Clip::text(TextSpec::new("title"));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        match layer.source {
            LayerSource::Text(spec) => assert_eq!(spec.text, "title"),
            other => panic!("expected Text source, got {other:?}"),
        }
    }

    #[test]
    fn video_layer_solid_clip_should_map_to_solid_source() {
        use ff_format::Color;
        let clip = Clip::solid(Color::rgb(1, 2, 3));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        match layer.source {
            LayerSource::Solid(color) => assert_eq!(color, Color::rgb(1, 2, 3)),
            other => panic!("expected Solid source, got {other:?}"),
        }
    }

    #[test]
    fn video_layer_trim_should_lead_with_trim_and_resetpts() {
        let clip = Clip::new("a.mp4").trim(Duration::from_secs(1), Duration::from_secs(3));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(
            layer.effects[0],
            FilterStep::Trim {
                start: Some(s),
                end: Some(e)
            } if (s - 1.0).abs() < 1e-9 && (e - 3.0).abs() < 1e-9
        ));
        assert!(matches!(layer.effects[1], FilterStep::ResetPts));
    }

    #[test]
    fn video_layer_offset_should_emit_offsetpts() {
        let clip = Clip::new("a.mp4").offset(Duration::from_secs(2));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(layer.effects.iter().any(
            |s| matches!(s, FilterStep::OffsetPts { seconds } if (seconds - 2.0).abs() < 1e-9)
        ));
    }

    #[test]
    fn video_layer_speed_should_emit_speed() {
        let clip = Clip::new("a.mp4").with_speed(2.0);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(
            layer
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::Speed { factor } if (factor - 2.0).abs() < 1e-9))
        );
    }

    #[test]
    fn video_layer_transition_with_prev_end_should_emit_xfade() {
        let clip =
            Clip::new("b.mp4").with_transition(XfadeTransition::Fade, Duration::from_millis(500));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, Some(4.0), None);
        // offset = (prev_end 4.0 - dur 0.5).max(0) = 3.5
        assert!(layer.effects.iter().any(|s| matches!(
            s,
            FilterStep::XFade { duration, offset, .. }
                if (duration - 0.5).abs() < 1e-9 && (offset - 3.5).abs() < 1e-9
        )));
    }

    #[test]
    fn video_layer_transition_on_first_clip_should_emit_no_xfade() {
        let clip =
            Clip::new("a.mp4").with_transition(XfadeTransition::Fade, Duration::from_millis(500));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(
            !layer
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::XFade { .. }))
        );
    }

    // Per-frame scale/rotation (ADR-0005)

    #[test]
    fn video_layer_animated_scale_should_emit_scale_animated_and_neutralize() {
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(2), 1.5, Easing::Linear));
        let clip = Clip::new("a.mp4").with_scale_track(track);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);

        // The static layer transform is neutralized so the compositor's static scale
        // node is skipped (no double-application).
        assert!(matches!(layer.scale_x, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        assert!(matches!(layer.scale_y, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));

        // The animation is carried as a self-animating ScaleAnimated with the factor
        // track converted to pixels (factor × canvas).
        let sa = layer
            .effects
            .iter()
            .find(|s| matches!(s, FilterStep::ScaleAnimated { .. }));
        let Some(FilterStep::ScaleAnimated { width, height, .. }) = sa else {
            panic!(
                "animated scale must emit ScaleAnimated, got {:?}",
                layer.effects
            );
        };
        assert!((width.value_at(Duration::ZERO) - 960.0).abs() < 1e-6); // 0.5 × 1920
        assert!((height.value_at(Duration::ZERO) - 540.0).abs() < 1e-6); // 0.5 × 1080
        assert!((width.value_at(Duration::from_secs(2)) - 2880.0).abs() < 1e-6); // 1.5 × 1920
    }

    #[test]
    fn video_layer_animated_rotation_should_emit_rotate_animated_and_neutralize() {
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.0, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(1), 90.0, Easing::Linear));
        let clip = Clip::new("a.mp4").with_rotation_track(track);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);

        assert!(matches!(layer.rotation, AnimatedValue::Static(v) if v.abs() < 1e-9));
        let ra = layer
            .effects
            .iter()
            .find(|s| matches!(s, FilterStep::RotateAnimated { .. }));
        let Some(FilterStep::RotateAnimated { angle, .. }) = ra else {
            panic!(
                "animated rotation must emit RotateAnimated, got {:?}",
                layer.effects
            );
        };
        // Degrees pass through unchanged (the compositor's rotate node converts).
        assert!((angle.value_at(Duration::from_secs(1)) - 90.0).abs() < 1e-6);
    }

    #[test]
    fn video_layer_static_scale_should_stay_on_the_layer() {
        // AC2: a non-animated scale must NOT be routed through ScaleAnimated and must
        // stay on the layer scalar, so the static render is unchanged.
        let clip = Clip::new("a.mp4").with_scale(0.5);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(
            !layer
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::ScaleAnimated { .. }))
        );
        assert!(matches!(layer.scale_x, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
    }

    #[test]
    fn realtime_descriptor_animated_scale_should_emit_scale_animated() {
        // The preview path emits the same self-animating step as export (parity).
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(2), 1.0, Easing::Linear));
        let clip = Clip::new("a.mp4").with_scale_track(track);
        let desc = realtime_descriptor(&clip, &no_anim(), 1280, 720);
        assert!(matches!(desc.scale_x, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        assert!(
            desc.effects
                .iter()
                .any(|s| matches!(s, FilterStep::ScaleAnimated { .. }))
        );
    }

    #[test]
    fn video_layer_opacity_track_should_win_over_static() {
        let mut clip = Clip::new("a.mp4").with_opacity(0.5);
        clip.opacity_track =
            Some(AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.0, Easing::Linear)));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.opacity, AnimatedValue::Track(_)));
    }

    #[test]
    fn video_layer_static_opacity_should_be_animatedvalue_static() {
        let clip = Clip::new("a.mp4").with_opacity(0.5);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.opacity, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
    }

    #[test]
    fn video_layer_neutral_opacity_should_fall_back_to_track_animation() {
        let automation = TrackAutomation {
            opacity: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                1.0,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp4"); // opacity defaults to 1.0 (neutral)
        let layer = video_layer(&clip, 0, &automation, 1920, 1080, None, None);
        assert!(matches!(layer.opacity, AnimatedValue::Track(_)));
    }

    #[test]
    fn video_layer_should_pass_through_blend_and_composite() {
        use ff_filter::{BlendMode, CompositeOp};
        let mut clip = Clip::new("a.mp4");
        clip.blend_mode = BlendMode::Multiply;
        clip.composite_op = CompositeOp::Under;
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.blend_mode, BlendMode::Multiply));
        assert!(matches!(layer.composite_op, CompositeOp::Under));
    }

    #[test]
    fn video_layer_should_order_trim_offset_speed_chain_xfade() {
        let mut clip = Clip::new("a.mp4")
            .trim(Duration::from_secs(1), Duration::from_secs(5))
            .offset(Duration::from_secs(2))
            .with_speed(2.0)
            .with_transition(XfadeTransition::Fade, Duration::from_millis(500));
        // A non-neutral color-correct effect makes `video_effect_chain` emit a trailing Eq step.
        clip = clip.with_color_correction(0.5, 1.0, 1.0);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, Some(10.0), None);
        let e = &layer.effects;
        assert!(matches!(e[0], FilterStep::Trim { .. }));
        assert!(matches!(e[1], FilterStep::ResetPts));
        assert!(matches!(e[2], FilterStep::OffsetPts { .. }));
        assert!(matches!(e[3], FilterStep::Speed { .. }));
        assert!(matches!(e[4], FilterStep::Eq { .. }));
        assert!(matches!(e[5], FilterStep::XFade { .. }));
    }

    // fit / fill framing (#1422)

    #[test]
    fn video_layer_fit_none_should_emit_no_framing_step() {
        let clip = Clip::new("a.mp4"); // fit defaults to None
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(!layer.effects.iter().any(|s| matches!(
            s,
            FilterStep::FillToAspect { .. }
                | FilterStep::FitToAspect { .. }
                | FilterStep::Scale { .. }
        )));
    }

    #[test]
    fn video_layer_fit_fill_should_emit_fill_to_aspect() {
        let clip = Clip::new("a.mp4").with_fit(FitMode::Fill);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(layer.effects.iter().any(|s| matches!(
            s,
            FilterStep::FillToAspect {
                width: 1920,
                height: 1080
            }
        )));
    }

    #[test]
    fn video_layer_fit_fit_should_emit_fit_to_aspect_black() {
        let clip = Clip::new("a.mp4").with_fit(FitMode::Fit);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(layer.effects.iter().any(|s| matches!(
            s,
            FilterStep::FitToAspect { width: 1920, height: 1080, color } if color == "black"
        )));
    }

    #[test]
    fn video_layer_fit_stretch_should_emit_scale_to_canvas() {
        let clip = Clip::new("a.mp4").with_fit(FitMode::Stretch);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(layer.effects.iter().any(|s| matches!(
            s,
            FilterStep::Scale {
                width: 1920,
                height: 1080,
                algorithm: ScaleAlgorithm::Bilinear
            }
        )));
    }

    #[test]
    fn video_layer_fit_should_sit_after_speed_and_before_effect_chain() {
        let clip = Clip::new("a.mp4")
            .with_speed(2.0)
            .with_fit(FitMode::Fill)
            .with_color_correction(0.5, 1.0, 1.0); // trailing Eq via `video_effect_chain`
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        let e = &layer.effects;
        assert!(matches!(e[0], FilterStep::Speed { .. }));
        assert!(matches!(e[1], FilterStep::FillToAspect { .. }));
        assert!(matches!(e[2], FilterStep::Eq { .. }));
    }

    #[test]
    fn realtime_descriptor_fit_fill_should_share_step_with_export() {
        // export == preview parity: both emit the same framing step.
        let clip = Clip::new("a.mp4").with_fit(FitMode::Fill);
        let d = realtime_descriptor(&clip, &no_anim(), 1920, 1080);
        assert!(matches!(
            d.effects.first(),
            Some(FilterStep::FillToAspect {
                width: 1920,
                height: 1080
            })
        ));
    }

    #[test]
    fn realtime_descriptor_fit_none_should_emit_no_framing_step() {
        let clip = Clip::new("a.mp4"); // fit defaults to None
        let d = realtime_descriptor(&clip, &no_anim(), 1920, 1080);
        assert!(!d.effects.iter().any(|s| matches!(
            s,
            FilterStep::FillToAspect { .. }
                | FilterStep::FitToAspect { .. }
                | FilterStep::Scale { .. }
        )));
    }

    #[test]
    fn fit_step_should_be_skipped_when_canvas_is_zero() {
        // The canvas-less `Clip::realtime_layer_descriptor` passes 0×0.
        let clip = Clip::new("a.mp4").with_fit(FitMode::Fill);
        let d = realtime_descriptor(&clip, &no_anim(), 0, 0);
        assert!(
            !d.effects
                .iter()
                .any(|s| matches!(s, FilterStep::FillToAspect { .. }))
        );
    }

    // audio_track

    #[test]
    fn audio_track_volume_track_should_win() {
        let mut clip = Clip::new("a.mp3").volume(-6.0);
        clip.volume_track =
            Some(AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.0, Easing::Linear)));
        let track = audio_track(&clip, &no_anim(), None);
        assert!(matches!(track.volume, AnimatedValue::Track(_)));
    }

    #[test]
    fn audio_track_static_volume_db_should_be_static() {
        let clip = Clip::new("a.mp3").volume(-6.0);
        let track = audio_track(&clip, &no_anim(), None);
        assert!(matches!(track.volume, AnimatedValue::Static(v) if (v + 6.0).abs() < 1e-9));
    }

    #[test]
    fn audio_track_should_order_trim_delay_speed_fades_effects() {
        let clip = Clip::new("a.mp3")
            .trim(Duration::from_secs(1), Duration::from_secs(5))
            .offset(Duration::from_millis(500))
            .with_speed(2.0)
            .with_fade_in(Duration::from_millis(200))
            .with_fade_out(Duration::from_millis(300));
        let track = audio_track(&clip, &no_anim(), Some(Duration::from_secs(4)));
        let kinds: Vec<&FilterStep> = track.effects.iter().collect();
        assert!(matches!(kinds[0], FilterStep::ATrim { .. }));
        assert!(matches!(kinds[1], FilterStep::AResetPts));
        assert!(matches!(kinds[2], FilterStep::AudioDelay { .. }));
        assert!(matches!(kinds[3], FilterStep::Speed { .. }));
        assert!(matches!(kinds[4], FilterStep::AFadeIn { .. }));
        assert!(matches!(kinds[5], FilterStep::AFadeOut { .. }));
    }

    #[test]
    fn audio_track_fade_out_should_skip_when_duration_not_greater() {
        let clip = Clip::new("a.mp3").with_fade_out(Duration::from_secs(5));
        // eff_dur (3s) <= fade_out (5s) -> skipped
        let track = audio_track(&clip, &no_anim(), Some(Duration::from_secs(3)));
        assert!(
            !track
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::AFadeOut { .. }))
        );
    }

    #[test]
    fn audio_track_pan_should_come_from_track_animation() {
        let automation = TrackAutomation {
            pan: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                0.5,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp3");
        let track = audio_track(&clip, &automation, None);
        assert!(matches!(track.pan, AnimatedValue::Track(_)));
    }

    #[test]
    fn audio_track_static_pitch_should_emit_pitch_shift() {
        let clip = Clip::new("a.mp3").with_pitch(4.0);
        let track = audio_track(&clip, &no_anim(), None);
        assert!(track.effects.iter().any(|s| matches!(
            s,
            FilterStep::PitchShift { semitones, .. } if (semitones - 4.0).abs() < 1e-6
        )));
    }

    #[test]
    fn audio_track_pitch_track_should_emit_pitch_shift_at_t0() {
        // A per-clip pitch track wins over the static pitch and renders at its
        // `t=0` value (per-sample automation is deferred; see ADR-0002).
        let clip = Clip::new("a.mp3").with_pitch(1.0).with_pitch_track(
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 5.0, Easing::Linear)),
        );
        let track = audio_track(&clip, &no_anim(), None);
        assert!(track.effects.iter().any(|s| matches!(
            s,
            FilterStep::PitchShift { semitones, .. } if (semitones - 5.0).abs() < 1e-6
        )));
    }

    #[test]
    fn audio_track_zero_pitch_should_emit_no_pitch_shift() {
        let clip = Clip::new("a.mp3"); // pitch defaults to 0.0
        let track = audio_track(&clip, &no_anim(), None);
        assert!(
            !track
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::PitchShift { .. }))
        );
    }

    // audio_volume (shared 3-way merge)

    #[test]
    fn audio_volume_should_fall_back_to_timeline_animation() {
        let automation = TrackAutomation {
            volume: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                -3.0,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp3"); // neutral volume_db, no volume_track
        assert!(matches!(
            audio_volume(&clip, &automation),
            AnimatedValue::Track(_)
        ));
    }

    #[test]
    fn audio_volume_static_should_win_over_timeline() {
        let automation = TrackAutomation {
            volume: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                -3.0,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp3").volume(-6.0);
        assert!(matches!(
            audio_volume(&clip, &automation),
            AnimatedValue::Static(x) if (x + 6.0).abs() < 1e-9
        ));
    }

    // audio_pan (shared 3-way merge)

    #[test]
    fn audio_pan_should_prefer_static_clip_pan() {
        // A non-zero static clip pan wins over the track-level pan animation.
        let automation = TrackAutomation {
            pan: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                -0.5,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp3").pan(0.8);
        assert!(matches!(
            audio_pan(&clip, &automation),
            AnimatedValue::Static(x) if (x - 0.8).abs() < 1e-9
        ));
    }

    #[test]
    fn audio_pan_should_fall_back_to_track_animation() {
        let automation = TrackAutomation {
            pan: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                0.5,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp3"); // center pan, no clip pan
        assert!(matches!(
            audio_pan(&clip, &automation),
            AnimatedValue::Track(_)
        ));
    }

    #[test]
    fn audio_track_should_apply_clip_pan() {
        // A clip pan reaches the mixer struct's `pan` field (previously discarded).
        let clip = Clip::new("a.mp3").pan(0.8);
        let track = audio_track(&clip, &no_anim(), None);
        assert!(matches!(
            track.pan,
            AnimatedValue::Static(x) if (x - 0.8).abs() < 1e-9
        ));
    }

    // realtime_descriptor (single derive → preview)

    #[test]
    fn realtime_descriptor_should_carry_no_temporal_or_xfade_steps() {
        // The preview runner realises trim/offset/speed/xfade from `ScenePlacement`;
        // the descriptor must carry only the per-clip effect chain.
        let clip = Clip::new("a.mp4")
            .trim(Duration::from_secs(1), Duration::from_secs(3))
            .offset(Duration::from_secs(2))
            .with_speed(2.0)
            .with_transition(XfadeTransition::Fade, Duration::from_millis(500));
        let d = realtime_descriptor(&clip, &no_anim(), 1920, 1080);
        assert!(!d.effects.iter().any(|s| matches!(
            s,
            FilterStep::Trim { .. }
                | FilterStep::ResetPts
                | FilterStep::OffsetPts { .. }
                | FilterStep::Speed { .. }
                | FilterStep::XFade { .. }
        )));
    }

    #[test]
    fn realtime_descriptor_should_carry_effect_chain() {
        let clip = Clip::new("a.mp4").with_color_correction(0.5, 1.0, 1.0); // forces an Eq step in `video_effect_chain`
        let d = realtime_descriptor(&clip, &no_anim(), 1920, 1080);
        assert!(d.effects.iter().any(|s| matches!(s, FilterStep::Eq { .. })));
    }

    #[test]
    fn realtime_descriptor_should_pick_up_timeline_scale_and_rotation() {
        let automation = TrackAutomation {
            scale_x: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                0.5,
                Easing::Linear,
            ))),
            rotation: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                90.0,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp4");
        let d = realtime_descriptor(&clip, &automation, 1920, 1080);
        // Timeline scale/rotation animations are carried as self-animating steps and
        // the scalars neutralize (ADR-0005), same as the export path.
        assert!(matches!(d.scale_x, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        assert!(matches!(d.scale_y, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        assert!(matches!(d.rotation, AnimatedValue::Static(v) if v.abs() < 1e-9));
        assert!(
            d.effects
                .iter()
                .any(|s| matches!(s, FilterStep::ScaleAnimated { .. }))
        );
        assert!(
            d.effects
                .iter()
                .any(|s| matches!(s, FilterStep::RotateAnimated { .. }))
        );
    }

    #[test]
    fn realtime_descriptor_opacity_should_fall_back_to_timeline_animation() {
        // A neutral per-clip opacity (1.0) falls through to the track automation.
        let automation = TrackAutomation {
            opacity: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                1.0,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp4");
        let d = realtime_descriptor(&clip, &automation, 1920, 1080);
        assert!(matches!(d.opacity, AnimatedValue::Track(_)));
    }

    #[test]
    fn realtime_descriptor_static_opacity_should_win_over_timeline() {
        // A per-clip static non-neutral opacity wins the 3-way merge.
        let automation = TrackAutomation {
            opacity: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                0.2,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp4").with_opacity(0.5);
        let d = realtime_descriptor(&clip, &automation, 1920, 1080);
        assert!(matches!(d.opacity, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
    }

    #[test]
    fn realtime_descriptor_should_carry_composite_op() {
        let mut clip = Clip::new("a.mp4");
        clip.composite_op = CompositeOp::Under;
        let d = realtime_descriptor(&clip, &no_anim(), 1920, 1080);
        assert!(matches!(d.composite_op, CompositeOp::Under));
    }

    // per-clip scale / rotation (3-way merge)

    #[test]
    fn video_layer_static_scale_should_drive_both_axes() {
        let clip = Clip::new("a.mp4").with_scale(0.5);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.scale_x, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
        assert!(matches!(layer.scale_y, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
    }

    #[test]
    fn video_layer_scale_track_should_win_on_both_axes() {
        // A per-clip scale track drives both axes. Post-ADR-0005 the animation is
        // carried as a self-animating ScaleAnimated (factor × canvas) and the layer
        // scalar neutralizes; both width and height reflect the 2.0 factor.
        let mut clip = Clip::new("a.mp4");
        clip.scale_track =
            Some(AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 2.0, Easing::Linear)));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.scale_x, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        assert!(matches!(layer.scale_y, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        let Some(FilterStep::ScaleAnimated { width, height, .. }) = layer
            .effects
            .iter()
            .find(|s| matches!(s, FilterStep::ScaleAnimated { .. }))
        else {
            panic!("expected ScaleAnimated, got {:?}", layer.effects);
        };
        assert!((width.value_at(Duration::ZERO) - 3840.0).abs() < 1e-6); // 2.0 × 1920
        assert!((height.value_at(Duration::ZERO) - 2160.0).abs() < 1e-6); // 2.0 × 1080
    }

    #[test]
    fn video_layer_neutral_scale_should_fall_back_to_timeline_animation() {
        let automation = TrackAutomation {
            scale_x: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                0.5,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        let clip = Clip::new("a.mp4"); // scale defaults to 1.0 (neutral)
        let layer = video_layer(&clip, 0, &automation, 1920, 1080, None, None);
        // The scale_x timeline animation is carried into ScaleAnimated's width; the
        // un-animated scale_y axis maps to a static height (1.0 × canvas).
        assert!(matches!(layer.scale_x, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        assert!(matches!(layer.scale_y, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
        let Some(FilterStep::ScaleAnimated { width, height, .. }) = layer
            .effects
            .iter()
            .find(|s| matches!(s, FilterStep::ScaleAnimated { .. }))
        else {
            panic!("expected ScaleAnimated, got {:?}", layer.effects);
        };
        assert!((width.value_at(Duration::ZERO) - 960.0).abs() < 1e-6); // 0.5 × 1920
        assert!((height.value_at(Duration::ZERO) - 1080.0).abs() < 1e-6); // 1.0 × 1080
    }

    #[test]
    fn video_layer_static_scale_should_win_over_timeline_animation() {
        let automation = TrackAutomation {
            scale_x: Some(AnimationTrack::new().push(Keyframe::new(
                Duration::ZERO,
                0.25,
                Easing::Linear,
            ))),
            ..Default::default()
        };
        // A per-clip static non-neutral scale wins the 3-way merge over the
        // track-level `scale_x` animation.
        let clip = Clip::new("a.mp4").with_scale(0.5);
        let layer = video_layer(&clip, 0, &automation, 1920, 1080, None, None);
        assert!(matches!(layer.scale_x, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
        assert!(matches!(layer.scale_y, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
    }

    #[test]
    fn video_layer_static_rotation_should_be_static() {
        let clip = Clip::new("a.mp4").with_rotation(30.0);
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.rotation, AnimatedValue::Static(v) if (v - 30.0).abs() < 1e-9));
    }

    #[test]
    fn video_layer_rotation_track_should_win() {
        // A per-clip rotation track is carried as a self-animating RotateAnimated and
        // the layer scalar neutralizes (ADR-0005).
        let mut clip = Clip::new("a.mp4");
        clip.rotation_track =
            Some(AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 90.0, Easing::Linear)));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.rotation, AnimatedValue::Static(v) if v.abs() < 1e-9));
        assert!(
            layer
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::RotateAnimated { .. }))
        );
    }

    #[test]
    fn realtime_descriptor_should_share_scale_and_rotation_with_export() {
        // Preview uses the same `video_transform`, so per-clip scale/rotation reach
        // preview identically to export.
        let clip = Clip::new("a.mp4").with_scale(0.5).with_rotation(45.0);
        let d = realtime_descriptor(&clip, &no_anim(), 1920, 1080);
        assert!(matches!(d.scale_x, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
        assert!(matches!(d.scale_y, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
        assert!(matches!(d.rotation, AnimatedValue::Static(v) if (v - 45.0).abs() < 1e-9));
    }
}
