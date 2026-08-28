use std::cmp::Ordering;
use std::time::Duration;

use super::{Easing, Lerp};

/// A single keyframe in an animation track.
///
/// The `easing` field controls the interpolation from **this** keyframe to the
/// next one.  The last keyframe's `easing` is never used (there is no
/// subsequent keyframe to interpolate toward).
///
/// # Ordering
///
/// Keyframes are ordered and compared by `timestamp` only.  Two keyframes at
/// the same timestamp are considered equal regardless of their values or easing
/// — this keeps binary-search by timestamp correct inside
/// `AnimationTrack` (added in issue #350).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(bound(
        serialize = "T: serde::Serialize",
        deserialize = "T: serde::Deserialize<'de>",
    ))
)]
pub struct Keyframe<T: Lerp> {
    /// Position of this keyframe on the timeline.
    pub timestamp: Duration,
    /// Value held at (and interpolated from) this keyframe.
    pub value: T,
    /// Easing applied for the transition from this keyframe to the next.
    pub easing: Easing,
}

impl<T: Lerp> Keyframe<T> {
    /// Creates a new keyframe.
    pub fn new(timestamp: Duration, value: T, easing: Easing) -> Self {
        Self {
            timestamp,
            value,
            easing,
        }
    }
}

// Ordering by timestamp only

impl<T: Lerp> PartialEq for Keyframe<T> {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl<T: Lerp> Eq for Keyframe<T> {}

impl<T: Lerp> PartialOrd for Keyframe<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Lerp> Ord for Keyframe<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.timestamp.cmp(&other.timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal Lerp impl used only within these tests.
    // The real impl for f64 is added in issue #351.
    #[derive(Clone, Debug)]
    struct TestVal(f64);

    impl Lerp for TestVal {
        fn lerp(a: &Self, b: &Self, t: f64) -> Self {
            TestVal(a.0 + (b.0 - a.0) * t)
        }
    }

    fn kf(ms: u64, v: f64) -> Keyframe<TestVal> {
        Keyframe::new(Duration::from_millis(ms), TestVal(v), Easing::Linear)
    }

    #[test]
    fn keyframe_new_should_store_all_fields() {
        let ts = Duration::from_millis(500);
        let kf = Keyframe::new(ts, TestVal(1.0), Easing::EaseInOut);
        assert_eq!(kf.timestamp, ts);
        assert!((kf.value.0 - 1.0).abs() < f64::EPSILON);
        assert!(matches!(kf.easing, Easing::EaseInOut));
    }

    #[test]
    fn keyframe_should_order_by_timestamp() {
        let a = kf(100, 0.0);
        let b = kf(200, 0.5);
        let c = kf(300, 1.0);

        assert!(a < b);
        assert!(b < c);
        assert!(a < c);

        let mut frames = vec![c, a, b];
        frames.sort();
        assert_eq!(frames[0].timestamp, Duration::from_millis(100));
        assert_eq!(frames[1].timestamp, Duration::from_millis(200));
        assert_eq!(frames[2].timestamp, Duration::from_millis(300));
    }

    #[test]
    fn keyframe_should_compare_equal_by_timestamp_only() {
        let a = Keyframe::new(Duration::from_millis(100), TestVal(0.0), Easing::Linear);
        let b = Keyframe::new(Duration::from_millis(100), TestVal(99.0), Easing::Hold);
        // Same timestamp → equal regardless of value or easing.
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), Ordering::Equal);
    }

    #[test]
    fn keyframe_should_be_less_than_later_keyframe() {
        let early = kf(0, 0.0);
        let late = kf(1000, 1.0);
        assert!(early < late);
        assert!(late > early);
    }
}
