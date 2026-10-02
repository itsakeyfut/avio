//! The project file carries a format version, and an older document still loads (#1905).
//!
//! Every test here is pure JSON and model work: no FFmpeg, no filters, no assets, so they
//! run on CI's minimal build as well as a full one. The render-equivalence check the issue
//! also asks for is not here on purpose, because with the chain one step long there is
//! nothing to migrate for it to compare; it lands with the first release that restructures
//! serialised state.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use avio::{
    Clip, ClipSource, PROJECT_FORMAT_VERSION, Project, ProjectError, Timeline, Track,
    XfadeTransition,
};
use ff_format::{Color, TextSpec};

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// A document worth round-tripping: all three source kinds, a trim, an offset, a
/// transition and a second track, so a field dropped in the envelope is visible.
fn sample() -> Timeline {
    Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::new("a.mp4").trim(Duration::ZERO, s(2.0)),
            Clip::new("b.mp4")
                .trim(Duration::ZERO, s(2.0))
                .offset(s(2.0))
                .with_transition(XfadeTransition::Fade, s(0.5)),
            Clip::text(TextSpec::new("title")).trim(Duration::ZERO, s(1.0)),
            Clip::solid(Color::WHITE).trim(Duration::ZERO, s(1.0)),
        ])
        .audio_track_with(Track::new(vec![
            Clip::new("music.mp3").trim(Duration::ZERO, s(4.0)),
        ]))
        .build()
        .unwrap()
}

fn fixture(name: &str) -> String {
    let path = std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/project"
    ))
    .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {} could not be read: {e}", path.display()))
}

#[test]
fn a_project_should_round_trip_through_json() {
    let before = sample();
    let text = Project::new(before.clone()).to_json_string().unwrap();
    let after = Project::from_json_str(&text).unwrap().into_timeline();

    assert_eq!(after.canvas_width(), before.canvas_width());
    assert_eq!(after.canvas_height(), before.canvas_height());
    assert!((after.frame_rate() - before.frame_rate()).abs() < f64::EPSILON);
    assert_eq!(after.video_tracks().len(), before.video_tracks().len());
    assert_eq!(after.audio_tracks().len(), before.audio_tracks().len());

    let (va, vb) = (&after.video_tracks()[0], &before.video_tracks()[0]);
    assert_eq!(va.clips.len(), vb.clips.len());
    for (a, b) in va.clips.iter().zip(vb.clips.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.offset, b.offset);
        assert_eq!(a.in_point, b.in_point);
        assert_eq!(a.out_point, b.out_point);
        assert_eq!(a.transition, b.transition);
        assert_eq!(
            std::mem::discriminant(&a.source),
            std::mem::discriminant(&b.source),
            "the source kind must survive the round trip"
        );
    }
}

#[test]
fn a_saved_project_should_carry_the_format_version() {
    let text = Project::new(sample()).to_json_string().unwrap();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(
        document
            .get("format_version")
            .and_then(serde_json::Value::as_u64),
        Some(u64::from(PROJECT_FORMAT_VERSION)),
        "the envelope is what makes the document readable in three releases' time"
    );
    assert!(
        document.get("timeline").is_some(),
        "the model sits under `timeline`, not at the root"
    );
}

/// The documented distinction: a bare serialised `Timeline` is not a project file. It
/// still loads, as version 0, which is what documents written before the envelope are.
#[test]
fn a_bare_serialised_timeline_should_load_as_version_zero() {
    let bare = serde_json::to_string(&sample()).unwrap();
    assert!(
        !bare.contains("format_version"),
        "a bare model carries no version, which is the thing this issue is about"
    );

    let loaded = Project::from_json_str(&bare).unwrap().into_timeline();

    assert_eq!(loaded.canvas_width(), 1920);
    assert_eq!(loaded.video_tracks()[0].clips.len(), 4);
}

#[test]
fn every_committed_fixture_should_load_to_its_expected_model() {
    // v0: no envelope, the shape a release before this one wrote.
    let v0 = Project::from_json_str(&fixture("v0.json"))
        .unwrap()
        .into_timeline();
    assert_eq!(v0.canvas_width(), 640);
    assert_eq!(v0.canvas_height(), 360);
    assert!((v0.frame_rate() - 30.0).abs() < f64::EPSILON);
    assert_eq!(v0.video_tracks().len(), 1);
    assert_eq!(v0.video_tracks()[0].clips.len(), 1);
    assert!(matches!(
        v0.video_tracks()[0].clips[0].source,
        ClipSource::File(_)
    ));
    assert_eq!(v0.video_tracks()[0].clips[0].out_point, Some(s(2.0)));

    // v1: the envelope, same model.
    let v1 = Project::from_json_str(&fixture("v1.json"))
        .unwrap()
        .into_timeline();
    assert_eq!(v1.canvas_width(), 640);
    assert_eq!(v1.video_tracks()[0].clips[0].out_point, Some(s(2.0)));
}

#[test]
fn a_document_from_a_newer_version_should_be_refused() {
    let newer = format!(
        r#"{{ "format_version": {}, "timeline": {} }}"#,
        PROJECT_FORMAT_VERSION + 1,
        serde_json::to_string(&sample()).unwrap()
    );

    let err = Project::from_json_str(&newer).unwrap_err();

    let ProjectError::VersionTooNew { document, reader } = &err else {
        panic!("expected VersionTooNew, got {err:?}");
    };
    assert_eq!(*document, PROJECT_FORMAT_VERSION + 1);
    assert_eq!(*reader, PROJECT_FORMAT_VERSION);
    let message = err.to_string();
    assert!(
        message.contains(&(PROJECT_FORMAT_VERSION + 1).to_string())
            && message.contains(&PROJECT_FORMAT_VERSION.to_string()),
        "both versions belong in the message a host shows, got {message}"
    );
}

#[test]
fn a_malformed_document_should_be_refused_with_a_reason() {
    let err = Project::from_json_str("{ not json").unwrap_err();
    assert!(matches!(err, ProjectError::Malformed { .. }), "got {err:?}");
}

#[test]
fn saving_and_loading_a_file_should_round_trip() {
    let dir = std::env::temp_dir().join("avio_project_format_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("song.avio");
    let _ = std::fs::remove_file(&path);

    Project::new(sample()).save(&path).unwrap();
    let loaded = Project::load(&path).unwrap().into_timeline();

    assert_eq!(loaded.canvas_width(), 1920);
    assert_eq!(loaded.video_tracks()[0].clips.len(), 4);

    let _ = std::fs::remove_file(&path);
}
