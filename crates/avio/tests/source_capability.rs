//! A source the render path cannot use is refused when the clip is built (#1850).
//!
//! `build` is where a rule that needs to read a file lives (ADR-0023): the edit path is
//! pure, so it cannot probe, and `validate` is advisory and does no file I/O. These
//! tests drive the real thing, with real media, through `TimelineBuilder::build`.
//!
//! The refusal a build with a missing decoder would produce is **not** tested here:
//! this build decodes everything the repository's assets use, so that path cannot be
//! reached. It is pinned by the unit tests on `source_serves_kind` in
//! `crates/avio/src/timeline.rs` instead.

#![allow(clippy::unwrap_used)]

use avio::{Clip, Timeline, TimelineError};
use std::path::PathBuf;

fn asset(rel: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join(rel)
}

fn video_asset() -> PathBuf {
    asset("assets/video/gameplay.mp4")
}

fn audio_asset() -> PathBuf {
    asset("assets/audio/konekonoosanpo.mp3")
}

/// Skips when the asset is missing or this build cannot open it at all.
///
/// The gate asks whether the *probe* works, never whether the refusal happened: gating
/// on the property under test would make these permanently green wherever the answer
/// went the other way.
fn probe_works(path: &PathBuf) -> bool {
    if !path.exists() {
        println!("Skipping: asset not found at {}", path.display());
        return false;
    }
    match avio::open(path) {
        Ok(_) => true,
        Err(e) => {
            println!("Skipping: this build cannot probe {}: {e}", path.display());
            false
        }
    }
}

/// An MP3 has no video stream, so a video track cannot use it. This is the file-backed
/// half of the kind rule, relocated here from #1927 because deciding it needs the file.
#[test]
fn build_should_refuse_an_audio_only_source_on_a_video_track() {
    let audio = audio_asset();
    if !probe_works(&audio) {
        return;
    }

    let result = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&audio)])
        .build();

    let err = result.unwrap_err();
    let TimelineError::SourceUnusable { path, reason } = &err else {
        panic!("expected SourceUnusable for an MP3 on a video track, got {err:?}");
    };
    assert!(
        path.contains("konekonoosanpo"),
        "the error should name the file, got {path}"
    );
    assert!(
        reason.contains("video"),
        "the error should say which kind is missing, got {reason}"
    );
}

/// The check must not become a whitelist. Media this build can actually use has to keep
/// building, which is the criterion that matters most: a late failure can be worked
/// around, a wrongly refused source cannot.
#[test]
fn build_should_accept_the_repository_assets() {
    let video = video_asset();
    let audio = audio_asset();
    if !probe_works(&video) || !probe_works(&audio) {
        return;
    }

    let result = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(&video)])
        .audio_track(vec![Clip::new(&audio)])
        .build();
    assert!(
        result.is_ok(),
        "the repository's own assets must build: {result:?}"
    );
}

/// A project reopened with a moved source is a relink case, not a broken document, so a
/// missing file is left to fail later as it always has. Pinned because turning this into
/// a build error would break every timeline in the workspace that names a placeholder.
#[test]
fn build_should_accept_a_missing_source() {
    let result = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![Clip::new("no-such-file-9999.mp4")])
        .build();
    assert!(
        result.is_ok(),
        "a missing source is a relink case, not a build failure: {result:?}"
    );
}

/// Forty clips on one source still build.
///
/// That each distinct path is probed **once** is a property of
/// `required_source_kinds`, asserted by the unit tests on it rather than by a clock
/// here: a duration bound would be the flaky kind of test that passes locally and in
/// `Test` but fails under the instrumented Coverage job.
#[test]
fn build_should_accept_many_clips_on_one_source() {
    let video = video_asset();
    if !probe_works(&video) {
        return;
    }

    let clips: Vec<Clip> = (0..40).map(|_| Clip::new(&video)).collect();
    let result = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(clips)
        .build();
    assert!(result.is_ok(), "forty clips on one source: {result:?}");
}
