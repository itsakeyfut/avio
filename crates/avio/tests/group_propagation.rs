//! Every command that a clip group propagates, walked in turn (#1813).
//!
//! `Command::GroupClips` exists so linked audio and video are edited together. Which
//! commands honour that link was decided per command and drifted: `MoveClip` did,
//! `TrimClip` and `SplitClip` did not, and the gap was invisible until a trim put a
//! shot and its dialogue two seconds apart.
//!
//! This walk is the guard. It groups a video and an audio clip and applies each
//! propagating command to the video member, asserting after each that the audio
//! member changed too, so a command that stops propagating fails here rather than in
//! somebody's edit. It is pure model: no render, nothing to probe-gate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use avio::{Clip, Command, Timeline, apply};

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// A four-second video clip and a four-second audio clip, linked, both at the top of
/// their own track, with a second clip behind each so a ripple has something to move.
fn linked_pair() -> Timeline {
    let t = Timeline::builder()
        .canvas(320, 180)
        .frame_rate(30.into())
        .video_track(vec![
            Clip::new("v.mp4").trim(Duration::ZERO, s(4.0)),
            Clip::new("v2.mp4")
                .trim(Duration::ZERO, s(4.0))
                .offset(s(4.0)),
        ])
        .audio_track(vec![
            Clip::new("a.mp4").trim(Duration::ZERO, s(4.0)),
            Clip::new("a2.mp4")
                .trim(Duration::ZERO, s(4.0))
                .offset(s(4.0)),
        ])
        .build()
        .unwrap();
    let v = t.video_tracks()[0].clips[0].id;
    let a = t.audio_tracks()[0].clips[0].id;
    apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap()
}

#[test]
fn every_propagating_command_should_reach_the_linked_member() {
    let base = linked_pair();
    let v = base.video_tracks()[0].clips[0].id;

    // MoveClip: the member shifts by the same delta on its own track.
    let out = apply(
        &base,
        &Command::MoveClip {
            clip: v,
            offset: s(5.0),
        },
    )
    .unwrap();
    assert_eq!(
        out.audio_tracks()[0].clips[0].offset,
        s(5.0),
        "MoveClip stopped propagating"
    );

    // MoveClipToTrack: only the addressed clip changes track; the member follows the
    // offset delta and stays where it is.
    let to = base.video_tracks()[0].id;
    let out = apply(
        &base,
        &Command::MoveClipToTrack {
            clip: v,
            to,
            offset: s(6.0),
        },
    )
    .unwrap();
    assert_eq!(
        out.audio_tracks()[0].clips[0].offset,
        s(6.0),
        "MoveClipToTrack stopped propagating"
    );

    // TrimClip: the member's source window follows the same change.
    let out = apply(
        &base,
        &Command::TrimClip {
            clip: v,
            in_point: Some(s(1.0)),
            out_point: Some(s(3.0)),
        },
    )
    .unwrap();
    let audio = &out.audio_tracks()[0].clips[0];
    assert_eq!(
        (audio.in_point, audio.out_point),
        (Some(s(1.0)), Some(s(3.0))),
        "TrimClip stopped propagating"
    );

    // RippleTrim: the member is trimmed and its own track closes the gap, so the
    // clips behind the pair stay level with each other.
    let out = apply(
        &base,
        &Command::RippleTrim {
            clip: v,
            in_point: Some(Duration::ZERO),
            out_point: Some(s(3.0)),
        },
    )
    .unwrap();
    assert_eq!(
        out.audio_tracks()[0].clips[0].out_point,
        Some(s(3.0)),
        "RippleTrim stopped propagating"
    );
    assert_eq!(
        out.video_tracks()[0].clips[1].offset,
        out.audio_tracks()[0].clips[1].offset,
        "the two tracks closed their gaps by different amounts"
    );

    // SplitClip: the member is razored at the same timeline position.
    let out = apply(
        &base,
        &Command::SplitClip {
            clip: v,
            at: s(2.0),
        },
    )
    .unwrap();
    assert_eq!(
        out.audio_tracks()[0].clips.len(),
        3,
        "SplitClip stopped propagating"
    );
    assert_eq!(out.audio_tracks()[0].clips[1].offset, s(2.0));

    // RippleDelete: every member is removed and each track closes its gap.
    let out = apply(&base, &Command::RippleDelete { clip: v }).unwrap();
    assert_eq!(
        out.audio_tracks()[0].clips.len(),
        1,
        "RippleDelete stopped propagating"
    );
    assert_eq!(out.audio_tracks()[0].clips[0].offset, Duration::ZERO);
}
