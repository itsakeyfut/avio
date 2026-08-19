//! Undo/redo history over the immutable [`Timeline`] document.
//!
//! An [`Editor`] holds a linear history of `Timeline` versions and a cursor into
//! it. [`Editor::apply`] runs a [`Command`] (via the pure
//! [`apply`](crate::apply)) and pushes the resulting version;
//! [`Editor::undo`] / [`Editor::redo`] move the cursor. This is the stateful
//! Do/Undo/Redo layer on top of the value-based edit API; the model itself stays
//! immutable.
//!
//! History is stored as full snapshots — one `Timeline` clone per edit. This is
//! simple and correct but grows with the number of edits; structural-sharing /
//! diff-based history is a later optimisation (#1352).
//!
//! The id counters live here, outside the snapshotted `Timeline`, as a session
//! high-water mark: an `undo` restores an older snapshot (and its older
//! counters), so seating the high-water before each edit keeps a later
//! `AddClip` / `AddTrack` from re-minting an id a discarded branch already used.
//! This is what makes the "never reused" guarantee of ADR-0001
//! (`docs/adr/0001-clip-and-track-identity.md`) hold across undo.

use crate::edit::{Command, EditError};
use crate::timeline::Timeline;

/// A stateful editing session: an undo/redo history of [`Timeline`] versions.
///
/// The version at the internal cursor is the current one. [`apply`](Self::apply)
/// discards any redo tail and pushes a new version; [`undo`](Self::undo) /
/// [`redo`](Self::redo) move the cursor without dropping versions.
///
/// For a real host: [`amend`](Self::amend) / [`replace_current`](Self::replace_current)
/// update the current version in place **without** adding an undo step, and
/// [`begin_group`](Self::begin_group) … [`commit_group`](Self::commit_group)
/// coalesce a gesture's many edits into a single undo step.
#[derive(Debug)]
pub struct Editor {
    /// Version history; `history[cursor]` is current. Always non-empty.
    history: Vec<Timeline>,
    /// Index of the current version within `history`.
    cursor: usize,
    /// Session high-water for the next clip/track id, never rewound by `undo`.
    /// See the module docs.
    next_clip_id: u64,
    next_track_id: u64,
    /// An in-progress coalesced gesture, or `None`. See [`Editor::begin_group`].
    group: Option<Group>,
}

/// An in-progress coalesced gesture: edits fold into `working` until commit.
#[derive(Debug)]
struct Group {
    /// The evolving version, seeded from the current version at `begin_group`.
    working: Timeline,
    /// Whether any edit changed `working` (so `commit_group` knows to push a step).
    dirty: bool,
}

impl Editor {
    /// Starts an editing session at `initial`.
    ///
    /// This also seats an externally built or mutated [`Timeline`] as the current
    /// version: the session's id high-water starts from `initial`'s counters.
    #[must_use]
    pub fn new(initial: Timeline) -> Self {
        let next_clip_id = initial.next_clip_id;
        let next_track_id = initial.next_track_id;
        Self {
            history: vec![initial],
            cursor: 0,
            next_clip_id,
            next_track_id,
            group: None,
        }
    }

    /// The current [`Timeline`] version. While a group is active this is the
    /// gesture's in-progress version.
    #[must_use]
    pub fn current(&self) -> &Timeline {
        match &self.group {
            Some(g) => &g.working,
            None => &self.history[self.cursor],
        }
    }

    /// Applies `command` to the current version, seating the session id high-water
    /// first (so an edit after `undo` never re-mints a discarded id) and raising
    /// it from the result. Returns the new version. Does not touch history.
    fn edit_current(&mut self, command: &Command) -> Result<Timeline, EditError> {
        let mut seed = self.current().clone();
        seed.next_clip_id = self.next_clip_id;
        seed.next_track_id = self.next_track_id;
        let next = crate::edit::apply(&seed, command)?;
        self.next_clip_id = next.next_clip_id;
        self.next_track_id = next.next_track_id;
        Ok(next)
    }

    /// Applies `command` and makes the result current.
    ///
    /// Outside a group this pushes a new undo step (discarding any redo tail);
    /// inside a group it folds into the gesture's in-progress version instead. On
    /// an invalid edit nothing changes.
    ///
    /// # Errors
    ///
    /// Returns the [`EditError`] from [`apply`](crate::apply) when the command
    /// cannot be applied; no state is modified in that case.
    pub fn apply(&mut self, command: &Command) -> Result<&Timeline, EditError> {
        // Compute the new version first: if this fails, `?` returns before any
        // state mutation, so a rejected edit never disturbs the history or group.
        let next = self.edit_current(command)?;
        if let Some(g) = &mut self.group {
            g.working = next;
            g.dirty = true;
        } else {
            self.history.truncate(self.cursor + 1);
            self.history.push(next);
            self.cursor += 1;
        }
        Ok(self.current())
    }

    /// Applies `command` to the current version **in place**, without adding an
    /// undo step (for live drags / in-place refinement of the current version).
    ///
    /// # Errors
    ///
    /// Returns the [`EditError`] from [`apply`](crate::apply); nothing changes then.
    pub fn amend(&mut self, command: &Command) -> Result<&Timeline, EditError> {
        let next = self.edit_current(command)?;
        self.replace_current(next);
        Ok(self.current())
    }

    /// Seats `timeline` as the current version **in place**, without adding an undo
    /// step. Outside a group this replaces the current version and drops the redo
    /// tail; inside a group it replaces the gesture's in-progress version. The id
    /// high-water is raised to cover `timeline` (never rewound).
    pub fn replace_current(&mut self, timeline: Timeline) {
        self.next_clip_id = self.next_clip_id.max(timeline.next_clip_id);
        self.next_track_id = self.next_track_id.max(timeline.next_track_id);
        if let Some(g) = &mut self.group {
            g.working = timeline;
            g.dirty = true;
        } else {
            self.history.truncate(self.cursor + 1);
            self.history[self.cursor] = timeline;
        }
    }

    /// Begins coalescing subsequent edits into a single undo step.
    ///
    /// Until [`commit_group`](Self::commit_group) or [`cancel_group`](Self::cancel_group),
    /// [`apply`](Self::apply) folds into the gesture's in-progress version instead
    /// of pushing steps, and [`undo`](Self::undo) / [`redo`](Self::redo) are
    /// disabled. Groups do not nest: calling this while a group is active is a
    /// no-op.
    pub fn begin_group(&mut self) {
        if self.group.is_none() {
            self.group = Some(Group {
                // Equivalent to `history[cursor]` under this guard, but reads as
                // "seed from the current version" and cannot be confused with the
                // in-group case.
                working: self.current().clone(),
                dirty: false,
            });
        }
    }

    /// Ends the current group, pushing its accumulated edits as **one** undo step.
    /// A group with no edits produces no step. A no-op if no group is active.
    pub fn commit_group(&mut self) {
        if let Some(g) = self.group.take()
            && g.dirty
        {
            self.history.truncate(self.cursor + 1);
            self.history.push(g.working);
            self.cursor += 1;
        }
    }

    /// Discards the current group and its in-progress edits, leaving the history
    /// unchanged. A no-op if no group is active.
    pub fn cancel_group(&mut self) {
        self.group = None;
    }

    /// Moves back one version and returns it, or `None` at the start of history or
    /// while a group is active (commit or cancel the group first).
    pub fn undo(&mut self) -> Option<&Timeline> {
        if self.group.is_some() || self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        Some(&self.history[self.cursor])
    }

    /// Moves forward one version and returns it, or `None` at the end of history or
    /// while a group is active.
    pub fn redo(&mut self) -> Option<&Timeline> {
        if self.group.is_some() || self.cursor + 1 >= self.history.len() {
            return None;
        }
        self.cursor += 1;
        Some(&self.history[self.cursor])
    }

    /// Whether [`undo`](Self::undo) would move to an earlier version (never while a
    /// group is active).
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.group.is_none() && self.cursor > 0
    }

    /// Whether [`redo`](Self::redo) would move to a later version (never while a
    /// group is active).
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.group.is_none() && self.cursor + 1 < self.history.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Clip;

    fn timeline(fps: f64) -> Timeline {
        Timeline::builder()
            .canvas(1920, 1080)
            .frame_rate(fps)
            .video_track(vec![Clip::new("a.mp4")])
            .build()
            .unwrap()
    }

    fn set_fps(fps: f64) -> Command {
        Command::SetFrameRate { fps }
    }

    #[test]
    fn editor_new_should_start_with_no_undo_or_redo() {
        let ed = Editor::new(timeline(30.0));
        assert!(!ed.can_undo());
        assert!(!ed.can_redo());
        assert!((ed.current().frame_rate() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn editor_apply_should_advance_and_enable_undo() {
        let mut ed = Editor::new(timeline(30.0));
        let cur = ed.apply(&set_fps(24.0)).unwrap();
        assert!((cur.frame_rate() - 24.0).abs() < f64::EPSILON);
        assert!(ed.can_undo());
        assert!(!ed.can_redo());
    }

    #[test]
    fn editor_undo_should_restore_previous_version() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        let prev = ed.undo().unwrap();
        assert!((prev.frame_rate() - 30.0).abs() < f64::EPSILON);
        assert!(!ed.can_undo());
        assert!(ed.can_redo());
    }

    #[test]
    fn editor_redo_should_reapply_the_undone_version() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        ed.undo().unwrap();
        let next = ed.redo().unwrap();
        assert!((next.frame_rate() - 24.0).abs() < f64::EPSILON);
        assert!(!ed.can_redo());
    }

    #[test]
    fn editor_undo_at_start_should_return_none() {
        let mut ed = Editor::new(timeline(30.0));
        assert!(ed.undo().is_none());
        assert!((ed.current().frame_rate() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn editor_redo_at_end_should_return_none() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        assert!(ed.redo().is_none());
    }

    #[test]
    fn editor_new_edit_after_undo_should_truncate_redo() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        ed.apply(&set_fps(48.0)).unwrap();
        ed.undo().unwrap(); // back to the 24.0 version; 48.0 is now the redo tail
        let cur = ed.apply(&set_fps(60.0)).unwrap(); // must discard the 48.0 redo
        assert!((cur.frame_rate() - 60.0).abs() < f64::EPSILON);
        assert!(!ed.can_redo());
        assert!(ed.redo().is_none());
    }

    #[test]
    fn editor_apply_error_should_leave_history_unchanged() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        let err = ed.apply(&set_fps(0.0)).unwrap_err(); // invalid: fps <= 0
        assert_eq!(err, EditError::InvalidFrameRate(0.0));
        // History untouched: still one edit deep, no redo, current is the 24.0 version.
        assert!(ed.can_undo());
        assert!(!ed.can_redo());
        assert!((ed.current().frame_rate() - 24.0).abs() < f64::EPSILON);
    }

    #[test]
    fn editor_should_preserve_clip_id_across_undo_redo() {
        let mut ed = Editor::new(timeline(30.0));
        let track = ed.current().video_tracks()[0].id;
        ed.apply(&Command::AddClip {
            track,
            clip: Box::new(Clip::new("x.mp4")),
        })
        .unwrap();
        let id = ed.current().video_tracks()[0].clips[1].id;
        ed.undo().unwrap();
        let after = ed.redo().unwrap();
        assert_eq!(
            after.video_tracks()[0].clips[1].id,
            id,
            "undo/redo must not renumber the clip"
        );
    }

    #[test]
    fn editor_should_not_reuse_ids_across_undo() {
        let mut ed = Editor::new(timeline(30.0));
        let track = ed.current().video_tracks()[0].id;
        ed.apply(&Command::AddClip {
            track,
            clip: Box::new(Clip::new("a.mp4")),
        })
        .unwrap();
        let first = ed.current().video_tracks()[0].clips[1].id;
        // Discard that edit; without the session high-water the counter would
        // rewind and the next AddClip would re-mint `first` for a different clip.
        ed.undo().unwrap();
        let after = ed
            .apply(&Command::AddClip {
                track,
                clip: Box::new(Clip::new("b.mp4")),
            })
            .unwrap();
        let second = after.video_tracks()[0].clips[1].id;
        assert_ne!(
            first, second,
            "an id used by a discarded branch must not be reused"
        );
    }

    #[test]
    fn editor_apply_batch_should_be_one_undo_step() {
        let mut ed = Editor::new(timeline(30.0));
        let track = ed.current().video_tracks()[0].id;
        ed.apply(&Command::Batch(vec![
            Command::AddClip {
                track,
                clip: Box::new(Clip::new("a.mp4")),
            },
            Command::AddClip {
                track,
                clip: Box::new(Clip::new("b.mp4")),
            },
        ]))
        .unwrap();
        assert_eq!(ed.current().video_tracks()[0].clips.len(), 3);
        // One undo removes the whole batch.
        let prev = ed.undo().unwrap();
        assert_eq!(prev.video_tracks()[0].clips.len(), 1);
    }

    #[test]
    fn editor_group_should_coalesce_to_one_undo_step() {
        let mut ed = Editor::new(timeline(30.0));
        ed.begin_group();
        ed.apply(&set_fps(24.0)).unwrap();
        ed.apply(&set_fps(48.0)).unwrap();
        assert!(
            (ed.current().frame_rate() - 48.0).abs() < f64::EPSILON,
            "the working version reflects grouped edits live"
        );
        ed.commit_group();
        assert!((ed.current().frame_rate() - 48.0).abs() < f64::EPSILON);
        // One undo step: undo restores the pre-gesture version (30), not 24.
        let prev = ed.undo().unwrap();
        assert!((prev.frame_rate() - 30.0).abs() < f64::EPSILON);
        assert!(!ed.can_undo());
    }

    #[test]
    fn editor_empty_group_should_add_no_step() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        ed.begin_group();
        ed.commit_group(); // no edits in the group
        assert!(!ed.can_redo());
        // Only the single real edit remains: one undo returns to 30.
        let prev = ed.undo().unwrap();
        assert!((prev.frame_rate() - 30.0).abs() < f64::EPSILON);
        assert!(!ed.can_undo());
    }

    #[test]
    fn editor_cancel_group_should_discard_edits() {
        let mut ed = Editor::new(timeline(30.0));
        ed.begin_group();
        ed.apply(&set_fps(24.0)).unwrap();
        ed.cancel_group();
        assert!(
            (ed.current().frame_rate() - 30.0).abs() < f64::EPSILON,
            "cancel reverts to the pre-group version"
        );
        assert!(!ed.can_undo(), "cancel added no step");
    }

    #[test]
    fn editor_undo_redo_should_be_disabled_during_a_group() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        ed.begin_group();
        ed.apply(&set_fps(48.0)).unwrap();
        assert!(!ed.can_undo());
        assert!(ed.undo().is_none());
        assert!(ed.redo().is_none());
        ed.commit_group();
        // After commit, undo works again and restores the pre-gesture version.
        assert!(ed.can_undo());
        let prev = ed.undo().unwrap();
        assert!((prev.frame_rate() - 24.0).abs() < f64::EPSILON);
    }

    #[test]
    fn editor_amend_should_update_current_without_growing_history() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap(); // step 1
        ed.amend(&set_fps(48.0)).unwrap(); // in place: no new step
        assert!((ed.current().frame_rate() - 48.0).abs() < f64::EPSILON);
        assert!(ed.can_undo());
        assert!(!ed.can_redo());
        // One undo goes past the amended value to the pre-step-1 version (30).
        let prev = ed.undo().unwrap();
        assert!((prev.frame_rate() - 30.0).abs() < f64::EPSILON);
        assert!(!ed.can_undo());
    }

    #[test]
    fn editor_replace_current_should_seat_value_and_drop_redo() {
        let mut ed = Editor::new(timeline(30.0));
        ed.apply(&set_fps(24.0)).unwrap();
        ed.apply(&set_fps(48.0)).unwrap();
        ed.undo().unwrap(); // back to 24, redo tail = [48]
        assert!(ed.can_redo());
        ed.replace_current(timeline(60.0));
        assert!((ed.current().frame_rate() - 60.0).abs() < f64::EPSILON);
        assert!(!ed.can_redo(), "seating a value drops the redo tail");
    }

    #[test]
    fn editor_group_should_keep_ids_monotonic() {
        let mut ed = Editor::new(timeline(30.0));
        let track = ed.current().video_tracks()[0].id;
        ed.begin_group();
        ed.apply(&Command::AddClip {
            track,
            clip: Box::new(Clip::new("a.mp4")),
        })
        .unwrap();
        ed.apply(&Command::AddClip {
            track,
            clip: Box::new(Clip::new("b.mp4")),
        })
        .unwrap();
        ed.commit_group();
        let clips = &ed.current().video_tracks()[0].clips;
        assert_eq!(clips.len(), 3);
        assert_ne!(clips[1].id, clips[2].id);
    }

    #[test]
    fn editor_amend_during_group_should_fold_into_the_gesture() {
        let mut ed = Editor::new(timeline(30.0));
        ed.begin_group();
        ed.apply(&set_fps(24.0)).unwrap();
        ed.amend(&set_fps(48.0)).unwrap(); // folds into the working version, not a step
        assert!((ed.current().frame_rate() - 48.0).abs() < f64::EPSILON);
        ed.commit_group();
        // Still one undo step: undo restores the pre-gesture version.
        let prev = ed.undo().unwrap();
        assert!((prev.frame_rate() - 30.0).abs() < f64::EPSILON);
        assert!(!ed.can_undo());
    }

    #[test]
    fn editor_nested_begin_group_should_not_restart_the_gesture() {
        let mut ed = Editor::new(timeline(30.0));
        ed.begin_group();
        ed.apply(&set_fps(24.0)).unwrap(); // working now at 24
        ed.begin_group(); // no-op: must not reseed working from history (still 30)
        assert!(
            (ed.current().frame_rate() - 24.0).abs() < f64::EPSILON,
            "a second begin_group must not discard in-progress edits"
        );
        ed.commit_group();
        let prev = ed.undo().unwrap();
        assert!((prev.frame_rate() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn editor_set_clip_should_be_undoable() {
        let mut ed = Editor::new(timeline(30.0));
        let id = ed.current().video_tracks()[0].clips[0].id;
        let mut patch = Clip::new("patched.mp4");
        patch.brightness = 0.5;
        ed.apply(&Command::SetClip {
            clip: id,
            value: Box::new(patch),
        })
        .unwrap();
        assert_eq!(
            ed.current().video_tracks()[0].clips[0].source.to_str(),
            Some("patched.mp4")
        );
        let prev = ed.undo().unwrap();
        assert_eq!(
            prev.video_tracks()[0].clips[0].source.to_str(),
            Some("a.mp4"),
            "undo restores the pre-patch clip"
        );
    }
}
