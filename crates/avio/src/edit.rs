//! Value-based editing of the [`Timeline`] document.
//!
//! A [`Command`] describes one edit; the pure [`apply`] function turns a
//! `(&Timeline, &Command)` into a **new** [`Timeline`] without mutating the
//! input. Because edits are values and each application yields a new version,
//! this is the foundation for Do/Undo/Redo (the `Editor` history is added in a
//! follow-up). This edit layer is part of the engine; the `ff-*` primitives
//! never see it.
//!
//! Clips and tracks are addressed by their stable [`ClipId`] / [`TrackId`], not
//! by position, so an edit stays valid when the timeline changes around it.
//! Resolution is a linear scan over the document's tracks; a command naming an
//! id that is present in no track returns an [`EditError`] and changes nothing.

use std::time::Duration;

use ff_filter::BlendMode;
use thiserror::Error;

use crate::clip::Clip;
use crate::ids::{ClipId, TrackId, TrackKind};
use crate::timeline::Timeline;
use crate::track::Track;

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
    /// Append `clip` to the end of the track with id `track`.
    ///
    /// The clip is assigned a fresh [`ClipId`] by the document; any id already on
    /// the incoming clip is ignored. The clip is boxed so `Command` stays small
    /// (it is stored in the edit history); every other variant is a few machine
    /// words.
    AddClip {
        /// Target track id.
        track: TrackId,
        /// Clip to append (its id is replaced with a fresh one).
        clip: Box<Clip>,
    },
    /// Remove the clip with id `clip`.
    RemoveClip {
        /// Clip to remove.
        clip: ClipId,
    },
    /// Set the timeline offset of the clip with id `clip`.
    MoveClip {
        /// Clip to move.
        clip: ClipId,
        /// New timeline offset.
        offset: Duration,
    },
    /// Set the source in/out points of the clip with id `clip`.
    TrimClip {
        /// Clip to trim.
        clip: ClipId,
        /// New source in-point (`None` = start of file).
        in_point: Option<Duration>,
        /// New source out-point (`None` = end of file).
        out_point: Option<Duration>,
    },
    /// Set a property of the clip with id `clip`.
    SetClipProperty {
        /// Clip to modify.
        clip: ClipId,
        /// The property to set.
        property: ClipProperty,
    },
    /// Replace the clip with id `clip` wholesale (an opaque per-clip patch).
    ///
    /// The result keeps the id `clip`, so identity never changes. `value.id` must
    /// be either unset or equal to `clip`, otherwise the edit is rejected with
    /// [`EditError::ClipIdMismatch`]. This is the general escape hatch for editing
    /// any per-clip field (colour, fades, transition, effect chain, keyframe/
    /// animation tracks, metadata, proxy, pitch) through the undoable path. Values
    /// are stored as-is (not clamped like [`Command::SetClipProperty`]); the
    /// derivation clamps at render time. The value is boxed to keep `Command` small.
    SetClip {
        /// Clip to replace.
        clip: ClipId,
        /// New clip value; its id is forced to `clip`.
        value: Box<Clip>,
    },
    /// Append a new, empty track of `kind` (assigned a fresh [`TrackId`]).
    AddTrack {
        /// Kind of track to append.
        kind: TrackKind,
    },
    /// Remove the track with id `track` and all of its clips.
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
    /// Apply several commands as one atomic edit (and, through [`Editor`](crate::Editor),
    /// one undo step).
    ///
    /// The sub-commands are applied in order to the same timeline. If any one
    /// fails, the whole batch is rejected and the timeline is left unchanged. An
    /// empty batch is a no-op, and batches may nest.
    Batch(Vec<Command>),
}

/// An edit that could not be applied to a [`Timeline`].
#[derive(Debug, Error, PartialEq)]
pub enum EditError {
    /// No track with the given id exists in the document.
    #[error("no track with id {id:?}")]
    TrackNotFound {
        /// The id that resolved to no track.
        id: TrackId,
    },
    /// No clip with the given id exists in the document.
    #[error("no clip with id {id:?}")]
    ClipNotFound {
        /// The id that resolved to no clip.
        id: ClipId,
    },
    /// A [`Command::SetClip`] value carries an id that names a different clip.
    #[error("clip id mismatch: expected {expected:?}, value has {found:?}")]
    ClipIdMismatch {
        /// The target clip id the patch was addressed to.
        expected: ClipId,
        /// The (set) id found on the patch value.
        found: ClipId,
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
/// the already-resolved canvas/fps. Invalid edits (an unknown track/clip id, zero
/// canvas, non-positive fps) return an [`EditError`] and change nothing. A
/// [`Command::Batch`] applies its sub-commands atomically: if any one fails, none
/// take effect.
///
/// # Errors
///
/// Returns [`EditError`] when the target track or clip id is not present, a
/// [`Command::SetCanvas`] / [`Command::SetFrameRate`] value is invalid, or a
/// [`Command::SetClip`] value's id names a different clip.
pub fn apply(timeline: &Timeline, command: &Command) -> Result<Timeline, EditError> {
    let mut next = timeline.clone();
    match command {
        Command::AddClip { track, clip } => {
            // Read the id before borrowing the track so the counter bump below
            // does not conflict with the mutable track borrow.
            let id = ClipId::from_raw(next.next_clip_id);
            let mut new_clip = (**clip).clone();
            new_clip.id = id;
            let tr =
                find_track_mut(&mut next, *track).ok_or(EditError::TrackNotFound { id: *track })?;
            tr.clips.push(new_clip);
            next.next_clip_id += 1;
        }
        Command::RemoveClip { clip } => {
            if !remove_clip(&mut next, *clip) {
                return Err(EditError::ClipNotFound { id: *clip });
            }
        }
        Command::MoveClip { clip, offset } => {
            find_clip_mut(&mut next, *clip)
                .ok_or(EditError::ClipNotFound { id: *clip })?
                .offset = *offset;
        }
        Command::TrimClip {
            clip,
            in_point,
            out_point,
        } => {
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            c.in_point = *in_point;
            c.out_point = *out_point;
        }
        Command::SetClipProperty { clip, property } => {
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            match property {
                // Match `Clip::with_opacity`, which clamps to the documented range.
                ClipProperty::Opacity(v) => c.opacity = v.clamp(0.0, 1.0),
                ClipProperty::Speed(v) => c.speed = *v,
                ClipProperty::BlendMode(v) => c.blend_mode = *v,
                ClipProperty::VolumeDb(v) => c.volume_db = *v,
                ClipProperty::Position { x, y } => {
                    c.x = *x;
                    c.y = *y;
                }
            }
        }
        Command::SetClip { clip, value } => {
            // Reject a patch built for a different clip; an unset value id is the
            // common case (the host built via `Clip::new`) and is accepted. This is
            // a caller error independent of whether the target exists, so it is
            // checked first.
            if value.id.is_set() && value.id != *clip {
                return Err(EditError::ClipIdMismatch {
                    expected: *clip,
                    found: value.id,
                });
            }
            let target =
                find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            let mut new_value = (**value).clone();
            new_value.id = *clip; // preserve identity
            *target = new_value;
        }
        Command::AddTrack { kind } => {
            let id = TrackId::from_raw(next.next_track_id);
            let mut tr = Track::new(Vec::new());
            tr.id = id;
            tracks_mut(&mut next, *kind).push(tr);
            next.next_track_id += 1;
        }
        Command::RemoveTrack { track } => {
            if !remove_track(&mut next, *track) {
                return Err(EditError::TrackNotFound { id: *track });
            }
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
        Command::Batch(commands) => {
            // Apply each sub-command to the accumulating timeline. On failure `?`
            // returns and `next` is dropped, so the input timeline is unchanged
            // (the batch is atomic). The id counters carry forward through `next`.
            for command in commands {
                next = apply(&next, command)?;
            }
        }
    }
    Ok(next)
}

fn tracks_mut(timeline: &mut Timeline, kind: TrackKind) -> &mut Vec<Track> {
    match kind {
        TrackKind::Video => &mut timeline.video_tracks,
        TrackKind::Audio => &mut timeline.audio_tracks,
    }
}

/// Finds the track with `id` in either list (video then audio).
fn find_track_mut(timeline: &mut Timeline, id: TrackId) -> Option<&mut Track> {
    if let Some(tr) = timeline.video_tracks.iter_mut().find(|tr| tr.id == id) {
        return Some(tr);
    }
    timeline.audio_tracks.iter_mut().find(|tr| tr.id == id)
}

/// Finds the clip with `id` anywhere in the document (video then audio tracks).
fn find_clip_mut(timeline: &mut Timeline, id: ClipId) -> Option<&mut Clip> {
    for tr in &mut timeline.video_tracks {
        if let Some(c) = tr.clips.iter_mut().find(|c| c.id == id) {
            return Some(c);
        }
    }
    for tr in &mut timeline.audio_tracks {
        if let Some(c) = tr.clips.iter_mut().find(|c| c.id == id) {
            return Some(c);
        }
    }
    None
}

/// Removes the clip with `id` from whichever track holds it. Returns whether a
/// clip was removed.
fn remove_clip(timeline: &mut Timeline, id: ClipId) -> bool {
    for tr in timeline
        .video_tracks
        .iter_mut()
        .chain(timeline.audio_tracks.iter_mut())
    {
        if let Some(pos) = tr.clips.iter().position(|c| c.id == id) {
            tr.clips.remove(pos);
            return true;
        }
    }
    false
}

/// Removes the track with `id` from whichever list holds it. Returns whether a
/// track was removed.
fn remove_track(timeline: &mut Timeline, id: TrackId) -> bool {
    if let Some(pos) = timeline.video_tracks.iter().position(|tr| tr.id == id) {
        timeline.video_tracks.remove(pos);
        return true;
    }
    if let Some(pos) = timeline.audio_tracks.iter().position(|tr| tr.id == id) {
        timeline.audio_tracks.remove(pos);
        return true;
    }
    false
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

    /// The id of the first video track.
    fn track0(t: &Timeline) -> TrackId {
        t.video_tracks()[0].id
    }

    /// The id of clip `i` on the first video track.
    fn clip_id(t: &Timeline, i: usize) -> ClipId {
        t.video_tracks()[0].clips[i].id
    }

    #[test]
    fn build_should_assign_set_and_unique_ids() {
        let t = timeline_with(3);
        let track = &t.video_tracks()[0];
        assert!(track.id.is_set());
        let ids: Vec<ClipId> = track.clips.iter().map(|c| c.id).collect();
        assert!(ids.iter().all(|id| id.is_set()));
        // Unique.
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "clip ids must be unique");
    }

    #[test]
    fn apply_add_clip_should_append_with_fresh_id() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::AddClip {
                track: track0(&t),
                clip: Box::new(Clip::new("added.mp4")),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 2);
        let added = &out.video_tracks()[0].clips[1];
        assert_eq!(added.source.to_str(), Some("added.mp4"));
        assert!(added.id.is_set());
        assert_ne!(added.id, clip_id(&t, 0), "new clip must get a distinct id");
    }

    #[test]
    fn apply_remove_clip_should_drop_it_by_id() {
        let t = timeline_with(2);
        let out = apply(
            &t,
            &Command::RemoveClip {
                clip: clip_id(&t, 0),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 1);
        assert_eq!(
            out.video_tracks()[0].clips[0].source.to_str(),
            Some("clip1.mp4")
        );
    }

    #[test]
    fn apply_move_clip_should_set_offset() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::MoveClip {
                clip: clip_id(&t, 0),
                offset: Duration::from_secs(3),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[0].offset,
            Duration::from_secs(3)
        );
    }

    #[test]
    fn apply_trim_clip_should_set_in_out() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::TrimClip {
                clip: clip_id(&t, 0),
                in_point: Some(Duration::from_secs(1)),
                out_point: Some(Duration::from_secs(4)),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[0].in_point,
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            out.video_tracks()[0].clips[0].out_point,
            Some(Duration::from_secs(4))
        );
    }

    #[test]
    fn apply_set_clip_property_should_update_field() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::SetClipProperty {
                clip: clip_id(&t, 0),
                property: ClipProperty::Opacity(0.25),
            },
        )
        .unwrap();
        assert!((out.video_tracks()[0].clips[0].opacity - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn apply_set_opacity_should_clamp_to_range() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::SetClipProperty {
                clip: clip_id(&t, 0),
                property: ClipProperty::Opacity(2.0),
            },
        )
        .unwrap();
        assert!((out.video_tracks()[0].clips[0].opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn apply_add_track_should_append_empty_track_with_fresh_id() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::AddTrack {
                kind: TrackKind::Video,
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks().len(), 2);
        let added = &out.video_tracks()[1];
        assert!(added.clips.is_empty());
        assert!(added.id.is_set());
        assert_ne!(added.id, track0(&t), "new track must get a distinct id");
    }

    #[test]
    fn apply_remove_track_should_drop_it_by_id() {
        let t = apply(
            &timeline_with(1),
            &Command::AddTrack {
                kind: TrackKind::Video,
            },
        )
        .unwrap();
        let second = t.video_tracks()[1].id;
        let out = apply(&t, &Command::RemoveTrack { track: second }).unwrap();
        assert_eq!(out.video_tracks().len(), 1);
    }

    #[test]
    fn apply_should_preserve_clip_ids_across_an_unrelated_edit() {
        let t = timeline_with(2);
        let ids_before: Vec<ClipId> = t.video_tracks()[0].clips.iter().map(|c| c.id).collect();
        let out = apply(&t, &Command::SetFrameRate { fps: 24.0 }).unwrap();
        let ids_after: Vec<ClipId> = out.video_tracks()[0].clips.iter().map(|c| c.id).collect();
        assert_eq!(
            ids_before, ids_after,
            "unrelated edit must not renumber clips"
        );
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
        let before = t.video_tracks()[0].clips.len();
        let _ = apply(
            &t,
            &Command::AddClip {
                track: track0(&t),
                clip: Box::new(Clip::new("x.mp4")),
            },
        )
        .unwrap();
        assert_eq!(
            t.video_tracks()[0].clips.len(),
            before,
            "input timeline must be unchanged"
        );
    }

    #[test]
    fn apply_unknown_track_should_err() {
        let t = timeline_with(1);
        // UNSET is never assigned to a placed track, so it is always absent.
        let err = apply(
            &t,
            &Command::AddClip {
                track: TrackId::UNSET,
                clip: Box::new(Clip::new("x.mp4")),
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::TrackNotFound { id: TrackId::UNSET });
    }

    #[test]
    fn apply_unknown_clip_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::RemoveClip {
                clip: ClipId::UNSET,
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: ClipId::UNSET });
    }

    #[test]
    fn apply_add_clip_repeated_should_mint_distinct_never_reused_ids() {
        let t = timeline_with(1);
        let a = apply(
            &t,
            &Command::AddClip {
                track: track0(&t),
                clip: Box::new(Clip::new("a.mp4")),
            },
        )
        .unwrap();
        let id_a = a.video_tracks()[0].clips[1].id;
        // Remove it, then add again along the same linear history: the removed id
        // must not be reused (the counter never rewinds within a chain).
        let b = apply(&a, &Command::RemoveClip { clip: id_a }).unwrap();
        let c = apply(
            &b,
            &Command::AddClip {
                track: track0(&b),
                clip: Box::new(Clip::new("b.mp4")),
            },
        )
        .unwrap();
        let id_c = c.video_tracks()[0].clips[1].id;
        assert_ne!(
            id_a, id_c,
            "a removed id must not be reused along a linear history"
        );
    }

    #[test]
    fn apply_remove_unknown_track_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::RemoveTrack {
                track: TrackId::UNSET,
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::TrackNotFound { id: TrackId::UNSET });
    }

    #[test]
    fn apply_move_unknown_clip_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::MoveClip {
                clip: ClipId::UNSET,
                offset: Duration::from_secs(1),
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: ClipId::UNSET });
    }

    #[test]
    fn build_should_assign_unique_ids_across_all_tracks() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v0.mp4"), Clip::new("v1.mp4")])
            .video_track(vec![Clip::new("v2.mp4")])
            .audio_track(vec![Clip::new("a0.mp3")])
            .build()
            .unwrap();
        let mut track_ids: Vec<TrackId> = t
            .video_tracks()
            .iter()
            .chain(t.audio_tracks().iter())
            .map(|tr| tr.id)
            .collect();
        let n_tracks = track_ids.len();
        track_ids.sort();
        track_ids.dedup();
        assert_eq!(
            track_ids.len(),
            n_tracks,
            "track ids unique across video+audio"
        );

        let mut clip_ids: Vec<ClipId> = t
            .video_tracks()
            .iter()
            .chain(t.audio_tracks().iter())
            .flat_map(|tr| tr.clips.iter().map(|c| c.id))
            .collect();
        let n_clips = clip_ids.len();
        clip_ids.sort();
        clip_ids.dedup();
        assert_eq!(clip_ids.len(), n_clips, "clip ids unique across all tracks");
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

    #[test]
    fn apply_batch_should_apply_all_sub_commands() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::Batch(vec![
                Command::AddClip {
                    track: track0(&t),
                    clip: Box::new(Clip::new("a.mp4")),
                },
                Command::SetFrameRate { fps: 24.0 },
            ]),
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 2);
        assert!((out.frame_rate() - 24.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_batch_should_be_atomic_on_failure() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::Batch(vec![
                Command::AddClip {
                    track: track0(&t),
                    clip: Box::new(Clip::new("a.mp4")),
                },
                // Fails: unknown clip id. The whole batch must be rejected.
                Command::RemoveClip {
                    clip: ClipId::UNSET,
                },
                Command::SetFrameRate { fps: 24.0 },
            ]),
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: ClipId::UNSET });
        // apply returned Err, so the caller keeps the original timeline unchanged.
        assert_eq!(t.video_tracks()[0].clips.len(), 1);
        assert!((t.frame_rate() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_empty_batch_should_be_a_no_op() {
        let t = timeline_with(1);
        let out = apply(&t, &Command::Batch(vec![])).unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 1);
        assert!((out.frame_rate() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_nested_batch_should_apply() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::Batch(vec![Command::Batch(vec![Command::AddClip {
                track: track0(&t),
                clip: Box::new(Clip::new("a.mp4")),
            }])]),
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 2);
    }

    #[test]
    fn apply_batch_of_two_add_clip_should_mint_distinct_ids() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::Batch(vec![
                Command::AddClip {
                    track: track0(&t),
                    clip: Box::new(Clip::new("a.mp4")),
                },
                Command::AddClip {
                    track: track0(&t),
                    clip: Box::new(Clip::new("b.mp4")),
                },
            ]),
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips.len(), 3);
        assert_ne!(clips[1].id, clips[2].id, "batch must mint distinct ids");
        assert!(clips[1].id.is_set() && clips[2].id.is_set());
    }

    #[test]
    fn apply_set_clip_should_replace_the_whole_value_and_preserve_id() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        // A wholesale patch: a different source and several fields at once, none of
        // which have a dedicated `ClipProperty` command.
        let mut patch = Clip::new("patched.mp4");
        patch.brightness = 0.5;
        patch.fade_in = Duration::from_secs(1);
        patch.speed = 2.0;
        let out = apply(
            &t,
            &Command::SetClip {
                clip: id,
                value: Box::new(patch),
            },
        )
        .unwrap();
        let c = &out.video_tracks()[0].clips[0];
        assert_eq!(c.source.to_str(), Some("patched.mp4"));
        assert!((c.brightness - 0.5).abs() < f32::EPSILON);
        assert_eq!(c.fade_in, Duration::from_secs(1));
        assert!((c.speed - 2.0).abs() < f64::EPSILON);
        assert_eq!(c.id, id, "SetClip preserves the clip id");
    }

    #[test]
    fn apply_set_clip_should_accept_a_matching_value_id() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let mut patch = Clip::new("patched.mp4");
        patch.id = id; // explicitly matches the target
        let out = apply(
            &t,
            &Command::SetClip {
                clip: id,
                value: Box::new(patch),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[0].source.to_str(),
            Some("patched.mp4")
        );
    }

    #[test]
    fn apply_set_clip_should_reject_a_mismatched_value_id() {
        let t = timeline_with(2);
        let id0 = clip_id(&t, 0);
        let id1 = clip_id(&t, 1);
        let mut patch = Clip::new("patched.mp4");
        patch.id = id1; // a different clip's id
        let err = apply(
            &t,
            &Command::SetClip {
                clip: id0,
                value: Box::new(patch),
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            EditError::ClipIdMismatch {
                expected: id0,
                found: id1,
            }
        );
        // The original clip is untouched.
        assert_eq!(
            t.video_tracks()[0].clips[0].source.to_str(),
            Some("clip0.mp4")
        );
    }

    #[test]
    fn apply_set_clip_unknown_clip_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::SetClip {
                clip: ClipId::UNSET,
                value: Box::new(Clip::new("x.mp4")),
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: ClipId::UNSET });
    }

    #[test]
    fn apply_set_clip_should_replace_a_clip_on_an_audio_track() {
        // `find_clip_mut` scans both track lists, so SetClip resolves an audio clip.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4")])
            .audio_track(vec![Clip::new("a.mp3")])
            .build()
            .unwrap();
        let id = t.audio_tracks()[0].clips[0].id;
        let out = apply(
            &t,
            &Command::SetClip {
                clip: id,
                value: Box::new(Clip::new("patched.mp3")),
            },
        )
        .unwrap();
        assert_eq!(
            out.audio_tracks()[0].clips[0].source.to_str(),
            Some("patched.mp3")
        );
        assert_eq!(out.audio_tracks()[0].clips[0].id, id);
    }
}
