//! Time primitives for video/audio processing.
//!
//! This module provides [`Rational`] for representing fractions (like time bases
//! and frame rates) and [`Timestamp`] for representing media timestamps with
//! their associated time base.
//!
//! # Examples
//!
//! ```
//! use ff_format::{Rational, Timestamp};
//! use std::time::Duration;
//!
//! // Create a rational number (e.g., 1/90000 time base)
//! let time_base = Rational::new(1, 90000);
//! assert_eq!(time_base.as_f64(), 1.0 / 90000.0);
//!
//! // Create a timestamp at 1 second (90000 * 1/90000)
//! let ts = Timestamp::new(90000, time_base);
//! assert!((ts.as_secs_f64() - 1.0).abs() < 0.0001);
//!
//! // Convert to Duration
//! let duration = ts.as_duration();
//! assert_eq!(duration.as_secs(), 1);
//! ```
//!
//! # Frames, and why the reverse conversion rounds
//!
//! [`Timestamp::from_frame_number`] and [`Timestamp::as_frame_number_rational`] are a pair,
//! and the second one **rounds**. That is not a stylistic choice and it must not be
//! simplified to a cast.
//!
//! **The lossy step is the `Duration` conversion, not the frame rate.** One frame at 30 fps
//! is 33333333.33... nanoseconds and a `Duration` holds whole nanoseconds, so a position
//! computed from a frame and then converted back lands just below the integer. Truncating
//! there loses the frame. Measured over 2006 frames in `avio`'s
//! `tests/frame_rate_round_trip.rs`, flooring the reverse conversion is wrong for between
//! 135 and 783 of them **at every rate including 25 and 50**, where one frame *is* a whole
//! number of nanoseconds; rounding is wrong for none at any rate. The broadcast rates are
//! not a special case here, which is the part that reads as surprising.
//!
//! [`Timecode`] writes a frame position the way a delivery specification writes it, and
//! carries the drop-frame rules.

mod rational;
mod timecode;
mod timestamp;

pub use rational::Rational;
pub use timecode::Timecode;
pub use timestamp::Timestamp;

/// Which frame a position `seconds` into a stream running at `fps` falls on.
///
/// **The one rounding rule**, so that a caller asking where a frame is and a renderer
/// deciding where to put one cannot come to different answers. Everything that converts a
/// position to a frame goes through here: [`Timestamp::as_frame_number`] and, in `avio`,
/// the export's own placement.
///
/// # Rounding, not truncating
///
/// Nearest, which is load-bearing rather than a preference. Converting a position into a
/// `Duration` is inexact at every rate, because a frame is rarely a whole number of
/// nanoseconds and the conversion through `f64` seconds is itself approximate, so
/// truncating here loses frames. Measured over 2006 frames in `avio`'s
/// `tests/frame_rate_round_trip.rs`, flooring is wrong for between 135 and 783 of them at
/// every rate **including 25 and 50**, where one frame *is* a whole number of nanoseconds.
/// Rounding is wrong for none at any rate.
///
/// `None` when `seconds` or `fps` cannot name a frame: a rate that is not positive and
/// finite, or a position that is negative or not finite. Returned rather than clamped so a
/// caller can fall back to an unquantised position instead of collapsing to frame zero.
///
/// # Examples
///
/// ```
/// use ff_format::time::frame_at_seconds;
///
/// // One second at 29.97 is 29.97002997... frames, and the nearest is 30.
/// assert_eq!(frame_at_seconds(1.0, 30000.0 / 1001.0), Some(30));
/// assert_eq!(frame_at_seconds(-1.0, 30.0), None);
/// assert_eq!(frame_at_seconds(1.0, 0.0), None);
/// ```
#[must_use]
pub fn frame_at_seconds(seconds: f64, fps: f64) -> Option<u64> {
    if !fps.is_finite() || fps <= 0.0 || !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the guard above rejects a negative or non-finite input, and the cast saturates rather than wrapping"
    )]
    Some((seconds * fps).round() as u64)
}
