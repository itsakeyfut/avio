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
    Track, VideoProperty, XfadeTransition, apply,
};
use ff_filter::{FilterStep, PitchAlgo};
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
            1,
            VideoProperty::ScaleX,
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
fn clip_effects_should_round_trip_through_serde() {
    // #1452: FilterStep effect chains now persist. Exercise a video chain and an
    // audio chain, including a PitchShift carrying its `algo` backend selector.
    let clip = Clip::new("v.mp4")
        .with_video_effect(FilterStep::Hue { degrees: 30.0 })
        .with_video_effect(FilterStep::HFlip)
        .with_audio_effect(FilterStep::Volume(-3.0))
        .with_audio_effect(FilterStep::PitchShift {
            semitones: 4.0,
            algo: PitchAlgo::Rubberband,
        });

    let json = serde_json::to_string(&clip).unwrap();
    let back: Clip = serde_json::from_str(&json).unwrap();

    // FilterStep has no PartialEq (its composition variants carry a builder), so
    // compare the re-serialized Values rather than the structs.
    let v1: serde_json::Value = serde_json::from_str(&json).unwrap();
    let v2: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&back).unwrap()).unwrap();
    assert_eq!(v1, v2, "effect chains must round-trip through serde");

    assert_eq!(
        back.video_effects.len(),
        2,
        "video effects survive the load"
    );
    assert!(
        matches!(back.video_effects[0], FilterStep::Hue { degrees } if (degrees - 30.0).abs() < 1e-6)
    );
    assert!(matches!(back.video_effects[1], FilterStep::HFlip));
    assert_eq!(
        back.audio_effects.len(),
        2,
        "audio effects survive the load"
    );
    assert!(
        matches!(
            back.audio_effects[1],
            FilterStep::PitchShift {
                algo: PitchAlgo::Rubberband,
                ..
            }
        ),
        "the PitchShift algo backend survives the round-trip"
    );
}

#[test]
fn timeline_audio_filter_should_round_trip_through_serde() {
    let original = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .audio_track(vec![Clip::new("music.mp3")])
        .audio_filter(vec![
            FilterStep::Volume(-2.0),
            FilterStep::PitchShift {
                semitones: -3.0,
                algo: PitchAlgo::Signal,
            },
        ])
        .build()
        .unwrap();

    let json = serde_json::to_string(&original).unwrap();
    assert!(
        json.contains("audio_filter") && json.contains("PitchShift"),
        "the audio_filter chain must be serialized: {json}"
    );
    let back: Timeline = serde_json::from_str(&json).unwrap();

    // A dropped chain would make the re-serialized Value differ from the original.
    let v1: serde_json::Value = serde_json::from_str(&json).unwrap();
    let v2: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&back).unwrap()).unwrap();
    assert_eq!(
        v1, v2,
        "timeline audio_filter must round-trip through serde"
    );
}

#[test]
fn timeline_track_audio_effects_should_round_trip_through_serde() {
    // #1592: `Track.audio_effects` was `#[serde(skip)]`, so a saved project dropped
    // its track-level (pre-mix) audio chain on reload. It now persists like the
    // timeline-level `audio_filter` and the per-clip `Clip.audio_effects`.
    let track = Track::new(vec![Clip::new("music.mp3")]).audio_effects(vec![
        FilterStep::Volume(-6.0),
        FilterStep::PitchShift {
            semitones: 3.0,
            algo: PitchAlgo::Signal,
        },
    ]);
    let original = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .audio_track_with(track)
        .build()
        .unwrap();

    let json = serde_json::to_string(&original).unwrap();
    assert!(
        json.contains("audio_effects") && json.contains("PitchShift"),
        "the track audio_effects chain must be serialized: {json}"
    );
    let back: Timeline = serde_json::from_str(&json).unwrap();

    // A dropped chain would make the re-serialized Value differ from the original.
    let v1: serde_json::Value = serde_json::from_str(&json).unwrap();
    let v2: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&back).unwrap()).unwrap();
    assert_eq!(v1, v2, "track audio_effects must round-trip through serde");

    // Non-vacuous: the chain and its variant payloads survive (empty before the fix).
    let effects = &back.audio_tracks()[0].audio_effects;
    assert_eq!(
        effects.len(),
        2,
        "both track audio effects survive the round-trip (previously dropped by serde(skip))"
    );
    assert!(matches!(effects[0], FilterStep::Volume(v) if (v + 6.0).abs() < 1e-9));
    assert!(matches!(
        effects[1],
        FilterStep::PitchShift {
            algo: PitchAlgo::Signal,
            ..
        }
    ));
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
