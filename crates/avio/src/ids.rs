//! Stable identity for the editing document.
//!
//! Every [`Clip`](crate::Clip) and [`Track`](crate::Track) carries an id that is
//! stable across edits, undo/redo, insertion, removal, and reorder. Ids are
//! **document-scoped monotonic `u64`s**: a [`Timeline`](crate::Timeline) holds a
//! pair of counters and stamps a fresh id whenever a clip or track is added, and
//! ids are **never reused**. That makes a command addressing a removed clip fail
//! (the id is in no track) instead of silently aliasing a different clip — the
//! same stale-reference safety a generational index gives, without the machinery.
//! The [`Editor`](crate::Editor) carries the counters as a session high-water so
//! that an `undo` cannot cause a later edit to re-mint a discarded id.
//!
//! Cross-document uniqueness (copy/paste between projects) and interchange would
//! want UUIDs; that is deferred to the interchange work. `serde` derives arrive
//! with the persistence work (#1426).
//!
//! The decision, and the alternatives weighed (a `slotmap` arena, UUIDs, or
//! positional addressing), are recorded in ADR-0001
//! (`docs/adr/0001-clip-and-track-identity.md`).

/// Stable identity of a [`Clip`](crate::Clip) within a [`Timeline`](crate::Timeline).
///
/// Assigned by the document (see the module docs); [`Clip::new`](crate::Clip::new)
/// leaves it [`UNSET`](ClipId::UNSET) until the clip is placed in a timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClipId(u64);

impl ClipId {
    /// The id of a clip that has not yet been placed in a document.
    pub const UNSET: ClipId = ClipId(0);

    /// Whether this id has been assigned by a document (i.e. is not [`UNSET`](Self::UNSET)).
    #[must_use]
    pub fn is_set(self) -> bool {
        self.0 != 0
    }

    /// Mints an id from a raw counter value. Crate-internal: only the document
    /// (`Timeline::build` and `apply`) mints ids, so callers cannot forge one.
    /// The public `id` fields remain writable, but `build`/`apply` re-stamp ids,
    /// so the document's uniqueness invariant holds regardless.
    pub(crate) fn from_raw(value: u64) -> Self {
        ClipId(value)
    }
}

/// Stable identity of a [`Track`](crate::Track) within a [`Timeline`](crate::Timeline).
///
/// Assigned by the document (see the module docs). Track ids are unique across
/// **both** the video and audio track lists (they share one counter), so a
/// `TrackId` identifies a track without also naming its [`TrackKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackId(u64);

impl TrackId {
    /// The id of a track that has not yet been placed in a document.
    pub const UNSET: TrackId = TrackId(0);

    /// Whether this id has been assigned by a document (i.e. is not [`UNSET`](Self::UNSET)).
    #[must_use]
    pub fn is_set(self) -> bool {
        self.0 != 0
    }

    /// Mints an id from a raw counter value. Crate-internal (see `ClipId::from_raw`).
    pub(crate) fn from_raw(value: u64) -> Self {
        TrackId(value)
    }
}

/// Which track list a [`TrackId`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TrackKind {
    /// A video track ([`Timeline::video_tracks`](crate::Timeline::video_tracks)).
    Video,
    /// An audio track ([`Timeline::audio_tracks`](crate::Timeline::audio_tracks)).
    Audio,
}
