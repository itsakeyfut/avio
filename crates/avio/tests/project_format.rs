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
use ff_format::{Rational, Tempo};

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// A document worth round-tripping: all three source kinds, a trim, an offset, a
/// transition and a second track, so a field dropped in the envelope is visible.
fn sample() -> Timeline {
    Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.into())
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
    assert_eq!(after.frame_rate(), before.frame_rate());
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
    assert_eq!(v0.frame_rate(), 30.into());
    assert_eq!(v0.video_tracks().len(), 1);
    assert_eq!(v0.video_tracks()[0].clips.len(), 1);
    assert!(matches!(
        v0.video_tracks()[0].clips[0].source,
        ClipSource::File(_)
    ));
    assert_eq!(v0.video_tracks()[0].clips[0].out_point, Some(s(2.0)));

    // v1: the envelope, same model. Its decimal `30.0` migrates to the ratio `30/1`.
    let v1 = Project::from_json_str(&fixture("v1.json"))
        .unwrap()
        .into_timeline();
    assert_eq!(v1.canvas_width(), 640);
    assert_eq!(v1.frame_rate(), 30.into());
    assert_eq!(v1.video_tracks()[0].clips[0].out_point, Some(s(2.0)));

    // v2: the rate is already a ratio, so the chain has nothing left to do.
    let v2 = Project::from_json_str(&fixture("v2.json"))
        .unwrap()
        .into_timeline();
    assert_eq!(v2.canvas_width(), 640);
    assert_eq!(v2.frame_rate(), 30.into());
    assert_eq!(v2.video_tracks()[0].clips[0].out_point, Some(s(2.0)));
}

/// The other half: a project that *does* carry a tempo survives being written and read.
///
/// The fixture test below covers a document with no tempo, and `Rational`'s own equality
/// cross-multiplies, so a tempo whose numerator and denominator were both mangled in the
/// same ratio would still compare equal. This asserts the components, at a tempo whose
/// denominator is not 1 so both carry information (#1914).
#[test]
fn a_tempo_should_survive_saving_and_loading() {
    // 93.75 BPM is 375/4, which no decimal holds exactly.
    let tempo = Tempo::new(Rational::new(375, 4)).expect("93.75 is a tempo");
    let before = Timeline::builder()
        .canvas(640, 360)
        .frame_rate(Rational::new(30, 1))
        .tempo(tempo)
        .video_track(vec![Clip::new("input.mp4").trim(Duration::ZERO, s(2.0))])
        .build()
        .expect("a timeline whose clip is never opened still builds");

    let text = Project::new(before).to_json_string().expect("serialises");
    let after = Project::from_json_str(&text)
        .expect("deserialises")
        .into_timeline();

    let bpm = after.tempo().expect("the tempo came back").bpm();
    assert_eq!(
        (bpm.num(), bpm.den()),
        (375, 4),
        "the ratio has to come back as itself, not as a decimal that approximates it"
    );
}

/// A field added after a format version shipped reads back as absent rather than failing.
///
/// `tempo` was added to `Timeline` in #1914 without moving `PROJECT_FORMAT_VERSION`,
/// because `serde(default)` means a version 2 document simply has no tempo. `v2.json`
/// predates the field, so it is the fixture that proves it: without the attribute the
/// deserialisation fails outright, which is the failure a user would meet on opening an
/// older project.
#[test]
fn a_document_written_before_the_tempo_existed_should_load_without_one() {
    let loaded = Project::from_json_str(&fixture("v2.json"))
        .expect("a version 2 document still loads after a field was added")
        .into_timeline();

    assert!(
        loaded.tempo().is_none(),
        "a project that never had a tempo must not acquire one"
    );
    // And the rest of the model is unaffected, so the absent field is the only difference.
    assert_eq!(loaded.frame_rate(), 30.into());
    assert_eq!(loaded.canvas_width(), 640);
}

/// The promise this change exists for, end to end: a project authored at a broadcast rate
/// is written and read back holding that exact ratio.
///
/// `30/1` round-trips under any plausible mistake in the ratio's serialised shape, so the
/// rate here is one whose numerator and denominator are both load-bearing.
#[test]
fn a_broadcast_rate_should_survive_saving_and_loading() {
    let before = Timeline::builder()
        .canvas(640, 360)
        .frame_rate(Rational::new(30_000, 1001))
        .video_track(vec![Clip::new("input.mp4").trim(Duration::ZERO, s(2.0))])
        .build()
        .expect("a timeline whose clip is never opened still builds");

    let text = Project::new(before).to_json_string().expect("serialises");
    let after = Project::from_json_str(&text)
        .expect("deserialises")
        .into_timeline();

    let rate = after.frame_rate();
    assert_eq!(
        (rate.num(), rate.den()),
        (30_000, 1001),
        "the ratio has to come back as itself, not as a decimal that approximates it"
    );
}

/// The branch an integer rate cannot test: a version 1 document that stored `29.97` has
/// to come back as `30000/1001`, not as the decimal read literally.
///
/// Someone who wrote `29.97` was cutting NTSC material, because that is what the number
/// means here. Reading it as `2997/100` would pin the project to a grid 0.1% off, and the
/// frame-exact addressing this change exists for is what would then make that visible
/// (#1947).
#[test]
fn a_version_one_decimal_broadcast_rate_should_load_as_its_standard_ratio() {
    let loaded = Project::from_json_str(&fixture("v1-ntsc.json"))
        .unwrap()
        .into_timeline();
    let rate = loaded.frame_rate();
    assert_eq!(
        (rate.num(), rate.den()),
        (30000, 1001),
        "29.97 names 30000/1001; a literal reading would give 2997/100"
    );
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
