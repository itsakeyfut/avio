//! Value-based editing of the [`Timeline`] document.
//!
//! A [`Command`] describes one edit; the pure [`apply`] function turns a
//! `(&Timeline, &Command)` into a **new** [`Timeline`] without mutating the
//! input. Because edits are values and each application yields a new version,
//! this is the foundation for Do/Undo/Redo (the `Editor` history is added in a
//! follow-up). This edit layer is part of the engine; the `ff-*` primitives
//! never see it.

use std::time::Duration;

use ff_filter::BlendMode;
use thiserror::Error;

use crate::clip::Clip;
use crate::timeline::Timeline;

/// Which track list a [`TrackId`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    /// A video track (`Timeline::video_tracks`).
    Video,
    /// An audio track (`Timeline::audio_tracks`).
    Audio,
}

/// Addresses a single track within a [`Timeline`] by kind and position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackId {
    /// Whether this is a video or audio track.
    pub kind: TrackKind,
    /// Zero-based index into the track list of that kind.
    pub index: usize,
}

impl TrackId {
    /// The video track at `index`.
    #[must_use]
    pub fn video(index: usize) -> Self {
        Self {
            kind: TrackKind::Video,
            index,
        }
    }

    /// The audio track at `index`.
    #[must_use]
    pub fn audio(index: usize) -> Self {
        Self {
            kind: TrackKind::Audio,
            index,
        }
    }
}

/// A per-clip property that [`Command::SetClipProperty`] can set.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipProperty {
    /// Overlay opacity in `[0.0, 1.0]`.
    Opacity(f32),
    /// Playback speed multiplier (`1.0` = normal).
    Speed(f64),
    /// Compositing blend mode.
    BlendMode(BlendMode),
    /// Per-clip volume in decibels (`0.0` = unity gain).
    VolumeDb(f64),
    /// Overlay position (pixels) of the clip's top-left on the canvas.
    Position {
        /// Horizontal offset.
        x: f64,
        /// Vertical offset.
        y: f64,
    },
}

/// A single, value-based edit to a [`Timeline`]. Apply it with [`apply`].
#[derive(Debug, Clone)]
pub enum Command {
    /// Append `clip` to the end of `track`.
    ///
    /// The clip is boxed so `Command` stays small (it is stored in the edit
    /// history); every other variant is a few machine words.
    AddClip {
        /// Target track.
        track: TrackId,
        /// Clip to append.
        clip: Box<Clip>,
    },
    /// Remove the clip at `index` from `track`.
    RemoveClip {
        /// Target track.
        track: TrackId,
        /// Zero-based clip index.
        index: usize,
    },
    /// Set the timeline offset of the clip at `index` on `track`.
    MoveClip {
        /// Target track.
        track: TrackId,
        /// Zero-based clip index.
        index: usize,
        /// New timeline offset.
        offset: Duration,
    },
    /// Set the source in/out points of the clip at `index` on `track`.
    TrimClip {
        /// Target track.
        track: TrackId,
        /// Zero-based clip index.
        index: usize,
        /// New source in-point (`None` = start of file).
        in_point: Option<Duration>,
        /// New source out-point (`None` = end of file).
        out_point: Option<Duration>,
    },
    /// Set a property of the clip at `index` on `track`.
    SetClipProperty {
        /// Target track.
        track: TrackId,
        /// Zero-based clip index.
        index: usize,
        /// The property to set.
        property: ClipProperty,
    },
    /// Append a new, empty track of `kind`.
    AddTrack {
        /// Kind of track to append.
        kind: TrackKind,
    },
    /// Remove `track` and all of its clips.
    RemoveTrack {
        /// Track to remove.
        track: TrackId,
    },
    /// Set the output canvas dimensions (marks the canvas explicit).
    SetCanvas {
        /// Canvas width in pixels (must be non-zero).
        width: u32,
        /// Canvas height in pixels (must be non-zero).
        height: u32,
    },
    /// Set the output frame rate (must be positive).
    SetFrameRate {
        /// Frames per second.
        fps: f64,
    },
}

/// An edit that could not be applied to a [`Timeline`].
#[derive(Debug, Error, PartialEq)]
pub enum EditError {
    /// The addressed track does not exist.
    #[error("{kind:?} track index {index} out of range (len {len})")]
    TrackOutOfRange {
        /// Track kind addressed.
        kind: TrackKind,
        /// Requested index.
        index: usize,
        /// Number of tracks of that kind.
        len: usize,
    },
    /// The addressed clip does not exist in its track.
    #[error("clip index {index} out of range (len {len})")]
    ClipOutOfRange {
        /// Requested index.
        index: usize,
        /// Number of clips in the track.
        len: usize,
    },
    /// Canvas dimensions must be non-zero.
    #[error("invalid canvas: {width}x{height}")]
    InvalidCanvas {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
    /// Frame rate must be positive.
    #[error("invalid frame rate: {0}")]
    InvalidFrameRate(f64),
}

/// Applies `command` to `timeline`, returning a **new** [`Timeline`].
///
/// This is a pure function: `timeline` is not modified (it is borrowed
/// immutably), no I/O is performed, and no source is re-probed — an edit carries
/// the already-resolved canvas/fps. Invalid edits (out-of-range track/clip index,
/// zero canvas, non-positive fps) return an [`EditError`] and change nothing.
///
/// # Errors
///
/// Returns [`EditError`] when the target track or clip index is out of range, or
/// a [`Command::SetCanvas`] / [`Command::SetFrameRate`] value is invalid.
pub fn apply(timeline: &Timeline, command: &Command) -> Result<Timeline, EditError> {
    let mut next = timeline.clone();
    match command {
        Command::AddClip { track, clip } => {
            track_mut(&mut next, *track)?.push((**clip).clone());
        }
        Command::RemoveClip { track, index } => {
            let clips = track_mut(&mut next, *track)?;
            check_clip(clips, *index)?;
            clips.remove(*index);
        }
        Command::MoveClip {
            track,
            index,
            offset,
        } => {
            clip_mut(&mut next, *track, *index)?.timeline_offset = *offset;
        }
        Command::TrimClip {
            track,
            index,
            in_point,
            out_point,
        } => {
            let clip = clip_mut(&mut next, *track, *index)?;
            clip.in_point = *in_point;
            clip.out_point = *out_point;
        }
        Command::SetClipProperty {
            track,
            index,
            property,
        } => {
            let clip = clip_mut(&mut next, *track, *index)?;
            match property {
                // Match `Clip::with_opacity`, which clamps to the documented range.
                ClipProperty::Opacity(v) => clip.opacity = v.clamp(0.0, 1.0),
                ClipProperty::Speed(v) => clip.speed = *v,
                ClipProperty::BlendMode(v) => clip.blend_mode = *v,
                ClipProperty::VolumeDb(v) => clip.volume_db = *v,
                ClipProperty::Position { x, y } => {
                    clip.x = *x;
                    clip.y = *y;
                }
            }
        }
        Command::AddTrack { kind } => {
            tracks_mut(&mut next, *kind).push(Vec::new());
        }
        Command::RemoveTrack { track } => {
            let tracks = tracks_mut(&mut next, track.kind);
            check_track(tracks, track.kind, track.index)?;
            tracks.remove(track.index);
        }
        Command::SetCanvas { width, height } => {
            if *width == 0 || *height == 0 {
                return Err(EditError::InvalidCanvas {
                    width: *width,
                    height: *height,
                });
            }
            next.canvas_width = *width;
            next.canvas_height = *height;
            next.canvas_explicit = true;
        }
        Command::SetFrameRate { fps } => {
            if *fps <= 0.0 {
                return Err(EditError::InvalidFrameRate(*fps));
            }
            next.frame_rate = *fps;
        }
    }
    Ok(next)
}

fn tracks_mut(timeline: &mut Timeline, kind: TrackKind) -> &mut Vec<Vec<Clip>> {
    match kind {
        TrackKind::Video => &mut timeline.video_tracks,
        TrackKind::Audio => &mut timeline.audio_tracks,
    }
}

fn check_track(tracks: &[Vec<Clip>], kind: TrackKind, index: usize) -> Result<(), EditError> {
    if index >= tracks.len() {
        return Err(EditError::TrackOutOfRange {
            kind,
            index,
            len: tracks.len(),
        });
    }
    Ok(())
}

fn check_clip(clips: &[Clip], index: usize) -> Result<(), EditError> {
    if index >= clips.len() {
        return Err(EditError::ClipOutOfRange {
            index,
            len: clips.len(),
        });
    }
    Ok(())
}

fn track_mut(timeline: &mut Timeline, id: TrackId) -> Result<&mut Vec<Clip>, EditError> {
    let tracks = tracks_mut(timeline, id.kind);
    check_track(tracks, id.kind, id.index)?;
    Ok(&mut tracks[id.index])
}

fn clip_mut(timeline: &mut Timeline, id: TrackId, index: usize) -> Result<&mut Clip, EditError> {
    let clips = track_mut(timeline, id)?;
    check_clip(clips, index)?;
    Ok(&mut clips[index])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A one-video-track timeline with `n` clips; explicit canvas + fps so
    /// `build()` does not probe the (nonexistent) sources.
    fn timeline_with(n: usize) -> Timeline {
        let clips: Vec<Clip> = (0..n).map(|i| Clip::new(format!("clip{i}.mp4"))).collect();
        Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(clips)
            .build()
            .unwrap()
    }

    #[test]
    fn apply_add_clip_should_append_to_track() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::AddClip {
                track: TrackId::video(0),
                clip: Box::new(Clip::new("added.mp4")),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].len(), 2);
        assert_eq!(out.video_tracks()[0][1].source.to_str(), Some("added.mp4"));
    }

    #[test]
    fn apply_remove_clip_should_drop_it() {
        let t = timeline_with(2);
        let out = apply(
            &t,
            &Command::RemoveClip {
                track: TrackId::video(0),
                index: 0,
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].len(), 1);
        assert_eq!(out.video_tracks()[0][0].source.to_str(), Some("clip1.mp4"));
    }

    #[test]
    fn apply_move_clip_should_set_offset() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::MoveClip {
                track: TrackId::video(0),
                index: 0,
                offset: Duration::from_secs(3),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0][0].timeline_offset,
            Duration::from_secs(3)
        );
    }

    #[test]
    fn apply_trim_clip_should_set_in_out() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::TrimClip {
                track: TrackId::video(0),
                index: 0,
                in_point: Some(Duration::from_secs(1)),
                out_point: Some(Duration::from_secs(4)),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0][0].in_point,
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            out.video_tracks()[0][0].out_point,
            Some(Duration::from_secs(4))
        );
    }

    #[test]
    fn apply_set_clip_property_should_update_field() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::SetClipProperty {
                track: TrackId::video(0),
                index: 0,
                property: ClipProperty::Opacity(0.25),
            },
        )
        .unwrap();
        assert!((out.video_tracks()[0][0].opacity - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn apply_set_opacity_should_clamp_to_range() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::SetClipProperty {
                track: TrackId::video(0),
                index: 0,
                property: ClipProperty::Opacity(2.0),
            },
        )
        .unwrap();
        assert!((out.video_tracks()[0][0].opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn apply_add_track_should_append_empty_track() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::AddTrack {
                kind: TrackKind::Video,
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks().len(), 2);
        assert!(out.video_tracks()[1].is_empty());
    }

    #[test]
    fn apply_remove_track_should_drop_it() {
        let t = apply(
            &timeline_with(1),
            &Command::AddTrack {
                kind: TrackKind::Video,
            },
        )
        .unwrap();
        let out = apply(
            &t,
            &Command::RemoveTrack {
                track: TrackId::video(1),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks().len(), 1);
    }

    #[test]
    fn apply_set_canvas_should_update_dims_and_mark_explicit() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::SetCanvas {
                width: 1280,
                height: 720,
            },
        )
        .unwrap();
        assert_eq!(out.canvas_width(), 1280);
        assert_eq!(out.canvas_height(), 720);
        assert_eq!(out.explicit_canvas(), Some((1280, 720)));
    }

    #[test]
    fn apply_set_frame_rate_should_update_fps() {
        let t = timeline_with(1);
        let out = apply(&t, &Command::SetFrameRate { fps: 24.0 }).unwrap();
        assert!((out.frame_rate() - 24.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_should_not_mutate_the_input() {
        let t = timeline_with(1);
        let before = t.video_tracks()[0].len();
        let _ = apply(
            &t,
            &Command::AddClip {
                track: TrackId::video(0),
                clip: Box::new(Clip::new("x.mp4")),
            },
        )
        .unwrap();
        assert_eq!(
            t.video_tracks()[0].len(),
            before,
            "input timeline must be unchanged"
        );
    }

    #[test]
    fn apply_out_of_range_track_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::AddClip {
                track: TrackId::video(5),
                clip: Box::new(Clip::new("x.mp4")),
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            EditError::TrackOutOfRange {
                kind: TrackKind::Video,
                index: 5,
                len: 1,
            }
        );
    }

    #[test]
    fn apply_out_of_range_clip_index_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::RemoveClip {
                track: TrackId::video(0),
                index: 9,
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipOutOfRange { index: 9, len: 1 });
    }

    #[test]
    fn apply_invalid_frame_rate_should_err() {
        let t = timeline_with(1);
        let err = apply(&t, &Command::SetFrameRate { fps: 0.0 }).unwrap_err();
        assert_eq!(err, EditError::InvalidFrameRate(0.0));
    }

    #[test]
    fn apply_invalid_canvas_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::SetCanvas {
                width: 0,
                height: 720,
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            EditError::InvalidCanvas {
                width: 0,
                height: 720
            }
        );
    }
}
