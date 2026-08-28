use std::time::Duration;

use super::{AnimationTrack, Lerp};

/// A value that is either constant or animated over time.
///
/// Use [`Static`](AnimatedValue::Static) for values that never change, and
/// [`Track`](AnimatedValue::Track) for values driven by a keyframe
/// [`AnimationTrack`].
///
/// Evaluated at build time via [`value_at(Duration::ZERO)`](Self::value_at) to
/// set initial filter parameters.  Per-frame updates are wired up in issue
/// #363.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(bound(
        serialize = "T: serde::Serialize",
        deserialize = "T: serde::Deserialize<'de>",
    ))
)]
pub enum AnimatedValue<T: Lerp> {
    /// A constant value, independent of time.
    Static(T),
    /// A time-varying value driven by a keyframe track.
    Track(AnimationTrack<T>),
}

impl<T: Lerp> AnimatedValue<T> {
    /// Evaluates the value at time `t`.
    ///
    /// - `Static(v)` — returns a clone of `v` regardless of `t`.
    /// - `Track(track)` — delegates to [`AnimationTrack::value_at`].
    pub fn value_at(&self, t: Duration) -> T {
        match self {
            AnimatedValue::Static(v) => v.clone(),
            AnimatedValue::Track(track) => track.value_at(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Easing, Keyframe};

    #[test]
    fn animated_value_static_should_return_constant_at_any_time() {
        let v: AnimatedValue<f64> = AnimatedValue::Static(42.0);
        assert!(
            (v.value_at(Duration::ZERO) - 42.0).abs() < f64::EPSILON,
            "expected 42.0 at t=0"
        );
        assert!(
            (v.value_at(Duration::from_secs(9999)) - 42.0).abs() < f64::EPSILON,
            "expected 42.0 at t=9999s"
        );
    }

    #[test]
    fn animated_value_track_should_delegate_to_track() {
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.0_f64, Easing::Linear))
            .push(Keyframe::new(
                Duration::from_secs(1),
                1.0_f64,
                Easing::Linear,
            ));
        let v: AnimatedValue<f64> = AnimatedValue::Track(track);
        let mid = v.value_at(Duration::from_millis(500));
        assert!(
            (mid - 0.5).abs() < 1e-9,
            "expected 0.5 at midpoint, got {mid}"
        );
    }
}
