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
use std::time::Duration;

use ff_filter::{AnimatedValue, AnimationTrack, AudioTrack, FilterStep, ProxySource, VideoLayer};
use ff_format::ChannelLayout;

use crate::clip::Clip;

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

/// Derives the export [`VideoLayer`] for one clip.
///
/// `prev_end` is the preceding clip's end-seconds on this track (for the
/// cross-fade offset); `None` marks the track's first clip — a transition on it
/// is ignored with a warning. `proxy` is the caller-probed proxy source.
pub(crate) fn video_layer(
    clip: &Clip,
    track_idx: usize,
    animations: &HashMap<String, AnimationTrack<f64>>,
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
    if clip.timeline_offset > Duration::ZERO {
        layer_effects.push(FilterStep::OffsetPts {
            seconds: clip.timeline_offset.as_secs_f64(),
        });
    }
    if (clip.speed - 1.0).abs() > 1e-9 {
        layer_effects.push(FilterStep::Speed { factor: clip.speed });
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

    // Per-clip opacity/position: an explicit keyframe track wins, then a static
    // non-neutral value, then any track-level animation.
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

    VideoLayer {
        source: clip.source.clone(),
        proxy,
        input_format: None,
        x,
        y,
        scale_x: track_animation(animations, "video", track_idx, "scale_x", 1.0),
        scale_y: track_animation(animations, "video", track_idx, "scale_y", 1.0),
        rotation: track_animation(animations, "video", track_idx, "rotation", 0.0),
        opacity,
        blend_mode: clip.blend_mode,
        composite_op: clip.composite_op,
        effects: layer_effects,
    }
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
    // A per-clip volume track wins over the static volume_db, which overrides
    // the track-level animation.
    let volume = match &clip.volume_track {
        Some(track) => AnimatedValue::Track(track.clone()),
        None if clip.volume_db != 0.0 => AnimatedValue::Static(clip.volume_db),
        None => track_animation(animations, "audio", track_idx, "volume", 0.0),
    };

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
    if clip.timeline_offset > Duration::ZERO {
        // `as_millis()` matches the old inline `adelay` (integer ms); offset
        // magnitudes are far below f64's exact-integer range.
        #[allow(clippy::cast_precision_loss)]
        let ms = clip.timeline_offset.as_millis() as f64;
        effects.push(FilterStep::AudioDelay { ms });
    }
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
                    clip.source.display(),
                );
            }
            None => {
                log::warn!(
                    "cannot determine clip duration — skipping fade_out for {}",
                    clip.source.display(),
                );
            }
        }
    }
    // Caller-attached per-clip audio effects run last.
    effects.extend(clip.audio_effects.iter().cloned());

    AudioTrack {
        source: clip.source.clone(),
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

    // ── video_layer ─────────────────────────────────────────────────────────

    #[test]
    fn video_layer_trim_should_lead_with_trim_and_resetpts() {
        let clip = Clip::new("a.mp4").trim(Duration::from_secs(1), Duration::from_secs(3));
        let layer = video_layer(&clip, 0, &no_anim(), None, None);
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
        let layer = video_layer(&clip, 0, &no_anim(), None, None);
        assert!(layer.effects.iter().any(
            |s| matches!(s, FilterStep::OffsetPts { seconds } if (seconds - 2.0).abs() < 1e-9)
        ));
    }

    #[test]
    fn video_layer_speed_should_emit_speed() {
        let clip = Clip::new("a.mp4").with_speed(2.0);
        let layer = video_layer(&clip, 0, &no_anim(), None, None);
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
        let layer = video_layer(&clip, 0, &no_anim(), Some(4.0), None);
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
        let layer = video_layer(&clip, 0, &no_anim(), None, None);
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
        let layer = video_layer(&clip, 0, &no_anim(), None, None);
        assert!(matches!(layer.opacity, AnimatedValue::Track(_)));
    }

    #[test]
    fn video_layer_static_opacity_should_be_animatedvalue_static() {
        let clip = Clip::new("a.mp4").with_opacity(0.5);
        let layer = video_layer(&clip, 0, &no_anim(), None, None);
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
        let layer = video_layer(&clip, 0, &anims, None, None);
        assert!(matches!(layer.opacity, AnimatedValue::Track(_)));
    }

    #[test]
    fn video_layer_should_pass_through_blend_and_composite() {
        use ff_filter::{BlendMode, CompositeOp};
        let mut clip = Clip::new("a.mp4");
        clip.blend_mode = BlendMode::Multiply;
        clip.composite_op = CompositeOp::Under;
        let layer = video_layer(&clip, 0, &no_anim(), None, None);
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
        let layer = video_layer(&clip, 0, &no_anim(), Some(10.0), None);
        let e = &layer.effects;
        assert!(matches!(e[0], FilterStep::Trim { .. }));
        assert!(matches!(e[1], FilterStep::ResetPts));
        assert!(matches!(e[2], FilterStep::OffsetPts { .. }));
        assert!(matches!(e[3], FilterStep::Speed { .. }));
        assert!(matches!(e[4], FilterStep::Eq { .. }));
        assert!(matches!(e[5], FilterStep::XFade { .. }));
    }

    // ── audio_track ─────────────────────────────────────────────────────────

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
}
