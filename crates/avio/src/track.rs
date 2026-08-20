//! A first-class track in the editing document.
//!
//! A [`Track`] is an ordered list of [`Clip`]s plus its editorial state: a stable
//! [`TrackId`], a name, and the `mute` / `solo` / `enabled` / `lock` flags. It
//! replaces the bare `Vec<Clip>` lane the [`Timeline`](crate::Timeline) used to
//! store, so a host can name a track, mute/solo it, and address it by id.
//!
//! `mute` / `solo` / `enabled` decide whether a track contributes to the derived
//! output (see [`Track::is_active`]); `lock` and `name` are authoring metadata the
//! derivation ignores.

use ff_filter::FilterStep;

use crate::clip::Clip;
use crate::ids::TrackId;

/// An ordered list of [`Clip`]s and its editorial state.
///
/// Construct one with [`Track::new`] (or via
/// [`TimelineBuilder::video_track`](crate::TimelineBuilder::video_track)); the
/// [`Timeline`](crate::Timeline) assigns the [`id`](Track::id) when the track is
/// placed in the document.
// The four flags (mute/solo/enabled/lock) are independent, orthogonal editorial
// states every NLE track carries — not a state machine — so a bool each is the
// clearest model.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Track {
    /// Stable identity, assigned by the document (`TrackId::UNSET` until placed).
    pub id: TrackId,
    /// Human-readable track name (may be empty).
    pub name: String,
    /// When `true`, the track contributes nothing to the output (see `Track::is_active`).
    pub mute: bool,
    /// When any track in the same list is soloed, only soloed tracks contribute.
    pub solo: bool,
    /// When `false`, the track is disabled and contributes nothing.
    pub enabled: bool,
    /// Authoring flag protecting the track from edits; ignored by the derivation.
    pub lock: bool,
    /// The clips on this track, in order (index 0 first on the timeline).
    pub clips: Vec<Clip>,
    /// Ordered per-track (pre-mix) audio effect chain applied to this track's
    /// mixed contribution before it enters the timeline mix, on render.
    ///
    /// Its natural use is a two-pass effect such as loudness normalization
    /// ([`FilterStep::LoudnessNormalize`]) or a compressor
    /// ([`FilterStep::ACompressor`]) that should act on the whole track rather
    /// than one clip. An empty chain (the default) is a no-op and leaves the
    /// audio path unchanged. Ignored for video tracks (they carry no audio).
    ///
    /// Not persisted by the `serde` feature yet: `FilterStep` is not
    /// serializable, so this field is skipped and deserializes to an empty vec
    /// (mirroring [`Clip::audio_effects`](crate::Clip::audio_effects)).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub audio_effects: Vec<FilterStep>,
}

impl Track {
    /// Creates an enabled, unnamed track holding `clips`, with its id unset.
    ///
    /// The [`Timeline`](crate::Timeline) stamps a real [`TrackId`] when the track
    /// is placed in the document.
    #[must_use]
    pub fn new(clips: Vec<Clip>) -> Self {
        Self {
            id: TrackId::UNSET,
            name: String::new(),
            mute: false,
            solo: false,
            enabled: true,
            lock: false,
            clips,
            audio_effects: Vec::new(),
        }
    }

    /// Sets the track name.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the `mute` flag.
    #[must_use]
    pub fn muted(mut self, mute: bool) -> Self {
        self.mute = mute;
        self
    }

    /// Sets the `solo` flag.
    #[must_use]
    pub fn soloed(mut self, solo: bool) -> Self {
        self.solo = solo;
        self
    }

    /// Sets the `enabled` flag.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the `lock` flag.
    #[must_use]
    pub fn locked(mut self, lock: bool) -> Self {
        self.lock = lock;
        self
    }

    /// Sets the per-track (pre-mix) audio effect chain (see
    /// [`audio_effects`](Self::audio_effects)).
    #[must_use]
    pub fn audio_effects(mut self, steps: Vec<FilterStep>) -> Self {
        self.audio_effects = steps;
        self
    }

    /// Whether this track contributes to the derived output.
    ///
    /// A track is active when it is `enabled`, not `mute`d, and — if any track in
    /// its list is soloed (`any_solo_in_list`) — is itself `solo`. `any_solo_in_list`
    /// must be computed over the track's own media list (video or audio), since
    /// solo is scoped per list.
    pub(crate) fn is_active(&self, any_solo_in_list: bool) -> bool {
        self.enabled && !self.mute && (!any_solo_in_list || self.solo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_active_should_reflect_enabled_mute_solo() {
        // Default (enabled, not muted, no solo in the list) -> active.
        assert!(Track::new(vec![]).is_active(false));
        // Disabled -> inactive.
        assert!(!Track::new(vec![]).enabled(false).is_active(false));
        // Muted -> inactive.
        assert!(!Track::new(vec![]).muted(true).is_active(false));
        // Another track in the list is soloed, this one is not -> shadowed.
        assert!(!Track::new(vec![]).is_active(true));
        // Soloed while a solo is present -> active.
        assert!(Track::new(vec![]).soloed(true).is_active(true));
        // Soloed but disabled -> still inactive (enabled/mute win over solo).
        assert!(
            !Track::new(vec![])
                .soloed(true)
                .enabled(false)
                .is_active(true)
        );
    }

    #[test]
    fn new_track_should_have_empty_audio_effects() {
        assert!(Track::new(vec![]).audio_effects.is_empty());
    }

    #[test]
    fn audio_effects_builder_should_set_chain() {
        let track = Track::new(vec![]).audio_effects(vec![FilterStep::Volume(-6.0)]);
        assert_eq!(track.audio_effects.len(), 1);
        assert!(matches!(
            track.audio_effects[0],
            FilterStep::Volume(v) if (v - (-6.0)).abs() < 1e-9
        ));
    }
}
