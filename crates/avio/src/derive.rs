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

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use ff_filter::{
    AnimatedValue, AnimationTrack, AudioTrack, BlendMode, CompositeOp, FilterStep, LayerSource,
    ProxySource, RealtimeLayerDescriptor, ScaleAlgorithm, VideoLayer,
};
use ff_format::ChannelLayout;

use crate::clip::{Clip, ClipSource, FitMode};

/// Resolves a track-level animation: `AnimatedValue::Track` when the map holds a
/// track for `"{prefix}_{track_idx}_{prop}"`, else `Static(default)`.
fn track_animation(
    animations: &HashMap<String, AnimationTrack<f64>>,
    prefix: &str,
    track_idx: usize,
    prop: &str,
    default: f64,
) -> AnimatedValue<f64> {
    animations
        .get(&format!("{prefix}_{track_idx}_{prop}"))
        .map_or(AnimatedValue::Static(default), |t| {
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
fn video_transform(
    clip: &Clip,
    track_idx: usize,
    animations: &HashMap<String, AnimationTrack<f64>>,
) -> VideoTransform {
    #[allow(clippy::float_cmp)]
    let opacity = if let Some(track) = &clip.opacity_track {
        AnimatedValue::Track(track.clone())
    } else if clip.opacity != 1.0 {
        AnimatedValue::Static(f64::from(clip.opacity))
    } else {
        track_animation(animations, "video", track_idx, "opacity", 1.0)
    };
    #[allow(clippy::float_cmp)]
    let x = if let Some(track) = &clip.x_track {
        AnimatedValue::Track(track.clone())
    } else if clip.x != 0.0 {
        AnimatedValue::Static(clip.x)
    } else {
        track_animation(animations, "video", track_idx, "x", 0.0)
    };
    #[allow(clippy::float_cmp)]
    let y = if let Some(track) = &clip.y_track {
        AnimatedValue::Track(track.clone())
    } else if clip.y != 0.0 {
        AnimatedValue::Static(clip.y)
    } else {
        track_animation(animations, "video", track_idx, "y", 0.0)
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
        track_animation(animations, "video", track_idx, "scale_x", 1.0)
    };
    #[allow(clippy::float_cmp)]
    let scale_y = if let Some(track) = &clip.scale_track {
        AnimatedValue::Track(track.clone())
    } else if clip.scale != 1.0 {
        AnimatedValue::Static(clip.scale)
    } else {
        track_animation(animations, "video", track_idx, "scale_y", 1.0)
    };
    #[allow(clippy::float_cmp)]
    let rotation = if let Some(track) = &clip.rotation_track {
        AnimatedValue::Track(track.clone())
    } else if clip.rotation != 0.0 {
        AnimatedValue::Static(clip.rotation)
    } else {
        track_animation(animations, "video", track_idx, "rotation", 0.0)
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
    animations: &HashMap<String, AnimationTrack<f64>>,
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
    let transform = video_transform(clip, track_idx, animations);
    VideoLayer {
        source,
        proxy,
        x: transform.x,
        y: transform.y,
        scale_x: transform.scale_x,
        scale_y: transform.scale_y,
        rotation: transform.rotation,
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
    track_idx: usize,
    animations: &HashMap<String, AnimationTrack<f64>>,
    canvas_width: u32,
    canvas_height: u32,
) -> RealtimeLayerDescriptor {
    let transform = video_transform(clip, track_idx, animations);
    // Frame to the canvas (same step as the export path) before the colour chain,
    // so preview and export share one framing interpretation.
    let mut effects = Vec::new();
    if let Some(step) = fit_step(clip, canvas_width, canvas_height) {
        effects.push(step);
    }
    effects.extend(clip.video_effect_chain());
    RealtimeLayerDescriptor {
        effects,
        opacity: transform.opacity,
        x: transform.x,
        y: transform.y,
        scale_x: transform.scale_x,
        scale_y: transform.scale_y,
        rotation: transform.rotation,
        blend_mode: transform.blend_mode,
        composite_op: transform.composite_op,
    }
}

/// The 3-way-merged audio volume (dB): a per-clip volume track wins, then a static
/// non-zero `volume_db`, then the timeline `audio_{idx}_volume` automation. This is
/// the single interpretation the export [`audio_track`] and the preview audio
/// projection ([`Timeline::to_scene`](crate::Timeline::to_scene)) share.
#[allow(clippy::float_cmp)]
pub(crate) fn audio_volume(
    clip: &Clip,
    track_idx: usize,
    animations: &HashMap<String, AnimationTrack<f64>>,
) -> AnimatedValue<f64> {
    match &clip.volume_track {
        Some(track) => AnimatedValue::Track(track.clone()),
        None if clip.volume_db != 0.0 => AnimatedValue::Static(clip.volume_db),
        None => track_animation(animations, "audio", track_idx, "volume", 0.0),
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
    track_idx: usize,
    animations: &HashMap<String, AnimationTrack<f64>>,
    fade_out_eff_dur: Option<Duration>,
) -> AudioTrack {
    let volume = audio_volume(clip, track_idx, animations);
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
        pan: track_animation(animations, "audio", track_idx, "pan", 0.0),
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

    fn no_anim() -> HashMap<String, AnimationTrack<f64>> {
        HashMap::new()
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
        let mut anims = HashMap::new();
        anims.insert(
            "video_0_opacity".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 1.0, Easing::Linear)),
        );
        let clip = Clip::new("a.mp4"); // opacity defaults to 1.0 (neutral)
        let layer = video_layer(&clip, 0, &anims, 1920, 1080, None, None);
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
        clip.brightness = 0.5; // makes `video_effect_chain` emit a trailing Eq step
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
        let mut clip = Clip::new("a.mp4").with_speed(2.0).with_fit(FitMode::Fill);
        clip.brightness = 0.5; // trailing Eq via `video_effect_chain`
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
        let d = realtime_descriptor(&clip, 0, &no_anim(), 1920, 1080);
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
        let d = realtime_descriptor(&clip, 0, &no_anim(), 1920, 1080);
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
        let d = realtime_descriptor(&clip, 0, &no_anim(), 0, 0);
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
        let track = audio_track(&clip, 0, &no_anim(), None);
        assert!(matches!(track.volume, AnimatedValue::Track(_)));
    }

    #[test]
    fn audio_track_static_volume_db_should_be_static() {
        let clip = Clip::new("a.mp3").volume(-6.0);
        let track = audio_track(&clip, 0, &no_anim(), None);
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
        let track = audio_track(&clip, 0, &no_anim(), Some(Duration::from_secs(4)));
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
        let track = audio_track(&clip, 0, &no_anim(), Some(Duration::from_secs(3)));
        assert!(
            !track
                .effects
                .iter()
                .any(|s| matches!(s, FilterStep::AFadeOut { .. }))
        );
    }

    #[test]
    fn audio_track_pan_should_come_from_track_animation() {
        let mut anims = HashMap::new();
        anims.insert(
            "audio_0_pan".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
        );
        let clip = Clip::new("a.mp3");
        let track = audio_track(&clip, 0, &anims, None);
        assert!(matches!(track.pan, AnimatedValue::Track(_)));
    }

    #[test]
    fn audio_track_static_pitch_should_emit_pitch_shift() {
        let clip = Clip::new("a.mp3").with_pitch(4.0);
        let track = audio_track(&clip, 0, &no_anim(), None);
        assert!(track.effects.iter().any(|s| matches!(
            s,
            FilterStep::PitchShift { semitones } if (semitones - 4.0).abs() < 1e-6
        )));
    }

    #[test]
    fn audio_track_pitch_track_should_emit_pitch_shift_at_t0() {
        // A per-clip pitch track wins over the static pitch and renders at its
        // `t=0` value (per-sample automation is deferred; see ADR-0002).
        let clip = Clip::new("a.mp3").with_pitch(1.0).with_pitch_track(
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 5.0, Easing::Linear)),
        );
        let track = audio_track(&clip, 0, &no_anim(), None);
        assert!(track.effects.iter().any(|s| matches!(
            s,
            FilterStep::PitchShift { semitones } if (semitones - 5.0).abs() < 1e-6
        )));
    }

    #[test]
    fn audio_track_zero_pitch_should_emit_no_pitch_shift() {
        let clip = Clip::new("a.mp3"); // pitch defaults to 0.0
        let track = audio_track(&clip, 0, &no_anim(), None);
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
        let mut anims = HashMap::new();
        anims.insert(
            "audio_0_volume".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, -3.0, Easing::Linear)),
        );
        let clip = Clip::new("a.mp3"); // neutral volume_db, no volume_track
        assert!(matches!(
            audio_volume(&clip, 0, &anims),
            AnimatedValue::Track(_)
        ));
    }

    #[test]
    fn audio_volume_static_should_win_over_timeline() {
        let mut anims = HashMap::new();
        anims.insert(
            "audio_0_volume".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, -3.0, Easing::Linear)),
        );
        let clip = Clip::new("a.mp3").volume(-6.0);
        assert!(matches!(
            audio_volume(&clip, 0, &anims),
            AnimatedValue::Static(x) if (x + 6.0).abs() < 1e-9
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
        let d = realtime_descriptor(&clip, 0, &no_anim(), 1920, 1080);
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
        let mut clip = Clip::new("a.mp4");
        clip.brightness = 0.5; // forces an Eq step in `video_effect_chain`
        let d = realtime_descriptor(&clip, 0, &no_anim(), 1920, 1080);
        assert!(d.effects.iter().any(|s| matches!(s, FilterStep::Eq { .. })));
    }

    #[test]
    fn realtime_descriptor_should_pick_up_timeline_scale_and_rotation() {
        let mut anims = HashMap::new();
        anims.insert(
            "video_1_scale_x".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
        );
        anims.insert(
            "video_1_rotation".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 90.0, Easing::Linear)),
        );
        let clip = Clip::new("a.mp4");
        let d = realtime_descriptor(&clip, 1, &anims, 1920, 1080);
        assert!(matches!(d.scale_x, AnimatedValue::Track(_)));
        assert!(matches!(d.rotation, AnimatedValue::Track(_)));
        assert!(matches!(d.scale_y, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
    }

    #[test]
    fn realtime_descriptor_opacity_should_fall_back_to_timeline_animation() {
        // A neutral per-clip opacity (1.0) falls through to the timeline track.
        let mut anims = HashMap::new();
        anims.insert(
            "video_0_opacity".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 1.0, Easing::Linear)),
        );
        let clip = Clip::new("a.mp4");
        let d = realtime_descriptor(&clip, 0, &anims, 1920, 1080);
        assert!(matches!(d.opacity, AnimatedValue::Track(_)));
    }

    #[test]
    fn realtime_descriptor_static_opacity_should_win_over_timeline() {
        // A per-clip static non-neutral opacity wins the 3-way merge.
        let mut anims = HashMap::new();
        anims.insert(
            "video_0_opacity".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.2, Easing::Linear)),
        );
        let clip = Clip::new("a.mp4").with_opacity(0.5);
        let d = realtime_descriptor(&clip, 0, &anims, 1920, 1080);
        assert!(matches!(d.opacity, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
    }

    #[test]
    fn realtime_descriptor_should_carry_composite_op() {
        let mut clip = Clip::new("a.mp4");
        clip.composite_op = CompositeOp::Under;
        let d = realtime_descriptor(&clip, 0, &no_anim(), 1920, 1080);
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
        let mut clip = Clip::new("a.mp4");
        clip.scale_track =
            Some(AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 2.0, Easing::Linear)));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.scale_x, AnimatedValue::Track(_)));
        assert!(matches!(layer.scale_y, AnimatedValue::Track(_)));
    }

    #[test]
    fn video_layer_neutral_scale_should_fall_back_to_timeline_animation() {
        let mut anims = HashMap::new();
        anims.insert(
            "video_0_scale_x".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
        );
        let clip = Clip::new("a.mp4"); // scale defaults to 1.0 (neutral)
        let layer = video_layer(&clip, 0, &anims, 1920, 1080, None, None);
        assert!(matches!(layer.scale_x, AnimatedValue::Track(_)));
        // scale_y has no timeline animation -> the default static 1.0.
        assert!(matches!(layer.scale_y, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9));
    }

    #[test]
    fn video_layer_static_scale_should_win_over_timeline_animation() {
        let mut anims = HashMap::new();
        anims.insert(
            "video_0_scale_x".to_string(),
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.25, Easing::Linear)),
        );
        // A per-clip static non-neutral scale wins the 3-way merge over the
        // timeline `scale_x` animation.
        let clip = Clip::new("a.mp4").with_scale(0.5);
        let layer = video_layer(&clip, 0, &anims, 1920, 1080, None, None);
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
        let mut clip = Clip::new("a.mp4");
        clip.rotation_track =
            Some(AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 90.0, Easing::Linear)));
        let layer = video_layer(&clip, 0, &no_anim(), 1920, 1080, None, None);
        assert!(matches!(layer.rotation, AnimatedValue::Track(_)));
    }

    #[test]
    fn realtime_descriptor_should_share_scale_and_rotation_with_export() {
        // Preview uses the same `video_transform`, so per-clip scale/rotation reach
        // preview identically to export.
        let clip = Clip::new("a.mp4").with_scale(0.5).with_rotation(45.0);
        let d = realtime_descriptor(&clip, 0, &no_anim(), 1920, 1080);
        assert!(matches!(d.scale_x, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
        assert!(matches!(d.scale_y, AnimatedValue::Static(v) if (v - 0.5).abs() < 1e-9));
        assert!(matches!(d.rotation, AnimatedValue::Static(v) if (v - 45.0).abs() < 1e-9));
    }
}
