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

use ff_filter::{AnimationTrack, FilterStep};

use crate::clip::Clip;
use crate::ids::TrackId;

/// A video-layer property that a [`TrackAutomation`] can animate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProperty {
    /// Horizontal position.
    X,
    /// Vertical position.
    Y,
    /// Horizontal scale.
    ScaleX,
    /// Vertical scale.
    ScaleY,
    /// Rotation.
    Rotation,
    /// Opacity.
    Opacity,
}

/// An audio-track property that a [`TrackAutomation`] can animate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioProperty {
    /// Volume in dB.
    Volume,
    /// Stereo pan.
    Pan,
}

/// Typed, per-property track-level automation.
///
/// Held by [`Track::automation`], it re-keys the old `"{video|audio}_{index}_{prop}"`
/// string map by track *identity*: because the automation lives on the track,
/// reordering or removing a track carries or drops its automation with it.
/// This mirrors the per-property clip-level tracks on [`Clip`](crate::Clip)
/// (`opacity_track`, `x_track`, …) so clip and track automation share one model.
///
/// Video tracks use the video properties (`opacity`/`x`/`y`/`scale_x`/`scale_y`/
/// `rotation`); audio tracks use `volume`/`pan`. Unused properties stay `None`.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackAutomation {
    /// Opacity animation (video).
    pub opacity: Option<AnimationTrack<f64>>,
    /// Horizontal-position animation (video).
    pub x: Option<AnimationTrack<f64>>,
    /// Vertical-position animation (video).
    pub y: Option<AnimationTrack<f64>>,
    /// Horizontal-scale animation (video).
    pub scale_x: Option<AnimationTrack<f64>>,
    /// Vertical-scale animation (video).
    pub scale_y: Option<AnimationTrack<f64>>,
    /// Rotation animation (video).
    pub rotation: Option<AnimationTrack<f64>>,
    /// Volume animation (audio, dB).
    pub volume: Option<AnimationTrack<f64>>,
    /// Pan animation (audio).
    pub pan: Option<AnimationTrack<f64>>,
}

impl TrackAutomation {
    /// Sets the animation for a video property.
    pub fn set_video(&mut self, property: VideoProperty, animation: AnimationTrack<f64>) {
        let slot = match property {
            VideoProperty::X => &mut self.x,
            VideoProperty::Y => &mut self.y,
            VideoProperty::ScaleX => &mut self.scale_x,
            VideoProperty::ScaleY => &mut self.scale_y,
            VideoProperty::Rotation => &mut self.rotation,
            VideoProperty::Opacity => &mut self.opacity,
        };
        *slot = Some(animation);
    }

    /// Sets the animation for an audio property.
    pub fn set_audio(&mut self, property: AudioProperty, animation: AnimationTrack<f64>) {
        let slot = match property {
            AudioProperty::Volume => &mut self.volume,
            AudioProperty::Pan => &mut self.pan,
        };
        *slot = Some(animation);
    }
}

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
    /// Persisted by the `serde` feature (#1452), like
    /// [`Clip::audio_effects`](crate::Clip::audio_effects) and
    /// [`Timeline::audio_filter`](crate::Timeline). The compositor-internal
    /// `FilterStep` variants (`Blend` / `Composite` / `AlphaMatte`) are not
    /// serialized, but an audio chain never contains them.
    pub audio_effects: Vec<FilterStep>,
    /// Typed, id-addressed track-level automation (see [`TrackAutomation`]).
    /// Empty by default; set via the `with_*_animation` builders or
    /// [`TimelineBuilder`](crate::TimelineBuilder)'s `video_animation` /
    /// `audio_animation`. Persisted by the `serde` feature.
    pub automation: TrackAutomation,
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
            automation: TrackAutomation::default(),
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

    /// Sets the track-level opacity animation (video).
    #[must_use]
    pub fn with_opacity_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.opacity = Some(animation);
        self
    }

    /// Sets the track-level horizontal-position animation (video).
    #[must_use]
    pub fn with_x_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.x = Some(animation);
        self
    }

    /// Sets the track-level vertical-position animation (video).
    #[must_use]
    pub fn with_y_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.y = Some(animation);
        self
    }

    /// Sets the track-level horizontal-scale animation (video).
    #[must_use]
    pub fn with_scale_x_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.scale_x = Some(animation);
        self
    }

    /// Sets the track-level vertical-scale animation (video).
    #[must_use]
    pub fn with_scale_y_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.scale_y = Some(animation);
        self
    }

    /// Sets the track-level rotation animation (video).
    #[must_use]
    pub fn with_rotation_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.rotation = Some(animation);
        self
    }

    /// Sets the track-level volume animation (audio, dB).
    #[must_use]
    pub fn with_volume_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.volume = Some(animation);
        self
    }

    /// Sets the track-level pan animation (audio).
    #[must_use]
    pub fn with_pan_animation(mut self, animation: AnimationTrack<f64>) -> Self {
        self.automation.pan = Some(animation);
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

    #[test]
    fn new_track_should_have_empty_automation() {
        let a = TrackAutomation::default();
        assert!(a.opacity.is_none() && a.x.is_none() && a.volume.is_none() && a.pan.is_none());
    }

    #[test]
    fn with_animation_builders_should_set_each_property() {
        let track = Track::new(vec![])
            .with_opacity_animation(AnimationTrack::new())
            .with_x_animation(AnimationTrack::new())
            .with_y_animation(AnimationTrack::new())
            .with_scale_x_animation(AnimationTrack::new())
            .with_scale_y_animation(AnimationTrack::new())
            .with_rotation_animation(AnimationTrack::new())
            .with_volume_animation(AnimationTrack::new())
            .with_pan_animation(AnimationTrack::new());
        let a = &track.automation;
        assert!(a.opacity.is_some(), "opacity set");
        assert!(a.x.is_some() && a.y.is_some(), "position set");
        assert!(a.scale_x.is_some() && a.scale_y.is_some(), "scale set");
        assert!(a.rotation.is_some(), "rotation set");
        assert!(a.volume.is_some() && a.pan.is_some(), "audio set");
    }

    #[test]
    fn set_video_and_set_audio_should_target_the_right_slot() {
        let mut a = TrackAutomation::default();
        a.set_video(VideoProperty::ScaleY, AnimationTrack::new());
        a.set_audio(AudioProperty::Pan, AnimationTrack::new());
        assert!(a.scale_y.is_some(), "set_video hits scale_y");
        assert!(a.pan.is_some(), "set_audio hits pan");
        assert!(
            a.scale_x.is_none() && a.volume.is_none(),
            "no other slot touched"
        );
    }
}
