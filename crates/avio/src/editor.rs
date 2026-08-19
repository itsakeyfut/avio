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
/// The version at the internal cursor is the current one. Applying a [`Command`]
/// discards any redo tail and pushes a new version; [`undo`](Self::undo) /
/// [`redo`](Self::redo) move the cursor without dropping versions.
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
}

impl Editor {
    /// Starts an editing session at `initial`.
    #[must_use]
    pub fn new(initial: Timeline) -> Self {
        let next_clip_id = initial.next_clip_id;
        let next_track_id = initial.next_track_id;
        Self {
            history: vec![initial],
            cursor: 0,
            next_clip_id,
            next_track_id,
        }
    }

    /// The current [`Timeline`] version.
    #[must_use]
    pub fn current(&self) -> &Timeline {
        &self.history[self.cursor]
    }

    /// Applies `command` to the current version and makes the result current.
    ///
    /// Any redo history (versions after the cursor) is discarded first. On an
    /// invalid edit the history is left completely unchanged.
    ///
    /// # Errors
    ///
    /// Returns the [`EditError`] from [`apply`](crate::apply) when the command
    /// cannot be applied; the history is not modified in that case.
    pub fn apply(&mut self, command: &Command) -> Result<&Timeline, EditError> {
        // Seat the session high-water counters onto the current version before
        // applying, so an edit after an `undo` (which restored an older snapshot,
        // and with it that snapshot's smaller counters) never re-mints an id a
        // truncated branch already handed out.
        let mut current = self.history[self.cursor].clone();
        current.next_clip_id = self.next_clip_id;
        current.next_track_id = self.next_track_id;

        // Compute the new version first: if this fails, `?` returns before any
        // history mutation, so a rejected edit never disturbs undo/redo state.
        let next = crate::edit::apply(&current, command)?;
        self.next_clip_id = next.next_clip_id;
        self.next_track_id = next.next_track_id;
        self.history.truncate(self.cursor + 1);
        self.history.push(next);
        self.cursor += 1;
        Ok(&self.history[self.cursor])
    }

    /// Moves back one version and returns it, or `None` at the start of history.
    pub fn undo(&mut self) -> Option<&Timeline> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        Some(&self.history[self.cursor])
    }

    /// Moves forward one version and returns it, or `None` at the end of history.
    pub fn redo(&mut self) -> Option<&Timeline> {
        if self.cursor + 1 >= self.history.len() {
            return None;
        }
        self.cursor += 1;
        Some(&self.history[self.cursor])
    }

    /// Whether [`undo`](Self::undo) would move to an earlier version.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    /// Whether [`redo`](Self::redo) would move to a later version.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.history.len()
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
}
