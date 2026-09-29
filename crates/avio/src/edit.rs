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
use crate::effect::{ClipEffect, EffectKind};
use crate::ids::{ClipId, EffectId, GroupId, MarkerId, TrackId, TrackKind};
use crate::marker::Marker;
use crate::timeline::Timeline;
use crate::track::Track;

/// A per-clip property that [`Command::SetClipProperty`] can set.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipProperty {
    /// Overlay opacity in `[0.0, 1.0]`.
    Opacity(f32),
    /// Playback speed multiplier (`1.0` = normal).
    ///
    /// Clamped to [`MIN_SPEED`](crate::MIN_SPEED): zero or a negative value has
    /// no interpretation, since a clip's footprint is `duration / speed`.
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
    ///
    /// A grouped clip carries its linked members: each one's window moves by the
    /// same change expressed in timeline time, clamped to what that member can hold
    /// (ADR-0019). The addressed clip takes the given values unchanged.
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
    ///
    /// A grouped clip is razored together with every linked member whose span
    /// contains `at`; a member the cut does not cross is left whole. The left halves
    /// keep the original [`GroupId`] and the right halves take one fresh group, so
    /// audio and video stay linked across the cut while the two sides of it stop
    /// dragging each other (ADR-0019).
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
    ///
    /// A grouped clip carries its linked members, as [`TrimClip`](Self::TrimClip)
    /// does, and each member ripples the track it sits on, so a linked pair stays
    /// aligned after both gaps close.
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
    /// / [`TrimClip`](Self::TrimClip) / [`RippleTrim`](Self::RippleTrim) /
    /// [`SplitClip`](Self::SplitClip) on any member propagates to the whole group as
    /// one undo step (see [`apply`]). A placement carries the offset delta, a trim
    /// carries the timeline-time change of each edge, and a razor cuts at the same
    /// timeline position; ADR-0019 has the rules and what happens to a member that
    /// cannot follow.
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
    /// Append a typed effect to a clip's ordered effect list (assigned a fresh
    /// [`EffectId`]). The effect starts enabled.
    AddEffect {
        /// Clip to add the effect to.
        clip: ClipId,
        /// The typed effect to append.
        kind: EffectKind,
    },
    /// Remove the effect with id `effect` from clip `clip`.
    RemoveEffect {
        /// Clip owning the effect.
        clip: ClipId,
        /// Effect to remove.
        effect: EffectId,
    },
    /// Replace the [`EffectKind`] of the effect with id `effect` on clip `clip`.
    ///
    /// This is how a parameter is set: the host sends the updated typed kind. The
    /// effect's id, position, and enabled state are preserved.
    SetEffectKind {
        /// Clip owning the effect.
        clip: ClipId,
        /// Effect to update.
        effect: EffectId,
        /// The replacement kind (carrying the new parameters).
        kind: EffectKind,
    },
    /// Enable or disable the effect with id `effect` on clip `clip`. A disabled
    /// effect is kept in the list (position and parameters preserved) but skipped
    /// during derivation.
    SetEffectEnabled {
        /// Clip owning the effect.
        clip: ClipId,
        /// Effect to toggle.
        effect: EffectId,
        /// New enabled state.
        enabled: bool,
    },
    /// Reorder a clip's effects. `order` must be a permutation of the clip's
    /// current effect ids (same set, no additions or omissions); otherwise the
    /// command fails with [`EditError::EffectNotFound`] and changes nothing.
    ReorderEffects {
        /// Clip whose effects are reordered.
        clip: ClipId,
        /// The effect ids in their new order.
        order: Vec<EffectId>,
    },
    /// Set the `lock` flag of the track with id `track`.
    ///
    /// A locked track refuses every other command that would change it (see
    /// [`apply`]); this one is the exception, because it is how the lock is
    /// released. Locking is an authoring constraint only: the derivation ignores
    /// the flag, so a locked track still renders and still previews.
    SetTrackLock {
        /// Track to lock or unlock.
        track: TrackId,
        /// The new flag value.
        lock: bool,
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
    /// An effect command named an effect id that clip `clip` does not carry (also
    /// used when a [`Command::ReorderEffects`] `order` is not a permutation of the
    /// clip's current effect ids).
    #[error("no effect with id {id:?} on clip {clip:?}")]
    EffectNotFound {
        /// The clip that was searched.
        clip: ClipId,
        /// The effect id that resolved to no effect.
        id: EffectId,
    },
    /// A [`Command::SetClip`] value carries an id that names a different clip.
    #[error("clip id mismatch: expected {expected:?}, value has {found:?}")]
    ClipIdMismatch {
        /// The target clip id the patch was addressed to.
        expected: ClipId,
        /// The (set) id found on the patch value.
        found: ClipId,
    },
    /// The edit would change a track whose [`lock`](crate::Track::lock) flag is set,
    /// or a clip on one.
    ///
    /// Release the lock with [`Command::SetTrackLock`], which is the one command a
    /// locked track still accepts.
    #[error("track {id:?} is locked; release it with Command::SetTrackLock")]
    TrackLocked {
        /// The locked track the edit would have changed.
        id: TrackId,
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
///
/// Returns [`EditError::TrackLocked`] when the edit would change a track whose
/// [`lock`](crate::Track::lock) flag is set, or a clip on one, or a clip grouped with
/// one. [`Command::SetTrackLock`] is the exception, because it releases the lock.
pub fn apply(timeline: &Timeline, command: &Command) -> Result<Timeline, EditError> {
    // A locked track refuses the edit before anything is cloned or changed.
    // `Command::Batch` re-enters here per sub-command, so a batch is covered by this
    // one guard and stays atomic (#1805, ADR-0021).
    if let Some(id) = locked_target(timeline, command) {
        return Err(EditError::TrackLocked { id });
    }
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
            // A caller-built clip may carry effects with UNSET (or stale) ids;
            // re-stamp them so effect ids stay document-unique and addressable.
            stamp_effect_ids(&mut new_clip, &mut next.next_effect_id);
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
            // Read the edges before writing them: the deltas the group follows are
            // this clip's change, expressed in timeline time (ADR-0019).
            let (d_in, d_out) = trim_deltas(c, *in_point, *out_point);
            let group = c.group;
            // The addressed clip takes what the caller asked for, unclamped; only
            // the members it carries are clamped to what they can hold.
            c.in_point = *in_point;
            c.out_point = *out_point;
            if let Some(g) = group {
                for member in collect_group_ids(&next, g) {
                    if member == *clip {
                        continue;
                    }
                    if let Some(m) = find_clip_mut(&mut next, member) {
                        let (new_in, new_out) = trimmed_edges(m, d_in, d_out);
                        m.in_point = new_in;
                        m.out_point = new_out;
                    }
                }
            }
        }
        Command::SetClipProperty { clip, property } => {
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            match property {
                // Match `Clip::with_opacity`, which clamps to the documented range.
                ClipProperty::Opacity(v) => c.opacity = v.clamp(0.0, 1.0),
                // Clamped like `Opacity` above: a non-positive speed has no
                // interpretation, and the edit path is where the model can refuse one
                // without changing an API that returns `Self` (#1816).
                ClipProperty::Speed(v) => c.speed = v.max(crate::MIN_SPEED),
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
            // An effect id this clip already carries survives the patch, so a host
            // editing one field does not have every effect renumbered underneath it
            // (#1814). An UNSET id, or one from another clip or document, is minted
            // fresh: the first was never assigned, and the second would either
            // duplicate an id or collide with a later mint.
            let known: Vec<EffectId> = find_clip_mut(&mut next, *clip)
                .ok_or(EditError::ClipNotFound { id: *clip })?
                .effects
                .iter()
                .map(|e| e.id)
                .collect();
            let mut counter = next.next_effect_id;
            let mut new_value = (**value).clone();
            new_value.id = *clip; // preserve identity
            stamp_unknown_effect_ids(&mut new_value, &known, &mut counter);
            let target =
                find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            *target = new_value;
            next.next_effect_id = counter;
        }
        Command::SplitClip { clip, at } => {
            let group = find_clip_mut(&mut next, *clip)
                .ok_or(EditError::ClipNotFound { id: *clip })?
                .group;
            // The right halves are linked to each other rather than to the left
            // ones, so razoring a group does not leave one group spanning both
            // sides of the cut, where moving either half would drag the other
            // (ADR-0019). An ungrouped clip's halves stay ungrouped.
            let right_group = group.map(|_| GroupId::from_raw(next.next_group_id));
            // The addressed clip is razored first, so a cut outside its span still
            // fails with `SplitOutOfRange` and leaves the timeline untouched.
            if !split_one(&mut next, *clip, *at, right_group) {
                return Err(EditError::SplitOutOfRange {
                    clip: *clip,
                    at: *at,
                });
            }
            if let Some(g) = group {
                // A member the cut does not cross keeps its single clip: a group
                // whose members are not aligned is still editable.
                for member in collect_group_ids(&next, g) {
                    if member != *clip {
                        split_one(&mut next, member, *at, right_group);
                    }
                }
                next.next_group_id += 1;
            }
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
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            let (d_in, d_out) = trim_deltas(c, *in_point, *out_point);
            let group = c.group;
            ripple_trim_one(&mut next, *clip, *in_point, *out_point);
            // Each member ripples its own track, so a linked pair stays aligned
            // after both tracks close their gaps.
            if let Some(g) = group {
                for member in collect_group_ids(&next, g) {
                    if member == *clip {
                        continue;
                    }
                    let Some(m) = find_clip_mut(&mut next, member) else {
                        continue;
                    };
                    let (new_in, new_out) = trimmed_edges(m, d_in, d_out);
                    ripple_trim_one(&mut next, member, new_in, new_out);
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
        Command::AddEffect { clip, kind } => {
            // Reserve a fresh id before the mutable clip borrow (as `AddClip` does).
            let id = EffectId::from_raw(next.next_effect_id);
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            c.effects.push(ClipEffect {
                id,
                enabled: true,
                kind: kind.clone(),
            });
            next.next_effect_id += 1;
        }
        Command::RemoveEffect { clip, effect } => {
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            let idx = c.effects.iter().position(|e| e.id == *effect).ok_or(
                EditError::EffectNotFound {
                    clip: *clip,
                    id: *effect,
                },
            )?;
            c.effects.remove(idx);
        }
        Command::SetEffectKind { clip, effect, kind } => {
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            let e = c.effects.iter_mut().find(|e| e.id == *effect).ok_or(
                EditError::EffectNotFound {
                    clip: *clip,
                    id: *effect,
                },
            )?;
            e.kind = kind.clone();
        }
        Command::SetEffectEnabled {
            clip,
            effect,
            enabled,
        } => {
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            let e = c.effects.iter_mut().find(|e| e.id == *effect).ok_or(
                EditError::EffectNotFound {
                    clip: *clip,
                    id: *effect,
                },
            )?;
            e.enabled = *enabled;
        }
        Command::ReorderEffects { clip, order } => {
            let c = find_clip_mut(&mut next, *clip).ok_or(EditError::ClipNotFound { id: *clip })?;
            // `order` must be a permutation of the current effect ids. Draining the
            // taken list as we consume `order` rejects unknown ids and duplicates
            // (a second reference finds nothing left); a non-empty remainder means
            // `order` omitted some effect. Any failure returns before assigning, so
            // the discarded `next` leaves the input timeline unchanged.
            let mut remaining = std::mem::take(&mut c.effects);
            let mut reordered = Vec::with_capacity(order.len());
            for id in order {
                let pos = remaining.iter().position(|e| e.id == *id).ok_or(
                    EditError::EffectNotFound {
                        clip: *clip,
                        id: *id,
                    },
                )?;
                reordered.push(remaining.remove(pos));
            }
            if let Some(leftover) = remaining.first() {
                return Err(EditError::EffectNotFound {
                    clip: *clip,
                    id: leftover.id,
                });
            }
            c.effects = reordered;
        }
        Command::SetTrackLock { track, lock } => {
            find_track_mut(&mut next, *track)
                .ok_or(EditError::TrackNotFound { id: *track })?
                .lock = *lock;
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

/// Stamps fresh, never-reused document ids onto a clip's effects, advancing
/// `next_id`. Used when a clip enters the document (`AddClip`) or a fresh clip is
/// created from an existing one (`SplitClip`'s right half), so effect ids stay
/// document-unique even for caller-built or cloned effects.
fn stamp_effect_ids(clip: &mut Clip, next_id: &mut u64) {
    for effect in &mut clip.effects {
        effect.id = EffectId::from_raw(*next_id);
        *next_id += 1;
    }
}

/// Mints ids for the effects of `clip` that this document does not already know,
/// leaving the ones it does alone.
///
/// `known` is what the clip being replaced was carrying. An id in it survives, so a
/// host that sends a clip back through [`Command::SetClip`] keeps the ids its effect
/// panel and undo entries are bound to (#1814, ADR-0001).
///
/// Anything else is minted fresh: `EffectId::UNSET` because it was never assigned,
/// and an id from another clip or another document because keeping it would put two
/// effects under one id, and because a value above `next_id` would collide with a
/// later mint.
fn stamp_unknown_effect_ids(clip: &mut Clip, known: &[EffectId], next_id: &mut u64) {
    // An id survives once. A patch can carry the same known id twice — a host that
    // duplicates an effect row clones the struct, id included — and keeping both
    // would put two effects under one id, which is the property ADR-0001 exists for.
    let mut kept: Vec<EffectId> = Vec::new();
    for effect in &mut clip.effects {
        if known.contains(&effect.id) && !kept.contains(&effect.id) {
            kept.push(effect.id);
            continue;
        }
        effect.id = EffectId::from_raw(*next_id);
        *next_id += 1;
    }
}

fn tracks_mut(timeline: &mut Timeline, kind: TrackKind) -> &mut Vec<Track> {
    match kind {
        TrackKind::Video => &mut timeline.video_tracks,
        TrackKind::Audio => &mut timeline.audio_tracks,
    }
}

/// The locked track this command would change, if any.
///
/// `None` when the command changes no track, changes only unlocked ones, or is
/// [`Command::SetTrackLock`], which a locked track still accepts because it is how
/// the lock is released (ADR-0021).
///
/// A clip-addressed command resolves the clip's own track. The commands that carry a
/// grouped edit to the rest of the group (ADR-0019) resolve the members' tracks too:
/// a linked pair half-edited because one side was locked would break the very sync
/// the group exists to protect.
fn locked_target(timeline: &Timeline, command: &Command) -> Option<TrackId> {
    let locked_track = |id: TrackId| locked_id(timeline, id);
    let locked_clip = |clip: ClipId| locked_holder(timeline, clip);
    let locked_group = |clip: ClipId| {
        let group = clip_of(timeline, clip)?.group;
        let members = group.map_or_else(|| vec![clip], |g| collect_group_ids(timeline, g));
        members.into_iter().find_map(locked_clip)
    };
    match command {
        // Clip edits that carry to the rest of the group.
        Command::MoveClip { clip, .. }
        | Command::TrimClip { clip, .. }
        | Command::RippleTrim { clip, .. }
        | Command::SplitClip { clip, .. }
        | Command::RippleDelete { clip }
        // Not a propagating edit, but it clears `group` on every member, so the
        // members' tracks are what it actually writes to.
        | Command::UngroupClips { clip } => locked_group(*clip),
        Command::MoveClipToTrack { clip, to, .. } => {
            locked_group(*clip).or_else(|| locked_track(*to))
        }
        // Clip edits that stop at the clip they name.
        Command::RemoveClip { clip }
        | Command::SetClipProperty { clip, .. }
        | Command::SetClip { clip, .. }
        | Command::AddEffect { clip, .. }
        | Command::RemoveEffect { clip, .. }
        | Command::SetEffectKind { clip, .. }
        | Command::SetEffectEnabled { clip, .. }
        | Command::ReorderEffects { clip, .. } => locked_clip(*clip),
        Command::GroupClips { clips } => clips.iter().copied().find_map(locked_clip),
        // Track edits.
        Command::AddClip { track, .. } | Command::RemoveTrack { track } => locked_track(*track),
        // A locked track still accepts this one, and the rest name no track: adding a
        // track, the markers, the canvas and the frame rate. `Batch` is covered by the
        // guard re-entering `apply` for each sub-command.
        Command::SetTrackLock { .. }
        | Command::AddTrack { .. }
        | Command::AddMarker { .. }
        | Command::RemoveMarker { .. }
        | Command::MoveMarker { .. }
        | Command::SetCanvas { .. }
        | Command::SetFrameRate { .. }
        | Command::Batch(_) => None,
    }
}

/// `Some(id)` when the track with `id` is locked.
fn locked_id(timeline: &Timeline, id: TrackId) -> Option<TrackId> {
    timeline
        .video_tracks
        .iter()
        .chain(timeline.audio_tracks.iter())
        .find(|tr| tr.id == id)
        .filter(|tr| tr.lock)
        .map(|tr| tr.id)
}

/// `Some(id)` when the track holding `clip` is locked.
fn locked_holder(timeline: &Timeline, clip: ClipId) -> Option<TrackId> {
    timeline
        .video_tracks
        .iter()
        .chain(timeline.audio_tracks.iter())
        .find(|tr| tr.clips.iter().any(|c| c.id == clip))
        .filter(|tr| tr.lock)
        .map(|tr| tr.id)
}

/// The clip with `id`, read-only.
fn clip_of(timeline: &Timeline, id: ClipId) -> Option<&Clip> {
    timeline
        .video_tracks
        .iter()
        .chain(timeline.audio_tracks.iter())
        .flat_map(|tr| tr.clips.iter())
        .find(|c| c.id == id)
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

/// The timeline-time change of one trim edge, in seconds, or `None` when it cannot
/// be expressed.
///
/// An unset in-point is exactly the start of the file, so an in-point edge always
/// yields a value. An unset out-point means "to end of file", a position `apply`
/// cannot know because it performs no I/O, so an out-point that is unset before or
/// after the trim yields `None` and does not propagate.
///
/// The source advances `speed` times as fast as the timeline, so dividing by it
/// turns a source-time change into the timeline-time change a viewer sees. `speed`
/// is floored the way [`clip_footprint`] floors it, so a degenerate value cannot
/// produce an unbounded delta.
fn edge_delta_secs(old: Option<Duration>, new: Option<Duration>, speed: f64) -> Option<f64> {
    Some((new?.as_secs_f64() - old?.as_secs_f64()) / speed.max(0.01))
}

/// The timeline-time change a trim makes to a clip's two edges.
///
/// This is what a group follows: the addressed clip's change, not its new values
/// (ADR-0019), mirroring `shift_group_offsets` carrying an offset delta.
fn trim_deltas(
    clip: &Clip,
    in_point: Option<Duration>,
    out_point: Option<Duration>,
) -> (Option<f64>, Option<f64>) {
    let speed = clip.speed;
    let old_in = clip.in_point.unwrap_or(Duration::ZERO);
    let new_in = in_point.unwrap_or(Duration::ZERO);
    (
        edge_delta_secs(Some(old_in), Some(new_in), speed),
        edge_delta_secs(clip.out_point, out_point, speed),
    )
}

/// One linked member's edges after a group trim's timeline-time deltas.
///
/// The deltas are converted back through this member's own `speed`, so members that
/// run at different rates keep their timeline edges together (ADR-0019). The member
/// is clamped rather than refused: `apply` cannot see source lengths, so the only
/// clamps available are `in >= 0` and `out >= in`, and a member that runs out of
/// material is the one place a grouped trim loses sync.
fn trimmed_edges(
    clip: &Clip,
    d_in: Option<f64>,
    d_out: Option<f64>,
) -> (Option<Duration>, Option<Duration>) {
    let speed = clip.speed.max(0.01);
    // An edge the trim did not move is left exactly as it was, unset included: a
    // member must not gain an explicit window from an edit that changed nothing,
    // because `Clip::duration` reads an unset edge as "unknown" and a footprint
    // appearing out of nowhere would move later clips on a ripple.
    let shifted = |edge: Duration, delta: Option<f64>| -> Option<Duration> {
        let delta = delta.filter(|d| *d != 0.0)?;
        Duration::try_from_secs_f64((edge.as_secs_f64() + delta * speed).max(0.0)).ok()
    };
    // The in-point is read as zero when unset, matching `split_clip`, so trimming the
    // head of a group reaches a member that carries no explicit in-point.
    let new_in = shifted(clip.in_point.unwrap_or(Duration::ZERO), d_in).or(clip.in_point);
    let new_out = clip
        .out_point
        .and_then(|out| shifted(out, d_out))
        .or(clip.out_point);
    // Keep the window the right way round by pulling the in-point back, never by
    // pushing the out-point out: `apply` cannot see the source, so moving an
    // out-point beyond where the trim put it would claim material that may not
    // exist, while an in-point at the out-point is merely an empty window.
    let new_in = match (new_in, new_out) {
        (Some(i), Some(o)) if i > o => Some(o),
        (i, _) => i,
    };
    (new_in, new_out)
}

/// Trims the clip with `id` and closes or opens the gap on its own track.
///
/// Extracted from [`Command::RippleTrim`] so a grouped ripple trim can run it once
/// per member: each member ripples the track it sits on, which is what keeps a
/// linked pair aligned after the gap closes on both.
fn ripple_trim_one(
    timeline: &mut Timeline,
    id: ClipId,
    in_point: Option<Duration>,
    out_point: Option<Duration>,
) {
    let Some((clips, idx)) = find_clip_track_mut(timeline, id) else {
        return;
    };
    let clip_offset = clips[idx].offset;
    let old_footprint = clip_footprint(&clips[idx]);
    clips[idx].in_point = in_point;
    clips[idx].out_point = out_point;
    let new_footprint = clip_footprint(&clips[idx]);
    // Shift later same-track clips by the change in footprint: shrink pulls them
    // left (closes the gap), grow pushes them right. Only when both footprints are
    // known (as `RippleDelete` requires a known footprint).
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

/// Razors the clip with `id` at timeline position `at`, returning whether it was cut.
///
/// `false` means the cut is not strictly inside the clip's span, which is an error
/// for the addressed clip and a skip for a linked member. The right half takes a
/// fresh [`ClipId`], fresh effect ids, and `right_group` (`None` leaves it carrying
/// whatever the original had, which for an ungrouped clip is nothing).
fn split_one(
    timeline: &mut Timeline,
    id: ClipId,
    at: Duration,
    right_group: Option<GroupId>,
) -> bool {
    // Reserve a fresh clip id and copy the effect counter out before borrowing the
    // tracks (both are written back after the borrow).
    let right_id = ClipId::from_raw(timeline.next_clip_id);
    let mut effect_counter = timeline.next_effect_id;
    let Some((clips, idx)) = find_clip_track_mut(timeline, id) else {
        return false;
    };
    let Some((left, mut right)) = split_clip(&clips[idx], at) else {
        return false;
    };
    right.id = right_id;
    if let Some(g) = right_group {
        right.group = Some(g);
    }
    // The right half is a fresh clip; its cloned effects must get fresh ids (the
    // left half keeps the original clip and its effect ids).
    stamp_effect_ids(&mut right, &mut effect_counter);
    clips[idx] = left;
    clips.insert(idx + 1, right);
    timeline.next_clip_id += 1;
    timeline.next_effect_id = effect_counter;
    true
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
    fn set_clip_property_speed_should_clamp_a_degenerate_value() {
        let t = timeline_with(1);
        let clip = clip_id(&t, 0);
        for bad in [0.0, -2.0] {
            let out = apply(
                &t,
                &Command::SetClipProperty {
                    clip,
                    property: ClipProperty::Speed(bad),
                },
            )
            .unwrap();
            let speed = out.video_tracks()[0].clips[0].speed;
            assert!(
                speed >= crate::MIN_SPEED,
                "a speed of {bad} must be clamped, got {speed}"
            );
        }
        let out = apply(
            &t,
            &Command::SetClipProperty {
                clip,
                property: ClipProperty::Speed(2.0),
            },
        )
        .unwrap();
        assert!(
            (out.video_tracks()[0].clips[0].speed - 2.0).abs() < f64::EPSILON,
            "a sane value is untouched"
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
        patch.scale = 1.5;
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
        assert!((c.scale - 1.5).abs() < f64::EPSILON);
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
        clip.scale = 1.5;
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
            assert!((c.scale - 1.5).abs() < f64::EPSILON);
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
        clip.scale = 1.5;
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
        assert!((moved.scale - 1.5).abs() < f64::EPSILON);
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
        clip.effects.push(ClipEffect::new(EffectKind::Raw {
            step: FilterStep::Lut3d {
                path: "look.cube".into(),
            },
        }));
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
        assert_eq!(moved.effects.len(), 1);
        assert!(matches!(moved.effects[0].kind, EffectKind::Raw { .. }));
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

    // markers

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

    // clip linking / groups (#1456)

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

    /// Replaces `apply_trim_clip_should_not_propagate_to_grouped_member`, which
    /// pinned the behaviour #1813 overturned: a group exists so that its members are
    /// edited together, and a trim that stops at the addressed clip loses A/V sync
    /// on the most frequent edit there is.
    #[test]
    fn trim_clip_should_propagate_to_every_group_member() {
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
        // The member carried no explicit window: an unset in-point reads as zero, so
        // it follows the +1s head trim; an unset out-point is the end of a file whose
        // length `apply` cannot know, so the tail does not move.
        assert_eq!(
            out.video_tracks()[0].clips[1].in_point,
            Some(Duration::from_secs(1))
        );
        assert_eq!(out.video_tracks()[0].clips[1].out_point, None);
    }

    #[test]
    fn trim_clip_should_carry_the_delta_not_the_value_to_a_member() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::from_secs(2), Duration::from_secs(6)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::from_secs(10), Duration::from_secs(14)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let out = apply(
            &t,
            &Command::TrimClip {
                clip: v,
                in_point: Some(Duration::from_secs(3)),
                out_point: Some(Duration::from_secs(5)),
            },
        )
        .unwrap();
        // +1s on the head, -1s on the tail, applied to the member's own window
        // rather than copied from the addressed clip's absolute values.
        assert_eq!(
            out.audio_tracks()[0].clips[0].in_point,
            Some(Duration::from_secs(11))
        );
        assert_eq!(
            out.audio_tracks()[0].clips[0].out_point,
            Some(Duration::from_secs(13))
        );
    }

    #[test]
    fn trim_propagation_should_scale_by_each_member_speed() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4")
                    .trim(Duration::ZERO, Duration::from_secs(8))
                    .with_speed(2.0),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let out = apply(
            &t,
            &Command::TrimClip {
                clip: v,
                in_point: None,
                out_point: Some(Duration::from_secs(4)),
            },
        )
        .unwrap();
        // The video runs at 2x, so cutting 4s of its source is 2s of timeline; the
        // audio runs at 1x and must lose 2s of its own source, not 4s. A plain
        // source-time delta would leave it at 0s and the two would end apart.
        assert_eq!(
            out.audio_tracks()[0].clips[0].out_point,
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn trim_propagation_should_clamp_a_member_instead_of_inverting_it() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(10)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(1)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let out = apply(
            &t,
            &Command::TrimClip {
                clip: v,
                in_point: Some(Duration::from_secs(5)),
                out_point: Some(Duration::from_secs(10)),
            },
        )
        .unwrap();
        let audio = &out.audio_tracks()[0].clips[0];
        // The member holds only 1s, so the +5s head trim stops at its own tail
        // rather than producing a window that runs backwards. `apply` performs no
        // I/O, so this is the only clamp available to it.
        assert_eq!(audio.in_point, Some(Duration::from_secs(1)));
        assert_eq!(audio.out_point, Some(Duration::from_secs(1)));
        // The clamp is not silent: a member left with nothing to play is reported,
        // so a host can tell the user which half of the pair ran out.
        let id = audio.id;
        assert!(
            out.validate()
                .iter()
                .any(|i| matches!(i, crate::TimelineIssue::EmptyFootprint { clip } if *clip == id)),
            "a clamped-empty member should be reported by validate"
        );
    }

    #[test]
    fn trim_should_not_propagate_an_unset_out_point() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let out = apply(
            &t,
            &Command::TrimClip {
                clip: v,
                in_point: Some(Duration::from_secs(1)),
                out_point: None,
            },
        )
        .unwrap();
        let audio = &out.audio_tracks()[0].clips[0];
        // Clearing the out-point means "to end of file", a position `apply` cannot
        // know, so the tail cannot be expressed as a delta and the member keeps its
        // own. The head still follows.
        assert_eq!(audio.in_point, Some(Duration::from_secs(1)));
        assert_eq!(audio.out_point, Some(Duration::from_secs(4)));
    }

    #[test]
    fn ripple_trim_should_propagate_to_every_group_member_and_ripple_each_track() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
                Clip::new("v2.mp4")
                    .trim(Duration::ZERO, Duration::from_secs(4))
                    .offset(Duration::from_secs(4)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
                Clip::new("a2.mp4")
                    .trim(Duration::ZERO, Duration::from_secs(4))
                    .offset(Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let out = apply(
            &t,
            &Command::RippleTrim {
                clip: v,
                // An explicit head: `Clip::duration` reads an unset edge as unknown,
                // and `RippleTrim` shifts nothing when the footprint is unknown.
                in_point: Some(Duration::ZERO),
                out_point: Some(Duration::from_secs(3)),
            },
        )
        .unwrap();
        assert_eq!(
            out.audio_tracks()[0].clips[0].out_point,
            Some(Duration::from_secs(3)),
            "the linked member is trimmed too"
        );
        // Each member closes the gap on its own track, so the two following clips
        // stay level with each other.
        assert_eq!(
            out.video_tracks()[0].clips[1].offset,
            Duration::from_secs(3)
        );
        assert_eq!(
            out.audio_tracks()[0].clips[1].offset,
            Duration::from_secs(3)
        );
    }

    /// Replaces `apply_split_clip_should_keep_both_halves_in_the_group`, which pinned
    /// the behaviour #1813 measured as a tear: with both halves in one group, a later
    /// `MoveClip` on either half dragged the other half on the same track.
    #[test]
    fn split_clip_should_put_the_right_halves_in_one_new_group() {
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
        assert_eq!(
            out.video_tracks()[0].clips[0].group,
            g,
            "the left half keeps the original group"
        );
        let right = out.video_tracks()[0].clips[1].group;
        assert!(right.is_some(), "the right half is still linked");
        assert_ne!(right, g, "but to its own side of the cut, not to the left");
    }

    /// A video and an audio clip, linked, each four seconds from the top.
    fn linked_pair() -> (Timeline, ClipId, ClipId) {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        (t, v, a)
    }

    #[test]
    fn split_clip_should_razor_every_group_member_that_spans_the_cut() {
        let (t, v, _a) = linked_pair();
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: v,
                at: Duration::from_secs(2),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 2);
        assert_eq!(
            out.audio_tracks()[0].clips.len(),
            2,
            "the linked audio is razored at the same timeline position"
        );
        assert_eq!(
            out.audio_tracks()[0].clips[1].offset,
            Duration::from_secs(2)
        );
        assert_eq!(
            out.audio_tracks()[0].clips[1].in_point,
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn split_clip_should_skip_a_group_member_that_does_not_span_the_cut() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(8)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(2)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: v,
                at: Duration::from_secs(5),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips.len(), 2);
        // The audio ends at 2s, so the cut is past it. A group whose members are not
        // aligned is still editable: the member that cannot be cut is left alone
        // rather than failing the razor.
        assert_eq!(out.audio_tracks()[0].clips.len(), 1);
        assert_eq!(
            out.audio_tracks()[0].clips[0].out_point,
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn moving_one_half_after_a_grouped_split_should_not_move_the_other_half() {
        let (t, v, _a) = linked_pair();
        let split = apply(
            &t,
            &Command::SplitClip {
                clip: v,
                at: Duration::from_secs(2),
            },
        )
        .unwrap();
        let right = split.video_tracks()[0].clips[1].id;
        let out = apply(
            &split,
            &Command::MoveClip {
                clip: right,
                offset: Duration::from_secs(9),
            },
        )
        .unwrap();
        // Before #1813 both halves carried one group id, so this move dragged the
        // left half from 0s to 7s and tore the material apart.
        assert_eq!(out.video_tracks()[0].clips[0].offset, Duration::ZERO);
        assert_eq!(
            out.video_tracks()[0].clips[1].offset,
            Duration::from_secs(9)
        );
        // The right halves are linked to each other, so the audio right half follows.
        assert_eq!(
            out.audio_tracks()[0].clips[1].offset,
            Duration::from_secs(9)
        );
        assert_eq!(out.audio_tracks()[0].clips[0].offset, Duration::ZERO);
    }

    #[test]
    fn split_clip_should_leave_an_ungrouped_clip_ungrouped() {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let before = t.next_group_id;
        let out = apply(
            &t,
            &Command::SplitClip {
                clip: v,
                at: Duration::from_secs(2),
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks()[0].clips[0].group, None);
        assert_eq!(out.video_tracks()[0].clips[1].group, None);
        assert_eq!(
            out.next_group_id, before,
            "an ungrouped razor mints no group id"
        );
    }

    // --- a locked track refuses edits (#1805, ADR-0021) ---

    /// A locked video track holding one clip, plus an unlocked audio track holding
    /// one, so a grouped edit can cross the boundary.
    fn locked_and_unlocked() -> (Timeline, ClipId, ClipId, TrackId) {
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track_with(
                Track::new(vec![
                    Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
                ])
                .locked(true),
            )
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let locked_clip = t.video_tracks()[0].clips[0].id;
        let free_clip = t.audio_tracks()[0].clips[0].id;
        let locked_track = t.video_tracks()[0].id;
        (t, locked_clip, free_clip, locked_track)
    }

    #[test]
    fn move_clip_on_a_locked_track_should_be_refused() {
        let (t, clip, _free, track) = locked_and_unlocked();
        let out = apply(
            &t,
            &Command::MoveClip {
                clip,
                offset: Duration::from_secs(99),
            },
        );
        assert_eq!(out.unwrap_err(), EditError::TrackLocked { id: track });
    }

    #[test]
    fn add_clip_to_a_locked_track_should_be_refused() {
        let (t, _clip, _free, track) = locked_and_unlocked();
        let out = apply(
            &t,
            &Command::AddClip {
                track,
                clip: Box::new(Clip::new("new.mp4")),
            },
        );
        assert_eq!(out.unwrap_err(), EditError::TrackLocked { id: track });
    }

    #[test]
    fn remove_track_should_be_refused_when_the_track_is_locked() {
        let (t, _clip, _free, track) = locked_and_unlocked();
        let out = apply(&t, &Command::RemoveTrack { track });
        assert_eq!(out.unwrap_err(), EditError::TrackLocked { id: track });
    }

    #[test]
    fn move_clip_to_a_locked_destination_should_be_refused() {
        let (t, _clip, free, track) = locked_and_unlocked();
        // The clip itself sits on an unlocked track; the destination is the locked
        // one, which is the half a check on the source alone would miss.
        let out = apply(
            &t,
            &Command::MoveClipToTrack {
                clip: free,
                to: track,
                offset: Duration::ZERO,
            },
        );
        assert_eq!(out.unwrap_err(), EditError::TrackLocked { id: track });
    }

    #[test]
    fn an_effect_command_on_a_locked_track_should_be_refused() {
        let (t, clip, _free, track) = locked_and_unlocked();
        let out = apply(
            &t,
            &Command::AddEffect {
                clip,
                kind: blur(4.0),
            },
        );
        assert_eq!(out.unwrap_err(), EditError::TrackLocked { id: track });
    }

    #[test]
    fn a_grouped_edit_reaching_a_locked_member_should_be_refused() {
        let (t, locked_clip, free_clip, track) = locked_and_unlocked();
        let t = apply(
            &t,
            &Command::GroupClips {
                clips: vec![free_clip, locked_clip],
            },
        );
        // Grouping itself names a clip on the locked track, so it is refused too.
        assert_eq!(t.unwrap_err(), EditError::TrackLocked { id: track });
    }

    #[test]
    fn a_grouped_move_should_be_refused_when_a_member_is_locked() {
        // Group first, then lock: the group exists, and the edit is addressed at the
        // member that is still free.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let video_track = t.video_tracks()[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let t = apply(
            &t,
            &Command::SetTrackLock {
                track: video_track,
                lock: true,
            },
        )
        .unwrap();

        let out = apply(
            &t,
            &Command::MoveClip {
                clip: a,
                offset: Duration::from_secs(9),
            },
        );
        assert_eq!(
            out.unwrap_err(),
            EditError::TrackLocked { id: video_track },
            "a grouped move must not half-apply because one member is locked"
        );
    }

    #[test]
    fn ungrouping_from_a_free_member_should_be_refused_when_another_is_locked() {
        // `UngroupClips` clears `group` on every member, so it writes to the locked
        // track even when the clip it names sits on a free one.
        let t = Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new("v.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .audio_track(vec![
                Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4)),
            ])
            .build()
            .unwrap();
        let v = t.video_tracks()[0].clips[0].id;
        let a = t.audio_tracks()[0].clips[0].id;
        let video_track = t.video_tracks()[0].id;
        let t = apply(&t, &Command::GroupClips { clips: vec![v, a] }).unwrap();
        let t = apply(
            &t,
            &Command::SetTrackLock {
                track: video_track,
                lock: true,
            },
        )
        .unwrap();

        let out = apply(&t, &Command::UngroupClips { clip: a });
        assert_eq!(
            out.unwrap_err(),
            EditError::TrackLocked { id: video_track },
            "ungrouping would have cleared the locked member's group"
        );
    }

    #[test]
    fn set_track_lock_should_release_a_locked_track() {
        let (t, clip, _free, track) = locked_and_unlocked();
        let unlocked = apply(&t, &Command::SetTrackLock { track, lock: false }).unwrap();
        assert!(!unlocked.video_tracks()[0].lock);
        let moved = apply(
            &unlocked,
            &Command::MoveClip {
                clip,
                offset: Duration::from_secs(9),
            },
        )
        .unwrap();
        assert_eq!(
            moved.video_tracks()[0].clips[0].offset,
            Duration::from_secs(9),
            "the edit goes through once the lock is released"
        );
    }

    #[test]
    fn a_batch_containing_a_locked_edit_should_change_nothing() {
        let (t, clip, free, track) = locked_and_unlocked();
        let out = apply(
            &t,
            &Command::Batch(vec![
                Command::MoveClip {
                    clip: free,
                    offset: Duration::from_secs(3),
                },
                Command::MoveClip {
                    clip,
                    offset: Duration::from_secs(9),
                },
            ]),
        );
        assert_eq!(out.unwrap_err(), EditError::TrackLocked { id: track });
    }

    #[test]
    fn timeline_level_commands_should_be_unaffected_by_a_lock() {
        let (t, _clip, _free, _track) = locked_and_unlocked();
        let out = apply(&t, &Command::SetFrameRate { fps: 25.0 }).unwrap();
        assert!((out.frame_rate() - 25.0).abs() < f64::EPSILON);
        let out = apply(
            &out,
            &Command::SetCanvas {
                width: 640,
                height: 360,
            },
        )
        .unwrap();
        assert_eq!((out.canvas_width(), out.canvas_height()), (640, 360));
        // And a new track can still be added while another is locked.
        let out = apply(
            &out,
            &Command::AddTrack {
                kind: TrackKind::Video,
            },
        )
        .unwrap();
        assert_eq!(out.video_tracks().len(), 2);
    }

    // --- typed effect commands (#1458) ---

    fn color_correct(brightness: f64) -> EffectKind {
        use crate::effect::Param;
        EffectKind::ColorCorrect {
            brightness: Param::Const(brightness),
            contrast: Param::Const(1.0),
            saturation: Param::Const(1.0),
            temperature: Param::Const(0.0),
            tint: Param::Const(0.0),
        }
    }

    fn blur(radius: f64) -> EffectKind {
        use crate::effect::Param;
        EffectKind::Blur {
            radius: Param::Const(radius),
        }
    }

    /// The effect ids on clip `i` of the first video track, in order.
    fn effect_ids(t: &Timeline, i: usize) -> Vec<EffectId> {
        t.video_tracks()[0].clips[i]
            .effects
            .iter()
            .map(|e| e.id)
            .collect()
    }

    #[test]
    fn apply_add_effect_should_append_with_a_fresh_id() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let out = apply(
            &t,
            &Command::AddEffect {
                clip: id,
                kind: color_correct(0.5),
            },
        )
        .unwrap();
        let effects = &out.video_tracks()[0].clips[0].effects;
        assert_eq!(effects.len(), 1);
        assert!(effects[0].id.is_set(), "the effect gets a document id");
        assert!(effects[0].enabled);
        // A second add mints a distinct id and appends after the first.
        let out2 = apply(
            &out,
            &Command::AddEffect {
                clip: id,
                kind: blur(3.0),
            },
        )
        .unwrap();
        let ids = effect_ids(&out2, 0);
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "each effect id is unique");
    }

    #[test]
    fn apply_add_effect_to_missing_clip_should_err() {
        let t = timeline_with(1);
        let err = apply(
            &t,
            &Command::AddEffect {
                clip: ClipId::UNSET,
                kind: color_correct(0.5),
            },
        )
        .unwrap_err();
        assert!(matches!(err, EditError::ClipNotFound { .. }));
    }

    #[test]
    fn apply_remove_effect_should_drop_the_addressed_effect() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let out = apply(
            &t,
            &Command::AddEffect {
                clip: id,
                kind: color_correct(0.5),
            },
        )
        .unwrap();
        let eff = effect_ids(&out, 0)[0];
        let out = apply(
            &out,
            &Command::RemoveEffect {
                clip: id,
                effect: eff,
            },
        )
        .unwrap();
        assert!(out.video_tracks()[0].clips[0].effects.is_empty());
    }

    #[test]
    fn apply_remove_effect_with_unknown_id_should_err() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let err = apply(
            &t,
            &Command::RemoveEffect {
                clip: id,
                effect: EffectId::UNSET,
            },
        )
        .unwrap_err();
        assert!(matches!(err, EditError::EffectNotFound { .. }));
    }

    #[test]
    fn apply_set_effect_kind_should_replace_params_and_keep_id() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let out = apply(
            &t,
            &Command::AddEffect {
                clip: id,
                kind: color_correct(0.5),
            },
        )
        .unwrap();
        let eff = effect_ids(&out, 0)[0];
        let out = apply(
            &out,
            &Command::SetEffectKind {
                clip: id,
                effect: eff,
                kind: blur(2.0),
            },
        )
        .unwrap();
        let e = &out.video_tracks()[0].clips[0].effects[0];
        assert_eq!(e.id, eff, "the id is preserved across a kind change");
        assert!(matches!(e.kind, EffectKind::Blur { .. }));
    }

    #[test]
    fn apply_set_effect_enabled_should_toggle_and_drop_from_chain() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let out = apply(
            &t,
            &Command::AddEffect {
                clip: id,
                kind: color_correct(0.5),
            },
        )
        .unwrap();
        let eff = effect_ids(&out, 0)[0];
        // Non-neutral color correction contributes an Eq step while enabled.
        assert_eq!(out.video_tracks()[0].clips[0].video_effect_chain().len(), 1);
        let out = apply(
            &out,
            &Command::SetEffectEnabled {
                clip: id,
                effect: eff,
                enabled: false,
            },
        )
        .unwrap();
        let c = &out.video_tracks()[0].clips[0];
        assert!(!c.effects[0].enabled);
        assert!(
            c.video_effect_chain().is_empty(),
            "a disabled effect is skipped in derivation but kept in the list"
        );
    }

    /// A clip with one blur effect, and the document that holds it.
    fn timeline_with_one_effect() -> (Timeline, ClipId, EffectId) {
        let t = timeline_with(1);
        let clip = clip_id(&t, 0);
        let t = apply(
            &t,
            &Command::AddEffect {
                clip,
                kind: blur(4.0),
            },
        )
        .unwrap();
        let effect = t.video_tracks()[0].clips[0].effects[0].id;
        (t, clip, effect)
    }

    #[test]
    fn set_clip_should_keep_an_effect_id_the_clip_already_has() {
        let (t, clip, effect) = timeline_with_one_effect();
        let mut patched = t.video_tracks()[0].clips[0].clone();
        patched.opacity = 0.5;
        let out = apply(
            &t,
            &Command::SetClip {
                clip,
                value: Box::new(patched),
            },
        )
        .unwrap();
        let after = &out.video_tracks()[0].clips[0];
        assert_eq!(after.opacity, 0.5, "the patch was applied");
        assert_eq!(
            after.effects[0].id, effect,
            "an edit that never touched the effect must not renumber it"
        );
    }

    #[test]
    fn set_clip_should_restamp_an_effect_id_the_clip_does_not_have() {
        let (t, clip, effect) = timeline_with_one_effect();
        let mut patched = t.video_tracks()[0].clips[0].clone();
        // Above `next_effect_id`, so keeping it would also collide with a later mint.
        patched.effects[0].id = EffectId::from_raw(9_000);
        let out = apply(
            &t,
            &Command::SetClip {
                clip,
                value: Box::new(patched),
            },
        )
        .unwrap();
        let after = out.video_tracks()[0].clips[0].effects[0].id;
        assert_ne!(after, EffectId::from_raw(9_000), "a foreign id is not kept");
        assert_ne!(after, effect, "nor is it silently mapped back");
        assert!(after.is_set());
    }

    #[test]
    fn set_clip_should_not_hand_out_a_duplicate_effect_id_afterwards() {
        let (t, clip, _effect) = timeline_with_one_effect();
        let mut patched = t.video_tracks()[0].clips[0].clone();
        // Exactly the value the document is about to mint: keeping a foreign id has
        // to be a real collision here, or this test would pass on a rule that keeps
        // whatever the patch carries.
        patched.effects[0].id = EffectId::from_raw(t.next_effect_id);
        let out = apply(
            &t,
            &Command::SetClip {
                clip,
                value: Box::new(patched),
            },
        )
        .unwrap();
        let out = apply(
            &out,
            &Command::AddEffect {
                clip,
                kind: blur(8.0),
            },
        )
        .unwrap();
        let ids: Vec<EffectId> = out.video_tracks()[0].clips[0]
            .effects
            .iter()
            .map(|e| e.id)
            .collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "two effects share an id: {ids:?}");
    }

    #[test]
    fn set_clip_should_not_keep_the_same_effect_id_twice_in_one_patch() {
        let (t, clip, effect) = timeline_with_one_effect();
        // A host duplicating an effect row clones the struct, id included.
        let mut patched = t.video_tracks()[0].clips[0].clone();
        let duplicate = patched.effects[0].clone();
        patched.effects.push(duplicate);
        let out = apply(
            &t,
            &Command::SetClip {
                clip,
                value: Box::new(patched),
            },
        )
        .unwrap();
        let effects = &out.video_tracks()[0].clips[0].effects;
        assert_eq!(effects.len(), 2);
        assert_eq!(effects[0].id, effect, "the first occurrence keeps the id");
        assert_ne!(
            effects[1].id, effect,
            "the copy must not share it: two effects under one id is what ADR-0001 rules out"
        );
    }

    #[test]
    fn apply_set_clip_should_stamp_effect_ids_on_the_patch() {
        // A SetClip patch built with `with_color_correction` carries an effect with
        // an UNSET id; installing it must mint a real, addressable document id.
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let patch = Clip::new("patched.mp4").with_color_correction(0.5, 1.0, 1.0);
        let out = apply(
            &t,
            &Command::SetClip {
                clip: id,
                value: Box::new(patch),
            },
        )
        .unwrap();
        let effects = &out.video_tracks()[0].clips[0].effects;
        assert_eq!(effects.len(), 1);
        assert!(
            effects[0].id.is_set(),
            "the patch's effect gets a fresh document id"
        );
    }

    #[test]
    fn apply_add_clip_should_stamp_effect_ids_on_a_caller_built_clip() {
        // A clip built with `with_color_correction` carries an effect with an UNSET
        // id; adding it must mint a real, document-unique id so it is addressable.
        let t = timeline_with(1);
        let track = track0(&t);
        let clip = Clip::new("built.mp4").with_color_correction(0.5, 1.0, 1.0);
        let out = apply(
            &t,
            &Command::AddClip {
                track,
                clip: Box::new(clip),
            },
        )
        .unwrap();
        let added = out.video_tracks()[0].clips.last().unwrap();
        assert_eq!(added.effects.len(), 1);
        assert!(
            added.effects[0].id.is_set(),
            "the caller-built effect gets a document id"
        );
    }

    #[test]
    fn apply_split_clip_should_remint_right_half_effect_ids() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let with_effect = apply(
            &t,
            &Command::Batch(vec![
                Command::TrimClip {
                    clip: id,
                    in_point: Some(Duration::ZERO),
                    out_point: Some(Duration::from_secs(10)),
                },
                Command::AddEffect {
                    clip: id,
                    kind: color_correct(0.5),
                },
            ]),
        )
        .unwrap();
        let left_effect = effect_ids(&with_effect, 0)[0];
        let out = apply(
            &with_effect,
            &Command::SplitClip {
                clip: id,
                at: Duration::from_secs(4),
            },
        )
        .unwrap();
        let clips = &out.video_tracks()[0].clips;
        assert_eq!(clips.len(), 2);
        let left = clips[0].effects[0].id;
        let right = clips[1].effects[0].id;
        assert_eq!(
            left, left_effect,
            "the left half keeps the original effect id"
        );
        assert_ne!(
            left, right,
            "the right half's cloned effect is re-minted (document-unique)"
        );
    }

    #[test]
    fn apply_reorder_effects_should_apply_a_permutation() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let out = apply(
            &t,
            &Command::Batch(vec![
                Command::AddEffect {
                    clip: id,
                    kind: color_correct(0.5),
                },
                Command::AddEffect {
                    clip: id,
                    kind: blur(3.0),
                },
            ]),
        )
        .unwrap();
        let ids = effect_ids(&out, 0);
        let out = apply(
            &out,
            &Command::ReorderEffects {
                clip: id,
                order: vec![ids[1], ids[0]],
            },
        )
        .unwrap();
        assert_eq!(effect_ids(&out, 0), vec![ids[1], ids[0]]);
    }

    #[test]
    fn apply_reorder_effects_with_non_permutation_should_err_and_not_change() {
        let t = timeline_with(1);
        let id = clip_id(&t, 0);
        let out = apply(
            &t,
            &Command::Batch(vec![
                Command::AddEffect {
                    clip: id,
                    kind: color_correct(0.5),
                },
                Command::AddEffect {
                    clip: id,
                    kind: blur(3.0),
                },
            ]),
        )
        .unwrap();
        let ids = effect_ids(&out, 0);
        // An order that omits one id (not a full permutation) is rejected.
        let err = apply(
            &out,
            &Command::ReorderEffects {
                clip: id,
                order: vec![ids[0]],
            },
        )
        .unwrap_err();
        assert!(matches!(err, EditError::EffectNotFound { .. }));
        // An order naming an unknown id is rejected too.
        let err = apply(
            &out,
            &Command::ReorderEffects {
                clip: id,
                order: vec![ids[0], ids[1], EffectId::UNSET],
            },
        )
        .unwrap_err();
        assert!(matches!(err, EditError::EffectNotFound { .. }));
    }
}
