//! Pure, informational validation of a [`Timeline`] document.
//!
//! [`Timeline::validate`] returns a typed list of [`TimelineIssue`]s so a host can
//! surface problems (overlaps, bad trims, unbounded generated clips, dangling
//! references) before rendering. It is purely informational: it performs no I/O,
//! never opens source files, and does not block [`Timeline::render`] on its own.

use crate::clip::Clip;
use crate::edit::clip_footprint;
use crate::ids::{ClipId, TrackId};
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
    /// Pure and informational: it performs **no I/O**, never opens source files,
    /// and does not mutate the timeline or block [`render`](Self::render). Checks
    /// that depend on a clip's timeline footprint (overlap detection) apply only
    /// to clips whose trim points are set, since an unset in/out point has no
    /// finite footprint.
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
        for track in self.video_tracks.iter().chain(self.audio_tracks.iter()) {
            check_track(track, &mut issues);
        }
        // Track-level automation is typed and lives on the track itself, so it can
        // no longer target a non-existent track or use a malformed key.
        issues
    }
}

/// Per-clip and per-track invariants for one track.
fn check_track(track: &Track, issues: &mut Vec<TimelineIssue>) {
    // Per-clip checks.
    for clip in &track.clips {
        check_clip_trim(clip, issues);
        // A generated (text/solid) source is infinite; an out-point must bound it.
        if clip.source_path().is_none() && clip.out_point.is_none() {
            issues.push(TimelineIssue::GeneratedClipWithoutOutPoint { clip: clip.id });
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
