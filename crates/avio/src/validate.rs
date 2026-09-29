//! Informational validation of a [`Timeline`] document.
//!
//! [`Timeline::validate`] returns a typed list of [`TimelineIssue`]s so a host can
//! surface problems (overlaps, bad trims, unbounded generated clips, dangling
//! references) before rendering. It is purely informational: it performs no I/O,
//! never opens source files, and does not block [`Timeline::render`] on its own.
//!
//! Almost every check reads the document alone. The exception is
//! [`TimelineIssue::TextRendererUnavailable`], which asks the linked `FFmpeg` build
//! whether it can draw text, so that one answer depends on the machine.
//!
//! **It is advisory, and the list is not a proof.** An empty result means the checks
//! implemented here found nothing, not that the render will succeed;
//! [`Timeline::render`] keeps its own checks and can still refuse. Two constructions
//! are deliberately **not** reported: a clip list whose order differs from the clips'
//! offsets, because the render follows `offset` and not the index (#1803), and a
//! `fade_in` plus `fade_out` that together exceed the footprint while each fits on its
//! own, because that is an overlap rather than a fade that cannot be drawn (#1816).
//!
//! **Whether a source can be used is deliberately not checked here either.** Answering
//! it means opening the file, which this module does not do, so
//! [`TimelineBuilder::build`](crate::TimelineBuilder::build) answers it instead and
//! refuses a source the render path cannot use (#1850). ADR-0023 records the division:
//! [`apply`](crate::apply) refuses what is decidable from the document alone, `build`
//! refuses what needs to read a file, and this module reports without refusing.

use crate::clip::{Clip, ClipSource, is_positive_finite};
use crate::edit::clip_footprint;
use crate::ids::{ClipId, TrackId, TrackKind};
use crate::timeline::Timeline;
use crate::track::Track;

/// A single problem found by [`Timeline::validate`].
///
/// Each variant names the offending [`ClipId`] / [`TrackId`]
/// and the cause. This is informational, not an error: a timeline may render even
/// with issues present.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineIssue {
    /// Two clips on the same track overlap in time (their timeline spans intersect).
    ///
    /// Only clips whose footprint is known (both trim points set) are checked.
    ClipOverlap {
        /// Track holding both clips.
        track: TrackId,
        /// The clip that starts earlier.
        earlier: ClipId,
        /// The clip that starts later.
        later: ClipId,
    },
    /// A clip's out-point is before its in-point (an invalid trim).
    TrimOutBeforeIn {
        /// The offending clip.
        clip: ClipId,
    },
    /// A clip's trim yields a zero-length footprint (out-point equals in-point).
    EmptyFootprint {
        /// The offending clip.
        clip: ClipId,
    },
    /// A generated (text/solid) clip has no out-point to bound its infinite source.
    ///
    /// Mirrors the render-time
    /// [`GeneratedSourceNeedsDuration`](crate::TimelineError::GeneratedSourceNeedsDuration)
    /// check.
    GeneratedClipWithoutOutPoint {
        /// The offending clip.
        clip: ClipId,
    },
    /// A clip sits on a track of a kind its source cannot serve.
    ///
    /// A generated (text/solid) source synthesizes video and carries no audio, so the
    /// render skips it when summing an audio track and the clip silently disappears.
    /// The edit path refuses this
    /// ([`EditError::ClipCannotServeTrack`](crate::EditError::ClipCannotServeTrack))
    /// and so does the builder
    /// ([`TimelineError::ClipCannotServeTrack`](crate::TimelineError::ClipCannotServeTrack)),
    /// so this reports a timeline that reached neither, which deserialization can
    /// (ADR-0023).
    ClipCannotServeTrack {
        /// The offending clip.
        clip: ClipId,
        /// The track it sits on.
        track: TrackId,
    },
    /// The timeline's [`frame_rate`](crate::Timeline::frame_rate) is not a positive,
    /// finite number, so the timeline cannot be divided into frames.
    ///
    /// Carries no id: the frame rate belongs to the timeline, not to a clip or a track.
    /// The edit path refuses such a value
    /// ([`EditError::InvalidFrameRate`](crate::EditError::InvalidFrameRate)) and so
    /// does the builder
    /// ([`TimelineError::InvalidFrameRate`](crate::TimelineError::InvalidFrameRate)),
    /// so this reports a timeline that reached neither, which deserialization can
    /// (#1932).
    DegenerateFrameRate,
    /// A clip's [`speed`](crate::Clip::speed) is not a positive, finite number, so its
    /// footprint (`duration / speed`) has no interpretation.
    ///
    /// The edit path clamps such a value to [`MIN_SPEED`](crate::MIN_SPEED), so
    /// this reports one that reached the document through the builder (#1816).
    DegenerateSpeed {
        /// The offending clip.
        clip: ClipId,
    },
    /// A clip's [`fade_in`](crate::Clip::fade_in) or
    /// [`fade_out`](crate::Clip::fade_out) is longer than the clip itself.
    ///
    /// Measured against the clip's **timeline** footprint, which is what the fade is
    /// applied over, so a retimed clip is judged by what it occupies rather than by
    /// how much source it consumes. Only checked when the footprint is known (both
    /// trim points set), as the overlap check is.
    FadeLongerThanClip {
        /// The offending clip.
        clip: ClipId,
    },
    /// A text clip cannot be rendered, because the linked `FFmpeg` build carries no
    /// [`TEXT_FILTER`](ff_filter::TEXT_FILTER).
    ///
    /// Unlike every other check here, the answer depends on the build rather than on
    /// the timeline: the same document is clean on a build that has the filter.
    /// Mirrors the render-time
    /// [`TextRendererUnavailable`](crate::TimelineError::TextRendererUnavailable)
    /// check, and lets a host disable its text tool instead of failing mid-export
    /// (#1809).
    TextRendererUnavailable {
        /// The offending clip.
        clip: ClipId,
    },
    /// A clip carries a transition but is the first clip on its track, so there is
    /// no preceding clip to cross-fade from (the transition is ignored at render).
    DanglingTransition {
        /// Track holding the clip.
        track: TrackId,
        /// The offending clip.
        clip: ClipId,
    },
}

impl Timeline {
    /// Validates this timeline and returns a list of structured diagnostics.
    ///
    /// Informational: it performs **no I/O**, never opens source files, and does
    /// not mutate the timeline or block [`render`](Self::render). Checks that depend
    /// on a clip's timeline footprint (overlap detection) apply only to clips whose
    /// trim points are set, since an unset in/out point has no finite footprint.
    ///
    /// Advisory: an empty result means these checks found nothing, not that
    /// [`render`](Self::render) will succeed. See the module documentation for what is
    /// deliberately not reported.
    ///
    /// It is a function of the document alone with one exception:
    /// [`TimelineIssue::TextRendererUnavailable`] asks the linked `FFmpeg` build
    /// whether it carries the text filter, so the same timeline can come back clean
    /// on one machine and flagged on another (#1809). That query opens nothing, so
    /// the no-I/O promise still holds.
    ///
    /// # Examples
    ///
    /// ```
    /// use avio::{Clip, Timeline};
    /// use std::time::Duration;
    ///
    /// let timeline = Timeline::builder()
    ///     .canvas(1920, 1080)
    ///     .frame_rate(30.0)
    ///     .video_track(vec![Clip::new("a.mp4")])
    ///     .build()
    ///     .unwrap();
    /// assert!(timeline.validate().is_empty());
    /// ```
    #[must_use]
    pub fn validate(&self) -> Vec<TimelineIssue> {
        let mut issues = Vec::new();
        // The kind is which list holds the track, so it is passed down rather than
        // read off the track (see `Timeline::track_kind`).
        for (tracks, kind) in [
            (&self.video_tracks, TrackKind::Video),
            (&self.audio_tracks, TrackKind::Audio),
        ] {
            for track in tracks {
                check_track(track, kind, &mut issues);
            }
        }
        // A timeline-level property, so it is checked here rather than per track
        // (#1932).
        if !is_positive_finite(self.frame_rate) {
            issues.push(TimelineIssue::DegenerateFrameRate);
        }
        // Track-level automation is typed and lives on the track itself, so it can
        // no longer target a non-existent track or use a malformed key.
        issues
    }
}

/// Per-clip and per-track invariants for one track.
fn check_track(track: &Track, kind: TrackKind, issues: &mut Vec<TimelineIssue>) {
    // Per-clip checks.
    // Asked once per track rather than per clip: the answer is a property of the
    // linked build, not of any clip.
    let text_available = ff_filter::text_rendering_available();
    for clip in &track.clips {
        check_clip_trim(clip, issues);
        check_clip_speed(clip, issues);
        check_clip_fades(clip, issues);
        // A generated (text/solid) source is infinite; an out-point must bound it.
        if clip.source_path().is_none() && clip.out_point.is_none() {
            issues.push(TimelineIssue::GeneratedClipWithoutOutPoint { clip: clip.id });
        }
        if !text_available && matches!(clip.source, ClipSource::Text(_)) {
            issues.push(TimelineIssue::TextRendererUnavailable { clip: clip.id });
        }
        if !clip.source.serves_track_kind(kind) {
            issues.push(TimelineIssue::ClipCannotServeTrack {
                clip: clip.id,
                track: track.id,
            });
        }
    }

    // A transition on the first clip has no predecessor to cross-fade from.
    if let Some(first) = track.clips.first()
        && first.transition.is_some()
    {
        issues.push(TimelineIssue::DanglingTransition {
            track: track.id,
            clip: first.id,
        });
    }

    check_overlaps(track, issues);
}

/// Flags an out-of-order trim (out < in) or a zero-length one (out == in).
fn check_clip_trim(clip: &Clip, issues: &mut Vec<TimelineIssue>) {
    if let (Some(in_point), Some(out_point)) = (clip.in_point, clip.out_point) {
        if out_point < in_point {
            issues.push(TimelineIssue::TrimOutBeforeIn { clip: clip.id });
        } else if out_point == in_point {
            issues.push(TimelineIssue::EmptyFootprint { clip: clip.id });
        }
    }
}

/// Flags a `speed` that is not a positive, finite number.
///
/// `speed` divides the clip's duration, so zero, a negative value, an infinity or a
/// NaN leaves the footprint undefined (#1816).
fn check_clip_speed(clip: &Clip, issues: &mut Vec<TimelineIssue>) {
    if !is_positive_finite(clip.speed) {
        issues.push(TimelineIssue::DegenerateSpeed { clip: clip.id });
    }
}

/// Flags a fade longer than the clip's timeline footprint.
///
/// The comparison is against the footprint rather than the source duration because
/// that is the span the fade is drawn over: a clip at `speed = 2.0` occupies half its
/// source, and a fade longer than the occupied span is the one that cannot fit.
fn check_clip_fades(clip: &Clip, issues: &mut Vec<TimelineIssue>) {
    let Some(footprint) = clip_footprint(clip) else {
        return; // no known footprint, as with the overlap check
    };
    if clip.fade_in > footprint || clip.fade_out > footprint {
        issues.push(TimelineIssue::FadeLongerThanClip { clip: clip.id });
    }
}

/// Flags overlapping clips on a track. Only clips with a known footprint are
/// considered; their timeline spans are `[offset, offset + footprint)`.
fn check_overlaps(track: &Track, issues: &mut Vec<TimelineIssue>) {
    // (id, start, end) for clips with a known footprint, sorted by start.
    let mut spans: Vec<(ClipId, std::time::Duration, std::time::Duration)> = track
        .clips
        .iter()
        .filter_map(|c| clip_footprint(c).map(|fp| (c.id, c.offset, c.offset.saturating_add(fp))))
        .collect();
    spans.sort_by_key(|&(_, start, _)| start);

    for i in 0..spans.len() {
        let (id_i, _start_i, end_i) = spans[i];
        for &(id_j, start_j, _end_j) in &spans[i + 1..] {
            // Sorted by start, so once a later clip starts at/after clip i's end no
            // further clip can overlap i.
            if start_j >= end_i {
                break;
            }
            issues.push(TimelineIssue::ClipOverlap {
                track: track.id,
                earlier: id_i,
                later: id_j,
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use ff_filter::XfadeTransition;
    use ff_format::Color;

    use super::*;

    fn base(clips: Vec<Clip>) -> crate::timeline::TimelineBuilder {
        Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(clips)
    }

    /// A timeline that reached neither `apply` nor `build` with this state, which
    /// deserialization can produce. Built here by moving the clip behind both guards'
    /// backs, which only a test can do (#1927).
    #[test]
    fn validate_should_report_a_clip_that_cannot_serve_its_track() {
        use ff_format::TextSpec;

        let mut t = base(vec![Clip::new("v.mp4")])
            .audio_track(vec![Clip::new("a.mp3")])
            .build()
            .unwrap();
        // Reach past the guards the way a deserialized document would. The id is
        // stamped by hand, because a clip pushed straight onto a track keeps
        // `ClipId::UNSET` and the assertion below would then hold whatever id the
        // report carried.
        let clip = ClipId::from_raw(t.next_clip_id);
        let mut title =
            Clip::text(TextSpec::new("title")).trim(Duration::ZERO, Duration::from_secs(2));
        title.id = clip;
        t.audio_tracks[0].clips.push(title);
        let track = t.audio_tracks()[0].id;

        let issues = t.validate();
        assert!(
            issues.contains(&TimelineIssue::ClipCannotServeTrack { clip, track }),
            "expected the title on the audio track to be reported by id, got {issues:?}"
        );
    }

    /// The other direction: a file source is not judged here, because the edit path
    /// does not judge it either (#1850 owns that half).
    #[test]
    fn validate_should_not_report_a_file_clip_on_an_audio_track() {
        let t = base(vec![Clip::new("v.mp4")])
            .audio_track(vec![Clip::new("a.mp3")])
            .build()
            .unwrap();
        assert!(
            !t.validate()
                .iter()
                .any(|i| matches!(i, TimelineIssue::ClipCannotServeTrack { .. })),
            "a file source must not be flagged: {:?}",
            t.validate()
        );
    }

    #[test]
    fn text_clip_should_be_flagged_when_its_renderer_is_unavailable() {
        use ff_format::TextSpec;

        let t = base(vec![
            Clip::text(TextSpec::new("title")).trim(Duration::ZERO, Duration::from_secs(2)),
        ])
        .build()
        .unwrap();
        let id = t.video_tracks()[0].clips[0].id;
        let flagged = t
            .validate()
            .iter()
            .any(|i| matches!(i, TimelineIssue::TextRendererUnavailable { clip } if *clip == id));
        // Asserted as agreement with the build rather than as a fixed answer, so the
        // test holds both where `drawtext` is missing and where it is present
        // (CI's FFmpeg carries no filters at all).
        assert_eq!(
            flagged,
            !ff_filter::text_rendering_available(),
            "the issue and the capability query disagree about this build"
        );
    }

    #[test]
    fn a_timeline_without_text_should_not_be_flagged_for_the_text_renderer() {
        let t = base(vec![
            Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            Clip::solid(Color::rgb(10, 10, 10)).trim(Duration::ZERO, Duration::from_secs(2)),
        ])
        .build()
        .unwrap();
        // Build-independent: a solid is generated too, but it needs no text filter.
        assert!(
            !t.validate()
                .iter()
                .any(|i| matches!(i, TimelineIssue::TextRendererUnavailable { .. })),
            "only text clips depend on the text renderer"
        );
    }

    /// A two-clip timeline whose **second** clip is the one under test, so a check
    /// that reports the wrong clip id fails here. The neighbours above pin the id the
    /// same way, through `contains`.
    fn second_clip_under_test(f: impl FnOnce(Clip) -> Clip) -> (Timeline, ClipId) {
        let t = base(vec![
            Clip::new("first.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            f(Clip::new("a.mp4")
                .trim(Duration::ZERO, Duration::from_secs(4))
                .offset(Duration::from_secs(10))),
        ])
        .build()
        .unwrap();
        let id = t.video_tracks()[0].clips[1].id;
        (t, id)
    }

    #[test]
    fn validate_should_report_a_zero_speed_clip() {
        let (t, id) = second_clip_under_test(|c| c.with_speed(0.0));
        assert!(
            t.validate()
                .contains(&TimelineIssue::DegenerateSpeed { clip: id })
        );
    }

    #[test]
    fn validate_should_report_a_negative_speed_clip() {
        let (t, id) = second_clip_under_test(|c| c.with_speed(-2.0));
        assert!(
            t.validate()
                .contains(&TimelineIssue::DegenerateSpeed { clip: id })
        );
    }

    #[test]
    fn validate_should_not_report_a_speed_of_one() {
        let (t, id) = second_clip_under_test(|c| c.with_speed(1.0));
        assert!(
            !t.validate()
                .contains(&TimelineIssue::DegenerateSpeed { clip: id })
        );
    }

    #[test]
    fn validate_should_report_a_fade_longer_than_its_clip() {
        // The issue's case: 5s of fade on a 1s clip.
        let t = base(vec![
            Clip::new("a.mp4")
                .trim(Duration::ZERO, Duration::from_secs(1))
                .with_fade_in(Duration::from_secs(5))
                .with_fade_out(Duration::from_secs(5)),
        ])
        .build()
        .unwrap();
        let id = t.video_tracks()[0].clips[0].id;
        assert!(
            t.validate()
                .contains(&TimelineIssue::FadeLongerThanClip { clip: id })
        );

        // Each edge on its own, so neither is carried by the other.
        let (fade_in_only, in_id) =
            second_clip_under_test(|c| c.with_fade_in(Duration::from_secs(5)));
        assert!(
            fade_in_only
                .validate()
                .contains(&TimelineIssue::FadeLongerThanClip { clip: in_id })
        );
        let (fade_out_only, out_id) =
            second_clip_under_test(|c| c.with_fade_out(Duration::from_secs(5)));
        assert!(
            fade_out_only
                .validate()
                .contains(&TimelineIssue::FadeLongerThanClip { clip: out_id })
        );
    }

    #[test]
    fn validate_should_not_report_a_fade_that_fits() {
        let (t, id) = second_clip_under_test(|c| {
            c.with_fade_in(Duration::from_millis(500))
                .with_fade_out(Duration::from_millis(500))
        });
        assert!(
            !t.validate()
                .contains(&TimelineIssue::FadeLongerThanClip { clip: id })
        );
    }

    #[test]
    fn validate_should_judge_a_fade_against_the_timeline_footprint() {
        // At 2x the clip occupies 2s of timeline for 4s of source, so a 3s fade does
        // not fit even though it is shorter than the source it consumes. This is what
        // makes the footprint the right quantity rather than a coincidence.
        let (t, id) =
            second_clip_under_test(|c| c.with_speed(2.0).with_fade_in(Duration::from_secs(3)));
        assert!(
            t.validate()
                .contains(&TimelineIssue::FadeLongerThanClip { clip: id })
        );
    }

    #[test]
    fn validate_should_not_report_a_clip_list_out_of_offset_order() {
        // Deliberately permitted: the render follows `offset`, not the index (#1803),
        // so reporting this would be a false alarm. Pinned as a test so a later change
        // has to argue with it.
        let t = base(vec![
            Clip::new("a.mp4")
                .trim(Duration::ZERO, Duration::from_secs(1))
                .offset(Duration::from_secs(10)),
            Clip::new("b.mp4")
                .trim(Duration::ZERO, Duration::from_secs(1))
                .offset(Duration::from_secs(2)),
        ])
        .build()
        .unwrap();
        assert!(
            t.validate().is_empty(),
            "an out-of-order clip list is permitted: {:?}",
            t.validate()
        );
    }

    #[test]
    fn validate_clean_timeline_should_have_no_issues() {
        let t = base(vec![
            Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            Clip::new("b.mp4")
                .trim(Duration::ZERO, Duration::from_secs(4))
                .offset(Duration::from_secs(4)),
        ])
        .build()
        .unwrap();
        assert!(
            t.validate().is_empty(),
            "clean timeline: {:?}",
            t.validate()
        );
    }

    #[test]
    fn validate_should_detect_track_overlap() {
        // a: [0, 4), b: [2, 6) -> overlap.
        let t = base(vec![
            Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            Clip::new("b.mp4")
                .trim(Duration::ZERO, Duration::from_secs(4))
                .offset(Duration::from_secs(2)),
        ])
        .build()
        .unwrap();
        let ids: Vec<_> = t.video_tracks()[0].clips.iter().map(|c| c.id).collect();
        assert!(t.validate().contains(&TimelineIssue::ClipOverlap {
            track: t.video_tracks()[0].id,
            earlier: ids[0],
            later: ids[1],
        }));
    }

    #[test]
    fn validate_overlap_should_catch_a_long_clip_spanning_a_far_one() {
        // a: [0, 10) (a long clip), b: [2, 4), c: [6, 8). `a` overlaps both `b`
        // and `c`; `b` and `c` do not touch. The far a-c overlap must not be lost
        // by the sorted break in `check_overlaps`.
        let t = base(vec![
            Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10)),
            Clip::new("b.mp4")
                .trim(Duration::ZERO, Duration::from_secs(2))
                .offset(Duration::from_secs(2)),
            Clip::new("c.mp4")
                .trim(Duration::ZERO, Duration::from_secs(2))
                .offset(Duration::from_secs(6)),
        ])
        .build()
        .unwrap();
        let track = t.video_tracks()[0].id;
        let ids: Vec<_> = t.video_tracks()[0].clips.iter().map(|c| c.id).collect();
        let overlaps: Vec<_> = t
            .validate()
            .into_iter()
            .filter(|i| matches!(i, TimelineIssue::ClipOverlap { .. }))
            .collect();
        assert!(overlaps.contains(&TimelineIssue::ClipOverlap {
            track,
            earlier: ids[0],
            later: ids[1],
        }));
        assert!(
            overlaps.contains(&TimelineIssue::ClipOverlap {
                track,
                earlier: ids[0],
                later: ids[2],
            }),
            "the far a-c overlap must be detected"
        );
        assert_eq!(overlaps.len(), 2, "b and c do not overlap each other");
    }

    #[test]
    fn validate_should_detect_trim_out_before_in() {
        let t = base(vec![
            Clip::new("a.mp4").trim(Duration::from_secs(5), Duration::from_secs(2)),
        ])
        .build()
        .unwrap();
        let id = t.video_tracks()[0].clips[0].id;
        assert!(
            t.validate()
                .contains(&TimelineIssue::TrimOutBeforeIn { clip: id })
        );
    }

    #[test]
    fn validate_should_detect_empty_footprint() {
        let t = base(vec![
            Clip::new("a.mp4").trim(Duration::from_secs(3), Duration::from_secs(3)),
        ])
        .build()
        .unwrap();
        let id = t.video_tracks()[0].clips[0].id;
        assert!(
            t.validate()
                .contains(&TimelineIssue::EmptyFootprint { clip: id })
        );
    }

    #[test]
    fn validate_should_detect_generated_clip_without_out_point() {
        let t = base(vec![Clip::solid(Color::rgb(0, 0, 0))])
            .build()
            .unwrap();
        let id = t.video_tracks()[0].clips[0].id;
        assert!(
            t.validate()
                .contains(&TimelineIssue::GeneratedClipWithoutOutPoint { clip: id })
        );
        // A bounded generated clip is fine.
        let ok = base(vec![
            Clip::solid(Color::rgb(0, 0, 0)).trim(Duration::ZERO, Duration::from_secs(1)),
        ])
        .build()
        .unwrap();
        assert!(
            !ok.validate()
                .iter()
                .any(|i| matches!(i, TimelineIssue::GeneratedClipWithoutOutPoint { .. }))
        );
    }

    /// A timeline that reached neither `apply` nor `build` with this value, which
    /// deserialization can produce. Set directly, because both guarded paths refuse it
    /// (#1932).
    #[test]
    fn validate_should_report_a_degenerate_frame_rate() {
        for fps in [f64::NAN, f64::INFINITY, 0.0, -30.0] {
            let mut t = base(vec![Clip::new("v.mp4")]).build().unwrap();
            t.frame_rate = fps;
            assert!(
                t.validate().contains(&TimelineIssue::DegenerateFrameRate),
                "expected a report for frame_rate={fps}, got {:?}",
                t.validate()
            );
        }
    }

    #[test]
    fn validate_should_not_report_a_normal_frame_rate() {
        let t = base(vec![Clip::new("v.mp4")]).build().unwrap();
        assert!(
            !t.validate().contains(&TimelineIssue::DegenerateFrameRate),
            "30 fps must not be reported: {:?}",
            t.validate()
        );
    }

    #[test]
    fn validate_should_detect_dangling_transition() {
        let t = base(vec![
            Clip::new("a.mp4")
                .trim(Duration::ZERO, Duration::from_secs(4))
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ])
        .build()
        .unwrap();
        let id = t.video_tracks()[0].clips[0].id;
        assert!(t.validate().contains(&TimelineIssue::DanglingTransition {
            track: t.video_tracks()[0].id,
            clip: id,
        }));
    }

    #[test]
    fn validate_should_not_open_source_files() {
        // Nonexistent paths with an explicit canvas: `build` does not probe, and
        // `validate` must not touch the filesystem either. A File clip is never
        // flagged as a generated-source issue, regardless of whether the path
        // exists, and no I/O error surfaces.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("does_not_exist_1.mp4").trim(Duration::ZERO, Duration::from_secs(2)),
            ])
            .audio_track(vec![Clip::new("does_not_exist_2.mp3")])
            .build()
            .unwrap();
        assert!(
            !t.validate()
                .iter()
                .any(|i| matches!(i, TimelineIssue::GeneratedClipWithoutOutPoint { .. }))
        );
    }
}
