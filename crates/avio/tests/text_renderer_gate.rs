//! A text clip is refused before the graph is built when the build cannot draw it (#1809).
//!
//! `drawtext` needs freetype, which several common `FFmpeg` packages are built
//! without, and the documented Windows install was one of them. The failure used to
//! arrive from inside the composition as `failed to build text drawtext layer=N`,
//! which names neither what is missing nor what to install.
//!
//! Both branches assert something: where the filter is present the render must not
//! be refused for this reason, and where it is absent it must be, so the test is
//! never vacuous on either kind of build.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;

use std::time::Duration;

use avio::{Clip, Color, EncoderConfig, TextSpec, Timeline, TimelineError};
use fixtures::{FileGuard, test_output_path};

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// A one-second text clip over a solid background.
fn text_timeline() -> Timeline {
    Timeline::builder()
        .canvas(320, 180)
        .frame_rate(30.into())
        .video_track(vec![
            Clip::solid(Color::rgb(20, 20, 20)).trim(Duration::ZERO, s(1.0)),
        ])
        .video_track(vec![
            Clip::text(TextSpec::new("TEST")).trim(Duration::ZERO, s(1.0)),
        ])
        .build()
        .unwrap()
}

#[test]
fn rendering_a_text_clip_should_be_refused_by_name_when_the_build_cannot_draw_it() {
    let out = test_output_path("text_gate.mp4");
    let _g = FileGuard::new(out.clone());
    let result = text_timeline().render(&out, EncoderConfig::builder().build());

    if avio::text_rendering_available() {
        assert!(
            !matches!(result, Err(TimelineError::TextRendererUnavailable)),
            "this build has the text filter, so the render must not be refused for it"
        );
        return;
    }

    let Err(e) = result else {
        panic!("a build without the text filter cannot have rendered a text clip");
    };
    assert!(
        matches!(e, TimelineError::TextRendererUnavailable),
        "expected the named refusal, got: {e}"
    );
    // The message is the point: it has to name the filter and what to install.
    let message = e.to_string();
    assert!(
        message.contains(avio::TEXT_FILTER),
        "the refusal does not name the missing filter: {message}"
    );
    assert!(
        message.contains("freetype"),
        "the refusal does not say what to install: {message}"
    );
}

#[test]
fn a_timeline_without_text_should_never_be_refused_for_the_text_renderer() {
    let out = test_output_path("text_gate_none.mp4");
    let _g = FileGuard::new(out.clone());
    let timeline = Timeline::builder()
        .canvas(320, 180)
        .frame_rate(30.into())
        .video_track(vec![
            Clip::solid(Color::rgb(20, 20, 20)).trim(Duration::ZERO, s(1.0)),
        ])
        .build()
        .unwrap();
    // Build-independent: whatever else this environment cannot do, it is not this.
    assert!(
        !matches!(
            timeline.render(&out, EncoderConfig::builder().build()),
            Err(TimelineError::TextRendererUnavailable)
        ),
        "a timeline with no text clip must never hit the text-renderer gate"
    );
}
