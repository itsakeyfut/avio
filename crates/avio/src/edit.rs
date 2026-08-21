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
use crate::ids::{ClipId, GroupId, MarkerId, TrackId, TrackKind};
use crate::marker::Marker;
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
    ///
    /// A single-clip removal: a grouped member is removed on its own (the group
    /// keeps its remaining members). Use [`RippleDelete`](Self::RippleDelete) to
    /// remove a whole linked group and close the gaps.
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
    /// Split the clip with id `clip` at timeline position `at` into two contiguous
    /// clips (a razor cut).
    ///
    /// The left clip keeps the original id, its offset, in-point, leading transition
    /// and fade-in, and ends at the cut. The right clip gets a fresh id, starts at
    /// `at`, keeps the original properties and the trailing fade-out, and clears the
    /// leading transition and fade-in (a hard cut carries no fade). `at` must be
    /// strictly inside the clip's timeline span, else [`EditError::SplitOutOfRange`].
    SplitClip {
        /// Clip to split.
        clip: ClipId,
        /// Timeline position of the cut.
        at: Duration,
    },
    /// Move the clip with id `clip` to the track with id `to`, at timeline `offset`.
    ///
    /// The clip keeps its id and all other properties, and is appended to the end of
    /// the destination track's clip list. Moving within the same track is allowed
    /// (the clip is re-offset and re-appended). Fails with
    /// [`EditError::TrackNotFound`] if `to` does not exist (the timeline is left
    /// unchanged), or [`EditError::ClipNotFound`] if the clip does not exist.
    MoveClipToTrack {
        /// Clip to move.
        clip: ClipId,
        /// Destination track.
        to: TrackId,
        /// New timeline offset on the destination track.
        offset: Duration,
    },
    /// Remove the clip with id `clip` and close the gap it leaves (a ripple delete).
    ///
    /// Clips on the same track that start after the removed clip (a greater `offset`)
    /// move left by the removed clip's timeline footprint; other tracks are not
    /// touched. When the removed clip runs to end-of-file (its footprint is unknown)
    /// nothing is shifted — it is a plain remove.
    RippleDelete {
        /// Clip to remove.
        clip: ClipId,
    },
    /// Set the source in/out points of the clip with id `clip` and close/open the
    /// gap this creates (a ripple trim).
    ///
    /// Like [`TrimClip`](Self::TrimClip), but clips on the same track that start
    /// after the trimmed clip (a greater `offset`) also shift by the change in the
    /// clip's timeline footprint: shrinking pulls them left (closes the gap),
    /// growing pushes them right (opens the gap). The trimmed clip's own `offset`
    /// is unchanged, and other tracks are not touched. When the footprint is
    /// unknown before or after the trim (in- or out-point unset), the trim still
    /// applies but nothing is shifted.
    RippleTrim {
        /// Clip to trim.
        clip: ClipId,
        /// New source in-point (`None` = start of file).
        in_point: Option<Duration>,
        /// New source out-point (`None` = end of file).
        out_point: Option<Duration>,
    },
    /// Link `clips` into one group (assigned a fresh [`GroupId`]).
    ///
    /// Grouped clips are edited together: a [`MoveClip`](Self::MoveClip) /
    /// [`MoveClipToTrack`](Self::MoveClipToTrack) / [`RippleDelete`](Self::RippleDelete)
    /// on any member propagates to the whole group as one undo step (see [`apply`]).
    /// Every named clip must exist, otherwise the edit is rejected with
    /// [`EditError::ClipNotFound`] and the timeline is unchanged. A clip already in a
    /// group is reassigned to the new one; an empty `clips` list is a no-op.
    GroupClips {
        /// Clips to link (must all exist).
        clips: Vec<ClipId>,
    },
    /// Unlink the group that the clip with id `clip` belongs to (clears `group` on
    /// every member).
    ///
    /// Fails with [`EditError::ClipNotFound`] if `clip` does not exist; a no-op when
    /// the clip is not grouped.
    UngroupClips {
        /// A member of the group to dissolve.
        clip: ClipId,
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
    /// Add an editorial [`Marker`] to the timeline.
    ///
    /// The marker is assigned a fresh [`MarkerId`] by the document; any id already
    /// on the incoming marker is ignored. Markers are metadata only and do not
    /// affect render or preview.
    AddMarker {
        /// Marker to add (its id is replaced with a fresh one).
        marker: Marker,
    },
    /// Remove the marker with id `marker`.
    RemoveMarker {
        /// Marker to remove.
        marker: MarkerId,
    },
    /// Set the timeline position (`pts`) of the marker with id `marker`.
    MoveMarker {
        /// Marker to move.
        marker: MarkerId,
        /// New timeline position.
        pts: Duration,
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
    /// A [`Command::RemoveMarker`] / [`Command::MoveMarker`] named a marker id that
    /// is present in no marker.
    #[error("no marker with id {id:?}")]
    MarkerNotFound {
        /// The id that resolved to no marker.
        id: MarkerId,
    },
    /// A [`Command::SetClip`] value carries an id that names a different clip.
    #[error("clip id mismatch: expected {expected:?}, value has {found:?}")]
    ClipIdMismatch {
        /// The target clip id the patch was addressed to.
        expected: ClipId,
        /// The (set) id found on the patch value.
        found: ClipId,
    },
    /// A [`Command::SplitClip`] point is not strictly inside the clip's span.
    #[error("split point {at:?} is not inside clip {clip:?}")]
    SplitOutOfRange {
        /// The clip that could not be split.
        clip: ClipId,
        /// The requested split position.
        at: Duration,
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
/// [`Command::SetCanvas`] / [`Command::SetFrameRate`] value is invalid, a
/// [`Command::SetClip`] value's id names a different clip, or a
/// [`Command::SplitClip`] point is outside the clip's span.
pub fn apply(timeline: &Timeline, command: &Command) -> Result<Timeline, EditError> {
    let mut next = timeline.clone();
    match command {
        Command::AddClip { track, clip } => {
            // Read the id before borrowing the track so the counter bump below
            // does not conflict with the mutable track borrow.
            let id = ClipId::from_raw(next.next_clip_id);
            let mut new_clip = (**clip).clone();
            new_clip.id = id;
            // A freshly added clip starts ungrouped; link it explicitly via
            // `GroupClips` (an incoming group id would be a stale, document-scoped
            // value, like the clip's own id which is re-stamped above).
            new_clip.group = None;
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
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            let old_offset = c.offset;
            let group = c.group;
            c.offset = *offset;
            // A grouped move carries the linked members by the same offset delta,
            // in this one `apply` (so it is a single undo step).
            if let Some(g) = group {
                shift_group_offsets(&mut next, g, *clip, old_offset, *offset);
            }
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
        Command::SplitClip { clip, at } => {
            // Reserve a fresh id for the right half before borrowing the tracks.
            let right_id = ClipId::from_raw(next.next_clip_id);
            let (clips, idx) = find_clip_track_mut(&mut next, *clip)
                .ok_or(EditError::ClipNotFound { id: *clip })?;
            let (left, mut right) =
                split_clip(&clips[idx], *at).ok_or(EditError::SplitOutOfRange {
                    clip: *clip,
                    at: *at,
                })?;
            right.id = right_id;
            clips[idx] = left;
            clips.insert(idx + 1, right);
            next.next_clip_id += 1;
        }
        Command::MoveClipToTrack { clip, to, offset } => {
            // Verify the destination exists before removing the clip, so a bad
            // target leaves the timeline unchanged.
            if find_track_mut(&mut next, *to).is_none() {
                return Err(EditError::TrackNotFound { id: *to });
            }
            let mut moved =
                take_clip(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            let old_offset = moved.offset;
            let group = moved.group;
            moved.offset = *offset;
            // Re-resolve the destination after the take (borrows/indices changed).
            find_track_mut(&mut next, *to)
                .ok_or(EditError::TrackNotFound { id: *to })?
                .clips
                .push(moved);
            // Linked members keep A/V sync: they shift by the same offset delta but
            // stay on their own tracks (only the addressed clip changes track).
            if let Some(g) = group {
                shift_group_offsets(&mut next, g, *clip, old_offset, *offset);
            }
        }
        Command::RippleDelete { clip } => {
            // A grouped ripple-delete removes every linked member (each closing the
            // gap on its own track); an ungrouped one removes just this clip.
            let group = find_clip_mut(&mut next, *clip)
                .ok_or(EditError::ClipNotFound { id: *clip })?
                .group;
            let targets: Vec<ClipId> = match group {
                Some(g) => collect_group_ids(&next, g),
                None => vec![*clip],
            };
            for target in targets {
                ripple_delete_one(&mut next, target);
            }
        }
        Command::RippleTrim {
            clip,
            in_point,
            out_point,
        } => {
            let (clips, idx) = find_clip_track_mut(&mut next, *clip)
                .ok_or(EditError::ClipNotFound { id: *clip })?;
            let clip_offset = clips[idx].offset;
            let old_footprint = clip_footprint(&clips[idx]);
            clips[idx].in_point = *in_point;
            clips[idx].out_point = *out_point;
            let new_footprint = clip_footprint(&clips[idx]);
            // Shift later same-track clips by the change in footprint: shrink pulls
            // them left (closes the gap), grow pushes them right. Only when both
            // footprints are known (as `RippleDelete` requires a known footprint).
            if let (Some(old), Some(new)) = (old_footprint, new_footprint) {
                for c in clips.iter_mut() {
                    if c.offset > clip_offset {
                        c.offset = if new >= old {
                            c.offset.saturating_add(new.saturating_sub(old))
                        } else {
                            c.offset.saturating_sub(old.saturating_sub(new))
                        };
                    }
                }
            }
        }
        Command::GroupClips { clips } => {
            if !clips.is_empty() {
                let g = GroupId::from_raw(next.next_group_id);
                // Assign in one pass; a missing clip returns Err, dropping the
                // cloned `next` so the input timeline is unchanged (atomic).
                for id in clips {
                    find_clip_mut(&mut next, *id)
                        .ok_or(EditError::ClipNotFound { id: *id })?
                        .group = Some(g);
                }
                next.next_group_id += 1;
            }
        }
        Command::UngroupClips { clip } => {
            let group = find_clip_mut(&mut next, *clip)
                .ok_or(EditError::ClipNotFound { id: *clip })?
                .group;
            if let Some(g) = group {
                for c in all_clips_mut(&mut next) {
                    if c.group == Some(g) {
                        c.group = None;
                    }
                }
            }
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
        Command::AddMarker { marker } => {
            let mut marker = marker.clone();
            marker.id = MarkerId::from_raw(next.next_marker_id);
            next.markers.push(marker);
            next.next_marker_id += 1;
        }
        Command::RemoveMarker { marker } => {
            let before = next.markers.len();
            next.markers.retain(|m| m.id != *marker);
            if next.markers.len() == before {
                return Err(EditError::MarkerNotFound { id: *marker });
            }
        }
        Command::MoveMarker { marker, pts } => {
            let m = next
                .markers
                .iter_mut()
                .find(|m| m.id == *marker)
                .ok_or(EditError::MarkerNotFound { id: *marker })?;
            m.pts = *pts;
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

/// Finds the track list holding the clip with `id` and the clip's index in it.
fn find_clip_track_mut(timeline: &mut Timeline, id: ClipId) -> Option<(&mut Vec<Clip>, usize)> {
    for track in &mut timeline.video_tracks {
        if let Some(idx) = track.clips.iter().position(|c| c.id == id) {
            return Some((&mut track.clips, idx));
        }
    }
    for track in &mut timeline.audio_tracks {
        if let Some(idx) = track.clips.iter().position(|c| c.id == id) {
            return Some((&mut track.clips, idx));
        }
    }
    None
}

/// Iterates every clip in the document (video tracks then audio tracks), mutably.
fn all_clips_mut(timeline: &mut Timeline) -> impl Iterator<Item = &mut Clip> {
    timeline
        .video_tracks
        .iter_mut()
        .chain(timeline.audio_tracks.iter_mut())
        .flat_map(|tr| tr.clips.iter_mut())
}

/// Collects the ids of every clip linked into `group`.
fn collect_group_ids(timeline: &Timeline, group: GroupId) -> Vec<ClipId> {
    timeline
        .video_tracks
        .iter()
        .chain(timeline.audio_tracks.iter())
        .flat_map(|tr| tr.clips.iter())
        .filter(|c| c.group == Some(group))
        .map(|c| c.id)
        .collect()
}

/// Shifts every member of `group` except `except` by the offset delta `new - old`
/// (saturating), so the group keeps its relative timing when one member moves.
fn shift_group_offsets(
    timeline: &mut Timeline,
    group: GroupId,
    except: ClipId,
    old: Duration,
    new: Duration,
) {
    for c in all_clips_mut(timeline) {
        if c.id != except && c.group == Some(group) {
            c.offset = if new >= old {
                c.offset.saturating_add(new.saturating_sub(old))
            } else {
                c.offset.saturating_sub(old.saturating_sub(new))
            };
        }
    }
}

/// Removes the clip with `id` and closes the gap on its track: later same-track
/// clips shift left by the removed clip's timeline footprint. No-op when the clip
/// is absent or its footprint is unknown (a plain remove, no shift).
fn ripple_delete_one(timeline: &mut Timeline, clip: ClipId) {
    let Some((clips, idx)) = find_clip_track_mut(timeline, clip) else {
        return;
    };
    let removed_offset = clips[idx].offset;
    let footprint = clip_footprint(&clips[idx]);
    clips.remove(idx);
    if let Some(shift) = footprint {
        for c in clips.iter_mut() {
            if c.offset > removed_offset {
                c.offset = c.offset.saturating_sub(shift);
            }
        }
    }
}

/// Splits `orig` at timeline position `at` into `(left, right)`, or `None` when the
/// cut is not strictly inside the clip's timeline span.
///
/// The source advances `speed` times as fast as the timeline, so the source split
/// point is `in + (at - offset) * speed`. The left half keeps the leading
/// transition/fade-in and ends at the cut (its trailing fade is cleared); the right
/// half starts at the cut, keeps the trailing fade-out, and clears the leading
/// transition/fade-in. The right half's id is left unset for the caller to stamp.
fn split_clip(orig: &Clip, at: Duration) -> Option<(Clip, Clip)> {
    // Timeline elapsed from the clip start to the cut; must be strictly positive.
    let elapsed = at.checked_sub(orig.offset)?;
    if elapsed.is_zero() {
        return None;
    }
    let in_pt = orig.in_point.unwrap_or(Duration::ZERO);
    let source_advance = Duration::try_from_secs_f64(elapsed.as_secs_f64() * orig.speed).ok()?;
    // `checked_add` keeps this panic-free even for an absurd `at`/speed.
    let source_split = in_pt.checked_add(source_advance)?;
    if source_split <= in_pt {
        return None; // degenerate (e.g. non-positive speed): left half would be empty
    }
    if let Some(out) = orig.out_point
        && source_split >= out
    {
        return None; // the cut is at or past the clip's end
    }

    let mut left = orig.clone();
    left.out_point = Some(source_split);
    left.fade_out = Duration::ZERO; // the left half now ends at a hard cut

    let mut right = orig.clone();
    right.in_point = Some(source_split);
    right.offset = at;
    right.transition = None; // a hard cut carries no leading transition/fade
    right.transition_duration = Duration::ZERO;
    right.fade_in = Duration::ZERO;

    Some((left, right))
}

/// Removes the clip with `id` from whichever track holds it and returns it.
fn take_clip(timeline: &mut Timeline, id: ClipId) -> Option<Clip> {
    for tr in timeline
        .video_tracks
        .iter_mut()
        .chain(timeline.audio_tracks.iter_mut())
    {
        if let Some(pos) = tr.clips.iter().position(|c| c.id == id) {
            return Some(tr.clips.remove(pos));
        }
    }
    None
}

/// The clip's timeline footprint: its source duration divided by `speed`, or `None`
/// when the source runs to end-of-file (`out_point` unset).
pub(crate) fn clip_footprint(clip: &Clip) -> Option<Duration> {
    let source = clip.duration()?;
    Duration::try_from_secs_f64(source.as_secs_f64() / clip.speed.max(0.01)).ok()
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

    /// A one-video-track timeline with two clips at the given second offsets.
    fn two_clips_at(off0: u64, off1: u64) -> Timeline {
        Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("a.mp4").offset(Duration::from_secs(off0)),
                Clip::new("b.mp4").offset(Duration::from_secs(off1)),
            ])
            .build()
            .unwrap()
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
        assert_eq!(
            added.source_path().and_then(std::path::Path::to_str),
            Some("added.mp4")
        );
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
            out.video_tracks()[0].clips[0]
                .source_path()
                .and_then(std::path::Path::to_str),
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
        assert_eq!(
            c.source_path().and_then(std::path::Path::to_str),
            Some("patched.mp4")
        );
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
            out.video_tracks()[0].clips[0]
                .source_path()
                .and_then(std::path::Path::to_str),
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
            t.video_tracks()[0].clips[0]
                .source_path()
                .and_then(std::path::Path::to_str),
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
            out.audio_tracks()[0].clips[0]
                .source_path()
                .and_then(std::path::Path::to_str),
            Some("patched.mp3")
        );
        assert_eq!(out.audio_tracks()[0].clips[0].id, id);
    }

    /// A single-video-track timeline holding `clip`, and that clip's id.
    fn split_setup(clip: Clip) -> (Timeline, ClipId) {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![clip])
            .build()
            .unwrap();
        let id = t.video_tracks()[0].clips[0].id;
        (t, id)
    }

    #[test]
    fn apply_split_clip_should_produce_two_contiguous_clips() {
        let (t, id) = split_setup(Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10)));
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(4),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips.len(), 2);
        // Left keeps the id and ends at the cut.
        assert_eq!(clips[0].id, id);
        assert_eq!(clips[0].offset, Duration::ZERO);
        assert_eq!(clips[0].out_point, Some(Duration::from_secs(4)));
        // Right gets a fresh id and starts at the cut, running to the original end.
        assert!(clips[1].id.is_set());
        assert_ne!(clips[1].id, id);
        assert_eq!(clips[1].offset, Duration::from_secs(4));
        assert_eq!(clips[1].in_point, Some(Duration::from_secs(4)));
        assert_eq!(clips[1].out_point, Some(Duration::from_secs(10)));
    }

    #[test]
    fn apply_split_clip_should_preserve_properties_on_both_halves() {
        let mut clip = Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10));
        clip.brightness = 0.5;
        clip.volume_db = -6.0;
        let (t, id) = split_setup(clip);
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(4),
            },
        )
        .unwrap();
        for c in &out.video_tracks()[0].clips {
            assert!((c.brightness - 0.5).abs() < f32::EPSILON);
            assert!((c.volume_db + 6.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn apply_split_clip_should_move_fades_and_transition() {
        use ff_filter::XfadeTransition;
        let mut clip = Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10));
        clip.fade_in = Duration::from_millis(500);
        clip.fade_out = Duration::from_millis(800);
        clip.transition = Some(XfadeTransition::Fade);
        clip.transition_duration = Duration::from_millis(300);
        let (t, id) = split_setup(clip);
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(4),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        // Left keeps the leading fade-in + transition and clears the trailing fade.
        assert_eq!(clips[0].fade_in, Duration::from_millis(500));
        assert_eq!(clips[0].transition, Some(XfadeTransition::Fade));
        assert_eq!(clips[0].fade_out, Duration::ZERO);
        // Right clears the leading fade-in + transition and keeps the trailing fade.
        assert_eq!(clips[1].fade_in, Duration::ZERO);
        assert_eq!(clips[1].transition, None);
        assert_eq!(clips[1].fade_out, Duration::from_millis(800));
    }

    #[test]
    fn apply_split_clip_should_map_source_position_with_speed() {
        let mut clip = Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10));
        clip.speed = 2.0;
        let (t, id) = split_setup(clip);
        // At timeline 2s the source has advanced 2s * 2.0 = 4s.
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(2),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips[0].out_point, Some(Duration::from_secs(4)));
        assert_eq!(clips[1].in_point, Some(Duration::from_secs(4)));
        assert_eq!(clips[1].offset, Duration::from_secs(2));
    }

    #[test]
    fn apply_split_clip_should_split_an_open_ended_clip() {
        // No trim: the clip runs to end-of-file (out_point is None).
        let (t, id) = split_setup(Clip::new("a.mp4"));
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(3),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips[0].out_point, Some(Duration::from_secs(3)));
        assert_eq!(clips[1].in_point, Some(Duration::from_secs(3)));
        assert_eq!(clips[1].out_point, None, "the right half still runs to EOF");
        assert_eq!(clips[1].offset, Duration::from_secs(3));
    }

    #[test]
    fn apply_split_clip_at_the_start_should_err() {
        let (t, id) = split_setup(Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10)));
        let err = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::ZERO,
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            EditError::SplitOutOfRange {
                clip: id,
                at: Duration::ZERO
            }
        );
    }

    #[test]
    fn apply_split_clip_past_the_end_should_err() {
        let (t, id) = split_setup(Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10)));
        let err = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(10),
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            EditError::SplitOutOfRange {
                clip: id,
                at: Duration::from_secs(10)
            }
        );
    }

    #[test]
    fn apply_split_unknown_clip_should_err() {
        let (t, _id) =
            split_setup(Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(10)));
        let err = apply(
            &t,
            &Command::SplitClip {
                clip: ClipId::UNSET,
                at: Duration::from_secs(4),
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: ClipId::UNSET });
    }

    #[test]
    fn apply_split_clip_should_split_a_clip_on_an_audio_track() {
        // `find_clip_track_mut` scans both lists, so SplitClip resolves an audio clip.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4")])
            .audio_track(vec![
                Clip::new("a.mp3").trim(Duration::ZERO, Duration::from_secs(8)),
            ])
            .build()
            .unwrap();
        let id = t.audio_tracks()[0].clips[0].id;
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(3),
            },
        )
        .unwrap();
        let clips = &out.audio_tracks()[0].clips;
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].out_point, Some(Duration::from_secs(3)));
        assert_eq!(clips[1].in_point, Some(Duration::from_secs(3)));
        assert_eq!(clips[1].out_point, Some(Duration::from_secs(8)));
        assert!(clips[1].id.is_set());
        assert_ne!(clips[1].id, id);
    }

    #[test]
    fn apply_move_clip_to_track_should_preserve_id_and_properties() {
        let mut clip = Clip::new("a.mp4");
        clip.brightness = 0.5;
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![clip])
            .video_track(vec![]) // empty destination track
            .build()
            .unwrap();
        let clip_id = t.video_tracks()[0].clips[0].id;
        let to = t.video_tracks()[1].id;
        let out = apply(
            &t,
            &Command::MoveClipToTrack {
                clip: clip_id,
                to,
                offset: Duration::from_secs(5),
            },
        )
        .unwrap();
        assert!(out.video_tracks()[0].clips.is_empty());
        let moved = &out.video_tracks()[1].clips[0];
        assert_eq!(moved.id, clip_id, "the id is preserved across the move");
        assert!((moved.brightness - 0.5).abs() < f32::EPSILON);
        assert_eq!(moved.offset, Duration::from_secs(5));
    }

    #[test]
    fn apply_move_clip_to_missing_track_should_err_and_not_change() {
        let (t, id) = split_setup(Clip::new("a.mp4"));
        let err = apply(
            &t,
            &Command::MoveClipToTrack {
                clip: id,
                to: TrackId::UNSET,
                offset: Duration::from_secs(1),
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::TrackNotFound { id: TrackId::UNSET });
        assert_eq!(t.video_tracks()[0].clips.len(), 1, "timeline unchanged");
    }

    #[test]
    fn apply_move_missing_clip_should_err() {
        let (t, _id) = split_setup(Clip::new("a.mp4"));
        let to = t.video_tracks()[0].id;
        let err = apply(
            &t,
            &Command::MoveClipToTrack {
                clip: ClipId::UNSET,
                to,
                offset: Duration::ZERO,
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: ClipId::UNSET });
    }

    #[test]
    fn apply_move_clip_to_track_should_preserve_effects_and_transition() {
        use ff_filter::{FilterStep, XfadeTransition};
        let mut clip = Clip::new("a.mp4");
        clip.transition = Some(XfadeTransition::Fade);
        clip.transition_duration = Duration::from_millis(300);
        clip.video_effects.push(FilterStep::Lut3d {
            path: "look.cube".into(),
        });
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![clip])
            .video_track(vec![])
            .build()
            .unwrap();
        let clip_id = t.video_tracks()[0].clips[0].id;
        let to = t.video_tracks()[1].id;
        let out = apply(
            &t,
            &Command::MoveClipToTrack {
                clip: clip_id,
                to,
                offset: Duration::ZERO,
            },
        )
        .unwrap();
        let moved = &out.video_tracks()[1].clips[0];
        assert_eq!(moved.transition, Some(XfadeTransition::Fade));
        assert_eq!(moved.transition_duration, Duration::from_millis(300));
        assert_eq!(moved.video_effects.len(), 1);
    }

    #[test]
    fn apply_move_clip_same_track_should_re_offset() {
        let (t, id) = split_setup(Clip::new("a.mp4"));
        let to = t.video_tracks()[0].id;
        let out = apply(
            &t,
            &Command::MoveClipToTrack {
                clip: id,
                to,
                offset: Duration::from_secs(7),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 1);
        assert_eq!(
            out.video_tracks()[0].clips[0].offset,
            Duration::from_secs(7)
        );
        assert_eq!(out.video_tracks()[0].clips[0].id, id);
    }

    /// Three back-to-back 4s clips at offsets 0/4/8 on one video track.
    fn ripple_setup() -> Timeline {
        let mk = |name: &str, off: u64| {
            Clip::new(name)
                .trim(Duration::ZERO, Duration::from_secs(4))
                .offset(Duration::from_secs(off))
        };
        Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![mk("a.mp4", 0), mk("b.mp4", 4), mk("c.mp4", 8)])
            .build()
            .unwrap()
    }

    #[test]
    fn apply_ripple_delete_should_close_the_gap() {
        let t = ripple_setup();
        let b_id = t.video_tracks()[0].clips[1].id;
        let out = apply(&t, &Command::RippleDelete { clip: b_id }).unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips.len(), 2);
        // `a` (offset 0) stays; `c` (was 8) shifts left by b's footprint (4) to 4.
        assert_eq!(
            clips[0].source_path().and_then(std::path::Path::to_str),
            Some("a.mp4")
        );
        assert_eq!(clips[0].offset, Duration::ZERO);
        assert_eq!(
            clips[1].source_path().and_then(std::path::Path::to_str),
            Some("c.mp4")
        );
        assert_eq!(clips[1].offset, Duration::from_secs(4));
    }

    #[test]
    fn apply_ripple_delete_should_not_disturb_other_tracks() {
        let mk = |name: &str, off: u64| {
            Clip::new(name)
                .trim(Duration::ZERO, Duration::from_secs(4))
                .offset(Duration::from_secs(off))
        };
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![mk("a.mp4", 0), mk("b.mp4", 4)])
            .video_track(vec![Clip::new("o.mp4").offset(Duration::from_secs(4))])
            .build()
            .unwrap();
        let a_id = t.video_tracks()[0].clips[0].id;
        let out = apply(&t, &Command::RippleDelete { clip: a_id }).unwrap();
        assert_eq!(out.video_tracks()[0].clips[0].offset, Duration::ZERO);
        assert_eq!(
            out.video_tracks()[1].clips[0].offset,
            Duration::from_secs(4),
            "the other track is untouched"
        );
    }

    #[test]
    fn apply_ripple_delete_should_shift_by_speed_scaled_footprint() {
        // `a`: source 0..10 at speed 2 -> timeline footprint 5. `b` starts at 5.
        let mut a = Clip::new("a.mp4")
            .trim(Duration::ZERO, Duration::from_secs(10))
            .offset(Duration::ZERO);
        a.speed = 2.0;
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![a, Clip::new("b.mp4").offset(Duration::from_secs(5))])
            .build()
            .unwrap();
        let a_id = t.video_tracks()[0].clips[0].id;
        let out = apply(&t, &Command::RippleDelete { clip: a_id }).unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[0].offset,
            Duration::ZERO,
            "shift uses the speed-scaled footprint (5), not the source duration (10)"
        );
    }

    #[test]
    fn apply_ripple_delete_open_ended_should_just_remove() {
        // `a` is open-ended (no trim); its footprint is unknown, so nothing shifts.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("a.mp4").offset(Duration::ZERO),
                Clip::new("b.mp4").offset(Duration::from_secs(5)),
            ])
            .build()
            .unwrap();
        let a_id = t.video_tracks()[0].clips[0].id;
        let out = apply(&t, &Command::RippleDelete { clip: a_id }).unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips.len(), 1);
        assert_eq!(
            clips[0].source_path().and_then(std::path::Path::to_str),
            Some("b.mp4")
        );
        assert_eq!(
            clips[0].offset,
            Duration::from_secs(5),
            "no shift when the removed clip's footprint is unknown"
        );
    }

    #[test]
    fn apply_ripple_trim_shorter_should_close_the_gap() {
        // b (offset 4, footprint 4) trimmed to source 0..2 (footprint 2, delta -2):
        // c (was 8) pulls left by 2 to 6; b's own offset is unchanged.
        let t = ripple_setup();
        let b_id = t.video_tracks()[0].clips[1].id;
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: b_id,
                in_point: Some(Duration::ZERO),
                out_point: Some(Duration::from_secs(2)),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips[0].offset, Duration::ZERO, "a unchanged");
        assert_eq!(
            clips[1].offset,
            Duration::from_secs(4),
            "b offset unchanged"
        );
        assert_eq!(
            clips[1].out_point,
            Some(Duration::from_secs(2)),
            "b trimmed"
        );
        assert_eq!(
            clips[2].offset,
            Duration::from_secs(6),
            "c pulled left by 2"
        );
    }

    #[test]
    fn apply_ripple_trim_longer_should_push_later_clips() {
        // b trimmed to source 0..6 (footprint 6, delta +2): c (was 8) pushes to 10.
        let t = ripple_setup();
        let b_id = t.video_tracks()[0].clips[1].id;
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: b_id,
                in_point: Some(Duration::ZERO),
                out_point: Some(Duration::from_secs(6)),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(
            clips[2].offset,
            Duration::from_secs(10),
            "c pushed right by 2"
        );
    }

    #[test]
    fn apply_ripple_trim_should_not_disturb_other_tracks() {
        let mk = |name: &str, off: u64| {
            Clip::new(name)
                .trim(Duration::ZERO, Duration::from_secs(4))
                .offset(Duration::from_secs(off))
        };
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![mk("a.mp4", 0), mk("b.mp4", 4)])
            .video_track(vec![Clip::new("o.mp4").offset(Duration::from_secs(4))])
            .build()
            .unwrap();
        let a_id = t.video_tracks()[0].clips[0].id;
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: a_id,
                in_point: Some(Duration::ZERO),
                out_point: Some(Duration::from_secs(2)),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[1].offset,
            Duration::from_secs(2),
            "b pulled left by 2"
        );
        assert_eq!(
            out.video_tracks()[1].clips[0].offset,
            Duration::from_secs(4),
            "the other track is untouched"
        );
    }

    #[test]
    fn apply_ripple_trim_should_shift_by_speed_scaled_footprint() {
        // a: source 0..10 at speed 2 -> footprint 5. b starts at 5. Trim a to
        // source 0..4 (footprint 2, delta -3): b pulls left to 2.
        let mut a = Clip::new("a.mp4")
            .trim(Duration::ZERO, Duration::from_secs(10))
            .offset(Duration::ZERO);
        a.speed = 2.0;
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![a, Clip::new("b.mp4").offset(Duration::from_secs(5))])
            .build()
            .unwrap();
        let a_id = t.video_tracks()[0].clips[0].id;
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: a_id,
                in_point: Some(Duration::ZERO),
                out_point: Some(Duration::from_secs(4)),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[1].offset,
            Duration::from_secs(2),
            "shift uses the speed-scaled footprint delta (5 -> 2 = -3), not the source delta"
        );
    }

    #[test]
    fn apply_ripple_trim_unknown_footprint_should_trim_without_shifting() {
        // Trimming b to an unbounded out-point makes its new footprint unknown, so
        // nothing shifts, but the trim still applies.
        let t = ripple_setup();
        let b_id = t.video_tracks()[0].clips[1].id;
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: b_id,
                in_point: Some(Duration::ZERO),
                out_point: None,
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips[1].out_point, None, "b trim applied");
        assert_eq!(
            clips[2].offset,
            Duration::from_secs(8),
            "c not shifted when the new footprint is unknown"
        );
    }

    #[test]
    fn apply_ripple_trim_missing_clip_should_err() {
        let t = ripple_setup();
        let missing = ClipId::from_raw(9999);
        let err = apply(
            &t,
            &Command::RippleTrim {
                clip: missing,
                in_point: Some(Duration::ZERO),
                out_point: Some(Duration::from_secs(2)),
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: missing });
    }

    #[test]
    fn apply_ripple_trim_head_should_shift_by_footprint_delta() {
        // Head-trim: b's in-point advances to 2 (source 2..4 -> footprint 2, delta
        // -2). b's own offset is unchanged; c (was 8) pulls left to 6.
        let t = ripple_setup();
        let b_id = t.video_tracks()[0].clips[1].id;
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: b_id,
                in_point: Some(Duration::from_secs(2)),
                out_point: Some(Duration::from_secs(4)),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(
            clips[1].offset,
            Duration::from_secs(4),
            "b offset unchanged"
        );
        assert_eq!(
            clips[1].in_point,
            Some(Duration::from_secs(2)),
            "b head trimmed"
        );
        assert_eq!(
            clips[2].offset,
            Duration::from_secs(6),
            "c pulled left by 2"
        );
    }

    #[test]
    fn apply_ripple_trim_from_unbounded_should_trim_without_shifting() {
        // The clip starts open-ended (footprint unknown). Trimming it to a bounded
        // out-point cannot know how much it shrank, so nothing shifts, but the trim
        // applies.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("a.mp4").offset(Duration::ZERO),
                Clip::new("b.mp4").offset(Duration::from_secs(5)),
            ])
            .build()
            .unwrap();
        let a_id = t.video_tracks()[0].clips[0].id;
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: a_id,
                in_point: Some(Duration::ZERO),
                out_point: Some(Duration::from_secs(2)),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(
            clips[0].out_point,
            Some(Duration::from_secs(2)),
            "a trim applied"
        );
        assert_eq!(
            clips[1].offset,
            Duration::from_secs(5),
            "no shift when the old footprint was unknown"
        );
    }

    #[test]
    fn apply_ripple_delete_missing_clip_should_err() {
        let (t, _id) = split_setup(Clip::new("a.mp4"));
        let err = apply(
            &t,
            &Command::RippleDelete {
                clip: ClipId::UNSET,
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::ClipNotFound { id: ClipId::UNSET });
    }

    // ── markers ─────────────────────────────────────────────────────────────

    #[test]
    fn apply_add_marker_should_append_with_fresh_id() {
        let t = timeline_with(1);
        let out = apply(
            &t,
            &Command::AddMarker {
                marker: Marker::new(Duration::from_secs(2)).with_name("intro"),
            },
        )
        .unwrap();
        assert_eq!(out.markers().len(), 1);
        let m = &out.markers()[0];
        assert!(m.id.is_set(), "an added marker gets a set id");
        assert_eq!(m.pts, Duration::from_secs(2));
        assert_eq!(m.name.as_deref(), Some("intro"));
    }

    #[test]
    fn apply_add_marker_should_stamp_fresh_id_over_incoming() {
        let t = timeline_with(1);
        let mut incoming = Marker::new(Duration::ZERO);
        incoming.id = MarkerId::from_raw(999);
        let out = apply(&t, &Command::AddMarker { marker: incoming }).unwrap();
        assert_ne!(
            out.markers()[0].id,
            MarkerId::from_raw(999),
            "the incoming id is replaced with a fresh one"
        );
        assert!(out.markers()[0].id.is_set());
    }

    #[test]
    fn apply_add_marker_should_assign_distinct_ids() {
        let t = timeline_with(1);
        let t = apply(
            &t,
            &Command::AddMarker {
                marker: Marker::new(Duration::from_secs(1)),
            },
        )
        .unwrap();
        let t = apply(
            &t,
            &Command::AddMarker {
                marker: Marker::new(Duration::from_secs(2)),
            },
        )
        .unwrap();
        assert_ne!(
            t.markers()[0].id,
            t.markers()[1].id,
            "each marker gets a distinct id"
        );
    }

    #[test]
    fn apply_remove_marker_should_drop_it_by_id() {
        let t = timeline_with(1);
        let t = apply(
            &t,
            &Command::AddMarker {
                marker: Marker::new(Duration::from_secs(1)),
            },
        )
        .unwrap();
        let id = t.markers()[0].id;
        let out = apply(&t, &Command::RemoveMarker { marker: id }).unwrap();
        assert!(out.markers().is_empty());
    }

    #[test]
    fn apply_remove_marker_missing_should_err() {
        let t = timeline_with(1);
        let missing = MarkerId::from_raw(9999);
        let err = apply(&t, &Command::RemoveMarker { marker: missing }).unwrap_err();
        assert_eq!(err, EditError::MarkerNotFound { id: missing });
    }

    #[test]
    fn apply_move_marker_should_set_pts() {
        let t = timeline_with(1);
        let t = apply(
            &t,
            &Command::AddMarker {
                marker: Marker::new(Duration::from_secs(1)),
            },
        )
        .unwrap();
        let id = t.markers()[0].id;
        let out = apply(
            &t,
            &Command::MoveMarker {
                marker: id,
                pts: Duration::from_secs(5),
            },
        )
        .unwrap();
        assert_eq!(out.markers()[0].pts, Duration::from_secs(5));
        assert_eq!(out.markers()[0].id, id, "the marker id is unchanged");
    }

    #[test]
    fn apply_move_marker_missing_should_err() {
        let t = timeline_with(1);
        let missing = MarkerId::from_raw(9999);
        let err = apply(
            &t,
            &Command::MoveMarker {
                marker: missing,
                pts: Duration::from_secs(1),
            },
        )
        .unwrap_err();
        assert_eq!(err, EditError::MarkerNotFound { id: missing });
    }

    #[test]
    fn apply_add_marker_should_not_change_clips() {
        let t = timeline_with(2);
        let before: Vec<_> = t.video_tracks()[0].clips.iter().map(|c| c.id).collect();
        let out = apply(
            &t,
            &Command::AddMarker {
                marker: Marker::new(Duration::ZERO),
            },
        )
        .unwrap();
        let after: Vec<_> = out.video_tracks()[0].clips.iter().map(|c| c.id).collect();
        assert_eq!(before, after, "adding a marker does not touch clips");
    }

    // ── clip linking / groups (#1456) ─────────────────────────────────────────

    #[test]
    fn apply_group_clips_should_link_members_with_one_fresh_group() {
        let t = timeline_with(3);
        let (a, b) = (clip_id(&t, 0), clip_id(&t, 1));
        let out = apply(&t, &Command::GroupClips { clips: vec![a, b] }).unwrap();
        let ga = out.video_tracks()[0].clips[0].group;
        let gb = out.video_tracks()[0].clips[1].group;
        let gc = out.video_tracks()[0].clips[2].group;
        assert!(
            ga.is_some() && ga == gb,
            "grouped members share one group id"
        );
        assert!(ga.unwrap().is_set());
        assert_eq!(gc, None, "an unnamed clip stays ungrouped");
    }

    #[test]
    fn apply_group_clips_missing_clip_should_err_and_change_nothing() {
        let t = timeline_with(2);
        let a = clip_id(&t, 0);
        let res = apply(
            &t,
            &Command::GroupClips {
                clips: vec![a, ClipId::UNSET],
            },
        );
        assert!(matches!(res, Err(EditError::ClipNotFound { .. })));
    }

    #[test]
    fn apply_group_clips_should_reassign_an_already_grouped_clip() {
        let t = timeline_with(3);
        let (a, b, c) = (clip_id(&t, 0), clip_id(&t, 1), clip_id(&t, 2));
        let t = apply(&t, &Command::GroupClips { clips: vec![a, b] }).unwrap();
        let out = apply(&t, &Command::GroupClips { clips: vec![b, c] }).unwrap();
        let ga = out.video_tracks()[0].clips[0].group;
        let gb = out.video_tracks()[0].clips[1].group;
        let gc = out.video_tracks()[0].clips[2].group;
        assert_ne!(gb, ga, "b left a's group");
        assert_eq!(gb, gc, "b joined c's group");
    }

    #[test]
    fn apply_ungroup_clips_should_clear_group_of_all_members() {
        let t = timeline_with(2);
        let (a, b) = (clip_id(&t, 0), clip_id(&t, 1));
        let t = apply(&t, &Command::GroupClips { clips: vec![a, b] }).unwrap();
        let out = apply(&t, &Command::UngroupClips { clip: a }).unwrap();
        assert_eq!(out.video_tracks()[0].clips[0].group, None);
        assert_eq!(out.video_tracks()[0].clips[1].group, None);
    }

    #[test]
    fn apply_ungroup_clips_ungrouped_should_be_noop() {
        let t = timeline_with(1);
        let a = clip_id(&t, 0);
        let out = apply(&t, &Command::UngroupClips { clip: a }).unwrap();
        assert_eq!(out.video_tracks()[0].clips[0].group, None);
    }

    #[test]
    fn apply_move_clip_should_carry_grouped_member_by_the_same_delta() {
        let t = two_clips_at(0, 10);
        let (a, b) = (clip_id(&t, 0), clip_id(&t, 1));
        let t = apply(&t, &Command::GroupClips { clips: vec![a, b] }).unwrap();
        // Move a right (0 -> 5, delta +5): b keeps the 10s gap (10 -> 15).
        let out = apply(
            &t,
            &Command::MoveClip {
                clip: a,
                offset: Duration::from_secs(5),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[0].offset,
            Duration::from_secs(5)
        );
        assert_eq!(
            out.video_tracks()[0].clips[1].offset,
            Duration::from_secs(15)
        );
        // Move a left (5 -> 2, delta -3): b 15 -> 12.
        let out2 = apply(
            &out,
            &Command::MoveClip {
                clip: a,
                offset: Duration::from_secs(2),
            },
        )
        .unwrap();
        assert_eq!(
            out2.video_tracks()[0].clips[1].offset,
            Duration::from_secs(12)
        );
    }

    #[test]
    fn apply_move_clip_ungrouped_should_not_shift_other_clips() {
        let t = two_clips_at(0, 10);
        let a = clip_id(&t, 0);
        let out = apply(
            &t,
            &Command::MoveClip {
                clip: a,
                offset: Duration::from_secs(5),
            },
        )
        .unwrap();
        assert_eq!(
            out.video_tracks()[0].clips[1].offset,
            Duration::from_secs(10),
            "a non-grouped move leaves other clips put"
        );
    }

    #[test]
    fn apply_move_clip_to_track_should_carry_grouped_member_and_keep_its_track() {
        // A/V pair (video v, audio a, both @0), grouped. Move v to a 2nd video track @5s.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("v.mp4")])
            .audio_track(vec![Clip::new("a.mp3")])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let t = apply(
            &t,
            &Command::AddTrack {
                kind: TrackKind::Video,
            },
        )
        .unwrap();
        let dest = t.video_tracks()[1].id;
        let out = apply(
            &t,
            &Command::MoveClipToTrack {
                clip: v,
                to: dest,
                offset: Duration::from_secs(5),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[1].clips.len(), 1);
        assert_eq!(
            out.video_tracks()[1].clips[0].offset,
            Duration::from_secs(5)
        );
        // The linked audio clip shifts by the same delta but stays on its track.
        assert_eq!(out.audio_tracks()[0].clips.len(), 1);
        assert_eq!(
            out.audio_tracks()[0].clips[0].offset,
            Duration::from_secs(5)
        );
    }

    #[test]
    fn apply_ripple_delete_should_remove_the_whole_group() {
        // Grouped video v + audio a; an ungrouped video clip w must survive.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4"),
                Clip::new("w.mp4").offset(Duration::from_secs(30)),
            ])
            .audio_track(vec![Clip::new("a.mp3")])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let out = apply(&t, &Command::RippleDelete { clip: v }).unwrap();
        assert_eq!(
            out.video_tracks()[0].clips.len(),
            1,
            "grouped video clip removed, ungrouped one kept"
        );
        assert_eq!(
            out.audio_tracks()[0].clips.len(),
            0,
            "grouped audio clip removed"
        );
    }

    #[test]
    fn apply_trim_clip_should_not_propagate_to_grouped_member() {
        let t = timeline_with(2);
        let (a, b) = (clip_id(&t, 0), clip_id(&t, 1));
        let t = apply(&t, &Command::GroupClips { clips: vec![a, b] }).unwrap();
        let out = apply(
            &t,
            &Command::TrimClip {
                clip: a,
                in_point: Some(Duration::from_secs(1)),
                out_point: Some(Duration::from_secs(3)),
            },
        )
        .unwrap();
        // Trim is per-clip: the linked member's source window is untouched.
        assert_eq!(out.video_tracks()[0].clips[1].in_point, None);
        assert_eq!(out.video_tracks()[0].clips[1].out_point, None);
    }

    #[test]
    fn apply_split_clip_should_keep_both_halves_in_the_group() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(10)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v] }).unwrap();
        let g = t.video_tracks()[0].clips[0].group;
        assert!(g.is_some());
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: v,
                at: Duration::from_secs(4),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 2);
        assert_eq!(out.video_tracks()[0].clips[0].group, g);
        assert_eq!(
            out.video_tracks()[0].clips[1].group,
            g,
            "the right half inherits the group"
        );
    }
}
