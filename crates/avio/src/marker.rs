//! Editorial timeline markers.
//!
//! A [`Marker`] is a labelled point on the timeline (for navigation, notes, or
//! chapter authoring). Markers are metadata only: they never affect derivation,
//! render, or preview. They are added, moved, and removed through the undoable
//! [`Command`](crate::Command) path and addressed by a stable
//! [`MarkerId`](crate::MarkerId).

use std::time::Duration;

use ff_format::Color;

use crate::ids::MarkerId;

/// A labelled point on the timeline.
///
/// All fields are public so callers can inspect them; a marker's [`id`](Self::id)
/// is assigned by the document when it is added (any id on an incoming marker is
/// replaced with a fresh one).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Marker {
    /// Stable identity within a [`Timeline`](crate::Timeline).
    ///
    /// [`MarkerId::UNSET`] until the marker is added to a document.
    pub id: MarkerId,
    /// Position on the timeline (presentation timestamp from the start of the
    /// composition).
    pub pts: Duration,
    /// Optional human-readable label. `None` = an unnamed marker.
    pub name: Option<String>,
    /// Optional marker colour (for a host's UI). `None` = the host default.
    pub color: Option<Color>,
    /// Optional free-form note. `None` = no comment.
    pub comment: Option<String>,
}

impl Marker {
    /// Creates a new, unnamed marker at `pts` with no colour or comment.
    ///
    /// Its [`id`](Self::id) is [`MarkerId::UNSET`] until it is added to a document
    /// via [`Command::AddMarker`](crate::Command::AddMarker).
    #[must_use]
    pub fn new(pts: Duration) -> Self {
        Self {
            id: MarkerId::UNSET,
            pts,
            name: None,
            color: None,
            comment: None,
        }
    }

    /// Sets the marker's label and returns the updated marker.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets the marker's colour and returns the updated marker.
    #[must_use]
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Sets the marker's comment and returns the updated marker.
    #[must_use]
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }
}
