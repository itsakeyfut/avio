//! Integration tests for serde persistence of the editing document (#1426).
//!
//! Gated behind the `serde` feature (which implies `pipeline`). The round-trip
//! check compares parsed `serde_json::Value`s rather than raw JSON strings so it
//! is insensitive to `HashMap` key ordering (map keys normalise through the
//! `Value` representation).

// Built only under the `serde` feature (declared via `[[test]] required-features`).
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use avio::{
    AnimationTrack, BlendMode, Clip, ClipSource, Command, Easing, Keyframe, Marker, Timeline,
    XfadeTransition, apply,
};
use ff_filter::FilterStep;
use ff_format::{Color, TextSpec};

/// Builds a representative multi-track document exercising File/Text/Solid clips,
/// trims, offsets, a transition, blend mode, a clip-level animation track, and a
/// timeline-level animation map.
fn sample_timeline() -> Timeline {
    Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::new("intro.mp4")
                .trim(Duration::from_secs(1), Duration::from_secs(3))
                .offset(Duration::from_millis(500))
                .with_opacity(0.5)
                .with_volume_track(AnimationTrack::new().push(Keyframe::new(
                    Duration::ZERO,
                    1.0,
                    Easing::Linear,
                ))),
            Clip::new("main.mp4")
                .with_transition(XfadeTransition::Fade, Duration::from_millis(750))
                .with_blend_mode(BlendMode::Screen),
        ])
        .video_track(vec![
            Clip::text(TextSpec::new("Title")).trim(Duration::ZERO, Duration::from_secs(2)),
            Clip::solid(Color::rgb(255, 0, 0)).trim(Duration::ZERO, Duration::from_secs(1)),
        ])
        .audio_track(vec![
            Clip::new("music.mp3")
                .with_fade_in(Duration::from_millis(200))
                .volume(-6.0),
        ])
        .video_animation(
            "video_1_scale_x",
            AnimationTrack::new().push(Keyframe::new(Duration::ZERO, 0.5, Easing::Linear)),
        )
        .build()
        .unwrap()
}

#[test]
fn timeline_should_round_trip_through_serde() {
    let original = sample_timeline();

    let json = serde_json::to_string(&original).unwrap();
    let back: Timeline = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&back).unwrap();

    // Compare as parsed Values so HashMap key ordering does not matter.
    let v1: serde_json::Value = serde_json::from_str(&json).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
    assert_eq!(v1, v2, "document must round-trip through serde");

    // Structural + id-preservation checks on the deserialized document.
    assert_eq!(back.canvas_width(), 1920);
    assert_eq!(back.canvas_height(), 1080);
    assert!((back.frame_rate() - 30.0).abs() < f64::EPSILON);
    assert_eq!(back.video_tracks().len(), 2);
    assert_eq!(back.audio_tracks().len(), 1);

    let orig_v0 = &original.video_tracks()[0];
    let back_v0 = &back.video_tracks()[0];
    assert_eq!(back_v0.id, orig_v0.id, "track id must be preserved");
    assert_eq!(
        back_v0.clips[0].id, orig_v0.clips[0].id,
        "clip id must be preserved"
    );
    assert_eq!(
        back_v0.clips[1].transition,
        Some(XfadeTransition::Fade),
        "transition must survive the round-trip"
    );
    assert_eq!(back_v0.clips[1].blend_mode, BlendMode::Screen);

    // The id-allocation counters (`next_clip_id`/`next_track_id`) are serialized, so
    // a clip added after a round-trip gets a fresh, non-colliding id — the counter
    // did not reset to 1 on deserialize.
    let existing: Vec<_> = back
        .video_tracks()
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| c.id))
        .collect();
    let grown = apply(
        &back,
        &Command::AddClip {
            track: back_v0.id,
            clip: Box::new(Clip::new("added.mp4")),
        },
    )
    .unwrap();
    let new_id = grown.video_tracks()[0].clips.last().unwrap().id;
    assert!(
        !existing.contains(&new_id),
        "a clip added after load must get a fresh id (allocation counter preserved)"
    );
}

#[test]
fn clip_source_should_round_trip_each_variant() {
    // File
    let file = Clip::new("clip.mp4");
    let file_back: Clip = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
    assert!(matches!(file_back.source, ClipSource::File(ref p) if p.to_str() == Some("clip.mp4")));

    // Text (carries a ff-format TextSpec)
    let text = Clip::text(TextSpec::new("hello"));
    let text_back: Clip = serde_json::from_str(&serde_json::to_string(&text).unwrap()).unwrap();
    match text_back.source {
        ClipSource::Text(spec) => assert_eq!(spec.text, "hello"),
        other => panic!("expected Text, got {other:?}"),
    }

    // Solid (carries a ff-format Color)
    let solid = Clip::solid(Color::rgb(10, 20, 30));
    let solid_back: Clip = serde_json::from_str(&serde_json::to_string(&solid).unwrap()).unwrap();
    match solid_back.source {
        ClipSource::Solid(c) => assert_eq!(c, Color::rgb(10, 20, 30)),
        other => panic!("expected Solid, got {other:?}"),
    }
}

#[test]
fn clip_effects_should_be_omitted_from_serialization() {
    // FilterStep is not serializable yet, so video_effects/audio_effects are
    // skipped: they never appear in the JSON and deserialize to an empty vec.
    let clip = Clip::new("v.mp4").with_video_effect(FilterStep::Hue { degrees: 30.0 });
    assert_eq!(clip.video_effects.len(), 1);

    let json = serde_json::to_string(&clip).unwrap();
    assert!(
        !json.contains("video_effects"),
        "skipped field must not be serialized: {json}"
    );

    let back: Clip = serde_json::from_str(&json).unwrap();
    assert!(
        back.video_effects.is_empty(),
        "skipped field must deserialize to an empty vec"
    );
}

#[test]
fn markers_should_round_trip_through_serde() {
    let base = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![Clip::new("a.mp4")])
        .build()
        .unwrap();
    let with_markers = apply(
        &base,
        &Command::AddMarker {
            marker: Marker::new(Duration::from_secs(2))
                .with_name("chapter 1")
                .with_color(Color::rgb(255, 0, 0)),
        },
    )
    .unwrap();
    let with_markers = apply(
        &with_markers,
        &Command::AddMarker {
            marker: Marker::new(Duration::from_secs(5)).with_comment("note"),
        },
    )
    .unwrap();

    let json = serde_json::to_string(&with_markers).unwrap();
    let back: Timeline = serde_json::from_str(&json).unwrap();

    // Value comparison is insensitive to HashMap key ordering.
    let v1: serde_json::Value = serde_json::from_str(&json).unwrap();
    let v2: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&back).unwrap()).unwrap();
    assert_eq!(v1, v2, "markers must round-trip through serde");

    assert_eq!(back.markers().len(), 2, "both markers survive");
    assert_eq!(back.markers()[0].name.as_deref(), Some("chapter 1"));
    assert_eq!(back.markers()[0].pts, Duration::from_secs(2));
    assert_eq!(back.markers()[0].color, Some(Color::rgb(255, 0, 0)));
    assert_eq!(
        back.markers()[0].id,
        with_markers.markers()[0].id,
        "marker id is preserved"
    );
    assert_eq!(back.markers()[1].comment.as_deref(), Some("note"));
}

#[test]
fn groups_should_round_trip_through_serde() {
    let base = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::new("a.mp4"),
            Clip::new("b.mp4"),
            Clip::new("c.mp4"),
        ])
        .build()
        .unwrap();
    let a = base.video_tracks()[0].clips[0].id;
    let b = base.video_tracks()[0].clips[1].id;
    let grouped = apply(&base, &Command::GroupClips { clips: vec![a, b] }).unwrap();

    let json = serde_json::to_string(&grouped).unwrap();
    let back: Timeline = serde_json::from_str(&json).unwrap();

    // Value comparison is insensitive to HashMap key ordering.
    let v1: serde_json::Value = serde_json::from_str(&json).unwrap();
    let v2: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&back).unwrap()).unwrap();
    assert_eq!(v1, v2, "groups must round-trip through serde");

    let ga = back.video_tracks()[0].clips[0].group;
    let gb = back.video_tracks()[0].clips[1].group;
    let gc = back.video_tracks()[0].clips[2].group;
    assert!(ga.is_some() && ga == gb, "the link survives the round-trip");
    assert_eq!(gc, None, "a non-grouped clip stays ungrouped");

    // The group-id counter is serialized, so a group formed after load gets a
    // fresh, non-colliding id.
    let c = back.video_tracks()[0].clips[2].id;
    let regrouped = apply(&back, &Command::GroupClips { clips: vec![c] }).unwrap();
    let gc2 = regrouped.video_tracks()[0].clips[2].group;
    assert!(
        gc2.is_some() && gc2 != ga,
        "a new group after load gets a fresh id"
    );
}
