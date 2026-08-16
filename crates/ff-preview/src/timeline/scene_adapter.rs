//! Temporary `Timeline -> Scene` projection.
//!
//! This adapter lets `ff-preview` keep accepting a `ff_pipeline::Timeline` while
//! its runner consumes the primitive [`Scene`]. It is a **pure model
//! projection**: it reads the timeline's tracks and each clip's fields, does no
//! probing or I/O, and resolves nothing that needs the media (durations are
//! resolved later, in [`TimelinePlayer::open_scene`](super::TimelinePlayer::open_scene)).
//!
//! It moves to `avio` with the editing model in the relocation (#1329, slice C);
//! after that, `ff-preview` no longer depends on the model and this file is
//! deleted.

use std::time::Duration;

use ff_pipeline::Clip;
use ff_pipeline::timeline::Timeline;

use super::scene::{Scene, SceneAudioPlacement, SceneAudioTrack, ScenePlacement, SceneVideoTrack};

impl Scene {
    /// Projects a [`Timeline`] into a primitive [`Scene`] (no probing, no I/O).
    ///
    /// Video track `0` is the V1 base (crossfade transitions apply); tracks `1..`
    /// are overlays (transitions forced off, matching the compositor).
    pub(crate) fn from_timeline(timeline: &Timeline) -> Self {
        let video_tracks = timeline
            .video_tracks()
            .iter()
            .enumerate()
            .map(|(track_idx, track)| SceneVideoTrack {
                placements: track
                    .iter()
                    .map(|clip| video_placement(clip, track_idx == 0))
                    .collect(),
            })
            .collect();

        let audio_tracks = timeline
            .audio_tracks()
            .iter()
            .map(|track| SceneAudioTrack {
                placements: track.iter().map(audio_placement).collect(),
            })
            .collect();

        Self {
            fps: timeline.frame_rate().max(1.0),
            canvas: timeline.explicit_canvas(),
            video_tracks,
            audio_tracks,
        }
    }
}

/// Projects one video clip. `is_base` selects the V1 base track, where a
/// crossfade transition contributes a `transition_dur`; overlays force zero.
fn video_placement(clip: &Clip, is_base: bool) -> ScenePlacement {
    let transition_dur = if is_base && clip.transition.is_some() {
        clip.transition_duration
    } else {
        Duration::ZERO
    };
    ScenePlacement {
        source: clip.source.clone(),
        timeline_offset: clip.timeline_offset,
        in_point: clip.in_point.unwrap_or(Duration::ZERO),
        out_point: clip.out_point,
        speed: clip.speed.max(0.01),
        transition_dur,
        opacity: clip.opacity.clamp(0.0, 1.0),
        layer: clip.realtime_layer_descriptor(),
        fade_in: clip.fade_in,
        fade_out: clip.fade_out,
        volume_db: clip.volume_db,
        volume_track: clip.volume_track.clone(),
    }
}

/// Projects one audio-only clip.
fn audio_placement(clip: &Clip) -> SceneAudioPlacement {
    SceneAudioPlacement {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ff_filter::{AnimationTrack, XfadeTransition};

    use super::*;

    #[test]
    fn scene_from_timeline_should_project_clip_fields() {
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
                // Overlay: a transition here must be projected as zero.
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

        let scene = Scene::from_timeline(&timeline);

        assert!((scene.fps - 30.0).abs() < f64::EPSILON);
        assert_eq!(scene.canvas, Some((1920, 1080)));
        assert_eq!(scene.video_tracks.len(), 2);
        assert_eq!(scene.audio_tracks.len(), 1);

        // ── V1 base, clip 0: resolved projections ──
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
        // The descriptor carries the same opacity the clip declared.
        assert!((base.layer.opacity - 0.5).abs() < f32::EPSILON);

        // ── V1 base, clip 1: transition duration is projected ──
        let base1 = &scene.video_tracks[0].placements[1];
        assert_eq!(base1.transition_dur, Duration::from_millis(750));

        // ── Overlay: transition forced to zero ──
        let overlay = &scene.video_tracks[1].placements[0];
        assert_eq!(
            overlay.transition_dur,
            Duration::ZERO,
            "overlay transitions must project as zero"
        );

        // ── Audio-only placement ──
        let audio = &scene.audio_tracks[0].placements[0];
        assert_eq!(audio.source.to_str(), Some("music.mp3"));
        assert_eq!(audio.fade_in, Duration::from_millis(200));
        assert_eq!(audio.fade_out, Duration::from_millis(300));
        assert!((audio.volume_db - (-6.0)).abs() < f64::EPSILON);
    }
}
