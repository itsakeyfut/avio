//! Describes where a file-backed source is used within a [`Timeline`].
//!
//! [`Timeline::find_source_uses`] walks every clip and returns one [`SourceUse`]
//! for each reference to the given path, while [`Timeline::distinct_sources`]
//! returns the set of all file paths the timeline currently holds.

use crate::ids::{ClipId, TrackId, TrackKind};

/// A single clip's reference to a file-backed source, as returned by
/// [`Timeline::find_source_uses`](crate::Timeline::find_source_uses).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SourceUse {
    /// The clip that references the source.
    pub clip: ClipId,
    /// The track the clip belongs to.
    pub track: TrackId,
    /// Whether the track is a video or audio track.
    pub track_kind: TrackKind,
    /// Which role the source plays for this clip.
    pub role: SourceRole,
}

/// The role a file path plays for a given clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SourceRole {
    /// `clip.source == ClipSource::File(path)` — the clip's primary media.
    ActiveSource,
    /// `clip.proxy == Some(path)` — the clip's low-resolution proxy decode source.
    Proxy,
}
