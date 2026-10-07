//! A frame survives the trip to a `Duration` and back, at every rate avio is for.
//!
//! This is #1827's fourth acceptance criterion, met here because #1947 is the change that
//! makes it possible: until the model held a ratio it could not tell `29.97` from
//! `30000/1001`, so there was no exact answer to round-trip to.
//!
//! **The lossy step is the `Duration` conversion, not the rate.** That is worth stating
//! because the opposite is the natural assumption, and it was the assumption #1827's body
//! was written on. One frame at 30 fps is 33333333.33... ns and `Duration` holds whole
//! nanoseconds, so truncating the reverse conversion is wrong at 30 fps exactly as much as
//! at 29.97. Measured over these frames and rates, flooring loses between 135 and 783 of
//! 2006 frames **at every rate including 25 and 50**, where one frame *is* a whole number
//! of nanoseconds: `Duration::from_secs_f64(n / fps)` is inexact on its own. Rounding
//! loses none at any rate.
//!
//! So these tests assert two things that have to stay true together: rounding round-trips
//! every frame, and flooring does not. The second is what stops the rounding rule from
//! being simplified away later as incidental.
//!
//! Pure arithmetic: no FFmpeg, no filters, no files, so this runs everywhere.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use avio::{Clip, Timeline};
use ff_format::Rational;

/// Every rate the engine is expected to hold.
///
/// Flooring the reverse conversion loses frames at all of them, 25 and 50 included: a
/// whole-nanosecond frame period is not enough, because the forward conversion through
/// `f64` seconds is itself inexact.
const RATES: &[(i32, i32)] = &[
    (30000, 1001), // 29.97
    (24000, 1001), // 23.976
    (60000, 1001), // 59.94
    (48000, 1001), // 47.952
    (30, 1),
    (24, 1),
    (25, 1),
    (50, 1),
];

/// Frames spread over a long timeline: the first few, where a one-nanosecond truncation is
/// a large fraction of a frame, and far-out ones, where the nanosecond count is large
/// enough to test that an `f64` still holds it exactly.
fn frames() -> Vec<u64> {
    let mut v: Vec<u64> = (0..2000).collect();
    v.extend([30, 1800, 108_000, 1_000_000, 5_000_000, 10_000_000]);
    v
}

/// The position of frame `n`, through the engine's own conversion.
///
/// A timeline rather than a local helper, so this exercises the code the export uses.
/// Until #1827 these were reimplemented here, which pinned the *rule* but not the code:
/// `derive::offset_frames` is `pub(crate)` and unreachable from an integration test.
fn timeline_at(rate: Rational) -> Timeline {
    Timeline::builder()
        .canvas(64, 64)
        .frame_rate(rate)
        .video_track(vec![Clip::new("never-opened.mp4")])
        .build()
        .expect("a timeline whose clip is never opened still builds")
}

fn position(n: u64, rate: Rational) -> Duration {
    timeline_at(rate).position_of_frame(n)
}

/// Which frame a position falls on, rounding, which is the engine's placement rule.
fn frame_at_rounding(at: Duration, rate: Rational) -> u64 {
    timeline_at(rate).frame_at(at)
}

/// The same, truncating, which is the rule this change had to not adopt.
fn frame_at_flooring(at: Duration, rate: Rational) -> u64 {
    (at.as_secs_f64() * rate.as_f64()) as u64
}

#[test]
fn a_frame_should_survive_the_round_trip_at_every_rate() {
    for &(num, den) in RATES {
        let rate = Rational::new(num, den);
        for n in frames() {
            let back = frame_at_rounding(position(n, rate), rate);
            assert_eq!(
                back, n,
                "frame {n} at {num}/{den} came back as {back}; the round trip must be exact"
            );
        }
    }
}

/// The companion assertion, and the reason the rounding rule is load-bearing rather than
/// incidental: truncating loses frames at **every** rate. If this ever stops being true,
/// the measurement this change rests on has changed and the rounding rule wants
/// re-deriving rather than deleting.
#[test]
fn flooring_the_reverse_conversion_should_lose_frames_at_every_rate() {
    for &(num, den) in RATES {
        let rate = Rational::new(num, den);
        let lost = frames()
            .into_iter()
            .filter(|&n| frame_at_flooring(position(n, rate), rate) != n)
            .count();
        assert!(
            lost > 0,
            "flooring must lose frames at {num}/{den}, which is why the placement rule rounds"
        );
    }
}

/// A ratio is not interchangeable with the decimal that names it, which is the whole
/// reason the model stopped storing a decimal.
///
/// `29.97` and `30000/1001` differ by about 3 parts in 10 million. Over a long enough
/// timeline that reaches a frame, and the two then disagree about where a clip sits.
#[test]
fn the_decimal_and_the_ratio_should_disagree_on_a_long_timeline() {
    let exact = Rational::new(30000, 1001);
    let decimal = 29.97_f64;

    // An hour in. The two rates differ by 29.97002997... - 29.97 = 2.997e-5 fps, which
    // over 3600 s is about 0.108 of a frame; by ten hours it is more than a frame.
    let ten_hours = Duration::from_secs(36_000);
    let with_exact = (ten_hours.as_secs_f64() * exact.as_f64()).round() as u64;
    let with_decimal = (ten_hours.as_secs_f64() * decimal).round() as u64;

    assert_ne!(
        with_exact, with_decimal,
        "if these agreed, storing the decimal would have been good enough"
    );
    assert!(
        with_exact > with_decimal,
        "30000/1001 is the larger rate, so it counts more frames in the same time"
    );
}
