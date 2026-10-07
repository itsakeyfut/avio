//! SMPTE timecode: a frame position written the way a delivery specification writes it.

use std::fmt;

use crate::error::TimecodeError;
use crate::time::Rational;

/// The frame rates SMPTE defines a timecode for, as nominal whole numbers.
///
/// A rate outside this list still works; it is merely not one the standard names, which is
/// the same latitude `libavutil`'s `check_fps` takes (it warns rather than refusing).
const STANDARD_NOMINAL_RATES: &[u32] = &[24, 25, 30, 48, 50, 60, 100, 120, 150];

/// The number of frames in ten minutes of drop-frame timecode at nominal 30.
///
/// Ten minutes of wall clock is 17982 frames rather than 18000 because nine of those ten
/// minutes drop two frame numbers each. Scaled by `nominal / 30` for 60 and 120.
const DROP_FRAMES_PER_10MIN_AT_30: u64 = 17982;

/// A position written as hours, minutes, seconds and frames.
///
/// # What is stored
///
/// A frame count and the rate it is counted at. The four fields a timecode is written with
/// are computed when it is formatted, so they cannot drift out of agreement with each
/// other, and the frame count is the unit every other API here speaks.
///
/// # Drop-frame
///
/// At 29.97 and its multiples, a timecode counted straight would drift away from the wall
/// clock by about 3.6 seconds an hour, because the rate is not 30. Drop-frame timecode
/// closes that by **skipping two frame numbers at the start of every minute except every
/// tenth minute**; no frames are dropped, only their names. The convention is to write it
/// with a semicolon before the frames field:
///
/// ```text
/// 00:01:00;02      drop-frame   (00:01:00;00 and ;01 do not exist)
/// 00:01:00:00      non-drop
/// ```
///
/// **Which rates get it**: those whose nominal rate is a multiple of 30, so 29.97, 59.94
/// and 119.88. Asking for drop-frame at any other rate is an error rather than a silent
/// fallback, because a timecode that says it is drop-frame and is not names a different
/// position. 23.976 is deliberately not included: the standard does not define drop-frame
/// there, however much the rate also fails to be an integer.
///
/// [`at`](Self::at) picks drop-frame wherever it is defined, which is what a broadcast
/// delivery means by a timecode at those rates; [`new`](Self::new) takes the choice
/// explicitly.
///
/// # The day wraps
///
/// A timecode names a position **within a 24-hour day**, so the hours field wraps and
/// [`frame`](Self::frame) is only recoverable from a formatted string for material shorter
/// than a day. At 29.97 frame 2589408 formats as `00:00:00;00`, the same string as frame 0.
/// That is the standard rather than a limitation of this type, and `libavutil` wraps the
/// same way; it is written down here because the round trip otherwise looks like an
/// identity and is not.
///
/// # Examples
///
/// ```
/// use ff_format::{Rational, Timecode};
///
/// let ntsc = Rational::new(30000, 1001);
/// let tc = Timecode::at(1800, ntsc).unwrap();
/// assert_eq!(tc.to_string(), "00:01:00;02");
/// assert!(tc.is_drop_frame());
///
/// let pal = Rational::new(25, 1);
/// assert_eq!(Timecode::at(1500, pal).unwrap().to_string(), "00:01:00:00");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Timecode {
    frame: u64,
    rate: Rational,
    drop_frame: bool,
}

impl Timecode {
    /// A timecode at `frame`, counted at `rate`, with drop-frame chosen explicitly.
    ///
    /// # Errors
    ///
    /// - [`TimecodeError::UnusableRate`] when `rate` is not positive, since a rate that
    ///   does not divide time into frames cannot name one.
    /// - [`TimecodeError::DropFrameUnavailable`] when `drop_frame` is asked for at a rate
    ///   whose nominal value is not a multiple of 30.
    pub fn new(frame: u64, rate: Rational, drop_frame: bool) -> Result<Self, TimecodeError> {
        let nominal = nominal_fps(rate).ok_or(TimecodeError::UnusableRate { rate })?;
        if drop_frame && nominal % 30 != 0 {
            return Err(TimecodeError::DropFrameUnavailable { rate });
        }
        if !STANDARD_NOMINAL_RATES.contains(&nominal) {
            log::warn!(
                "timecode at a rate SMPTE does not name rate={}/{} nominal={nominal}",
                rate.num(),
                rate.den()
            );
        }
        Ok(Self {
            frame,
            rate,
            drop_frame,
        })
    }

    /// A timecode at `frame`, drop-frame wherever the rate defines it.
    ///
    /// # Errors
    ///
    /// [`TimecodeError::UnusableRate`] when `rate` is not positive.
    pub fn at(frame: u64, rate: Rational) -> Result<Self, TimecodeError> {
        let nominal = nominal_fps(rate).ok_or(TimecodeError::UnusableRate { rate })?;
        // `num % den != 0` rather than `den != 1`: the question is whether the rate is a
        // whole number, not whether the ratio is written in lowest terms. `30000/1000` is
        // exactly 30 fps, so it does not drift from the clock and must not drop frames,
        // but its denominator is not 1.
        let is_whole = rate.num() % rate.den() == 0;
        Self::new(frame, rate, nominal % 30 == 0 && !is_whole)
    }

    /// Reads a timecode written as `HH:MM:SS:FF`, or `HH:MM:SS;FF` for drop-frame.
    ///
    /// **The separator is the drop-frame flag**, which is why this takes no third
    /// argument: a string carries the choice its author made, and overriding it here would
    /// let a reader silently relabel someone's delivery point.
    ///
    /// # Errors
    ///
    /// - [`TimecodeError::UnusableRate`] when `rate` is not positive.
    /// - [`TimecodeError::DropFrameUnavailable`] when the string says drop-frame and the
    ///   rate does not define it.
    /// - [`TimecodeError::Malformed`] when the shape is not four numbers with three
    ///   separators.
    /// - [`TimecodeError::FieldOutOfRange`] when a field cannot name what it claims, such
    ///   as a frames field at or above the nominal rate.
    /// - [`TimecodeError::DroppedFrameNumber`] when the string names one of the frame
    ///   numbers drop-frame skips. Refused rather than nudged to the next real frame,
    ///   because nudging moves the position the author wrote.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{Rational, Timecode};
    ///
    /// let ntsc = Rational::new(30000, 1001);
    /// assert_eq!(Timecode::parse("00:01:00;02", ntsc).unwrap().frame(), 1800);
    /// assert!(Timecode::parse("00:01:00;00", ntsc).is_err());   // does not exist
    /// ```
    pub fn parse(text: &str, rate: Rational) -> Result<Self, TimecodeError> {
        let nominal = nominal_fps(rate).ok_or(TimecodeError::UnusableRate { rate })?;

        let malformed = || TimecodeError::Malformed {
            text: text.to_string(),
        };

        // The frames separator is the last one and carries the drop-frame flag; the other
        // two are always colons.
        let (head, frames) = text.rsplit_once([':', ';']).ok_or_else(malformed)?;
        let drop_frame = text
            .as_bytes()
            .get(head.len())
            .copied()
            .ok_or_else(malformed)?
            == b';';

        let mut fields = head.split(':');
        let hours = fields.next().ok_or_else(malformed)?;
        let minutes = fields.next().ok_or_else(malformed)?;
        let seconds = fields.next().ok_or_else(malformed)?;
        if fields.next().is_some() {
            return Err(malformed());
        }

        let hours = field(hours, "hours", 23, text)?;
        let minutes = field(minutes, "minutes", 59, text)?;
        let seconds = field(seconds, "seconds", 59, text)?;
        let frames = field(frames, "frames", u64::from(nominal) - 1, text)?;

        if drop_frame && nominal % 30 != 0 {
            return Err(TimecodeError::DropFrameUnavailable { rate });
        }

        let dropped = u64::from(nominal / 30 * 2);
        if drop_frame && seconds == 0 && minutes % 10 != 0 && frames < dropped {
            return Err(TimecodeError::DroppedFrameNumber {
                text: text.to_string(),
            });
        }

        let nominal = u64::from(nominal);
        let counted = ((hours * 60 + minutes) * 60 + seconds) * nominal + frames;
        let frame = if drop_frame {
            // The inverse of the adjustment `Display` applies: a drop-frame timecode has
            // skipped `dropped` numbers for each minute that is not a tenth.
            let whole_minutes = hours * 60 + minutes;
            counted - dropped * (whole_minutes - whole_minutes / 10)
        } else {
            counted
        };

        Self::new(frame, rate, drop_frame)
    }

    /// The frame this timecode names, counted from zero with nothing skipped.
    #[must_use]
    pub const fn frame(&self) -> u64 {
        self.frame
    }

    /// The rate the frame is counted at.
    #[must_use]
    pub const fn rate(&self) -> Rational {
        self.rate
    }

    /// Whether this timecode skips frame numbers to track the wall clock.
    #[must_use]
    pub const fn is_drop_frame(&self) -> bool {
        self.drop_frame
    }

    /// The whole-number rate the four fields are written against: 30 for 29.97.
    #[must_use]
    pub fn nominal_fps(&self) -> u32 {
        nominal_fps(self.rate).unwrap_or(1)
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nominal = u64::from(self.nominal_fps());
        let numbered = if self.drop_frame {
            adjust_drop_frame(self.frame, nominal)
        } else {
            self.frame
        };

        let frames = numbered % nominal;
        let seconds = numbered / nominal % 60;
        let minutes = numbered / (nominal * 60) % 60;
        let hours = numbered / (nominal * 3600) % 24;
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(f, "{hours:02}:{minutes:02}:{seconds:02}{sep}{frames:02}")
    }
}

/// The whole-number rate a ratio names, or `None` when it names no rate at all.
///
/// Rounds rather than truncates, so `30000/1001` is 30 and not 29. This is
/// `libavutil/timecode.c`'s `fps_from_frame_rate`, which the drop-frame rules below are
/// written against, so it has to agree with it exactly.
fn nominal_fps(rate: Rational) -> Option<u32> {
    if !rate.is_positive() {
        return None;
    }
    let num = i64::from(rate.num());
    let den = i64::from(rate.den());
    let nominal = (num + den / 2) / den;
    u32::try_from(nominal).ok().filter(|&n| n > 0)
}

/// Turns a frame index into the number drop-frame timecode writes for it.
///
/// `libavutil/timecode.c`'s `av_timecode_adjust_ntsc_framenum2`, which is the only
/// definition of this worth having: the rule is a convention rather than arithmetic that
/// can be derived, and getting the ten-minute exception wrong produces a timecode that is
/// correct for the first minute and wrong afterwards.
///
/// `nominal` must be a multiple of 30; [`Timecode::new`] is what guarantees it.
fn adjust_drop_frame(frame: u64, nominal: u64) -> u64 {
    let dropped = nominal / 30 * 2;
    let per_10min = nominal / 30 * DROP_FRAMES_PER_10MIN_AT_30;

    let tens = frame / per_10min;
    let within = frame % per_10min;

    // `within.saturating_sub(dropped)` where the C writes `(m - drop_frames)` on a signed
    // int: the quotient is zero either way for the first two frames of a ten-minute span,
    // because C truncates toward zero.
    frame + 9 * dropped * tens + dropped * (within.saturating_sub(dropped) / (per_10min / 10))
}

/// Parses one field of a timecode, refusing a value the field cannot name.
fn field(text: &str, name: &'static str, max: u64, whole: &str) -> Result<u64, TimecodeError> {
    let value = text.parse::<u64>().map_err(|_| TimecodeError::Malformed {
        text: whole.to_string(),
    })?;
    if value > max {
        return Err(TimecodeError::FieldOutOfRange {
            field: name,
            value,
            max,
        });
    }
    Ok(value)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{Timecode, nominal_fps};
    use crate::error::TimecodeError;
    use crate::time::Rational;

    fn ntsc() -> Rational {
        Rational::new(30_000, 1001)
    }

    fn ntsc_60() -> Rational {
        Rational::new(60_000, 1001)
    }

    fn film() -> Rational {
        Rational::new(24_000, 1001)
    }

    #[test]
    fn nominal_fps_should_round_rather_than_truncate() {
        // `libavutil`'s `fps_from_frame_rate`: truncating would make 29.97 into 29 and
        // every field below wrong.
        assert_eq!(nominal_fps(ntsc()), Some(30));
        assert_eq!(nominal_fps(film()), Some(24));
        assert_eq!(nominal_fps(ntsc_60()), Some(60));
        assert_eq!(nominal_fps(Rational::new(25, 1)), Some(25));
        assert_eq!(nominal_fps(Rational::new(0, 1)), None);
        assert_eq!(nominal_fps(Rational::new(30, 0)), None);
    }

    #[test]
    fn timecode_should_format_non_drop_with_colons() {
        let pal = Rational::new(25, 1);
        assert_eq!(Timecode::at(1500, pal).unwrap().to_string(), "00:01:00:00");
        assert_eq!(
            Timecode::new(1800, ntsc(), false).unwrap().to_string(),
            "00:01:00:00",
            "non-drop at 29.97 counts straight, which is why it drifts from the clock"
        );
    }

    #[test]
    fn timecode_should_format_drop_frame_with_a_semicolon() {
        let tc = Timecode::at(1800, ntsc()).unwrap();
        assert!(tc.is_drop_frame());
        assert_eq!(tc.to_string(), "00:01:00;02");
    }

    /// The expected strings come from porting `av_timecode_adjust_ntsc_framenum2` and
    /// running it, not from working the convention out by hand.
    #[test]
    fn timecode_should_drop_two_frame_numbers_every_minute_except_every_tenth() {
        for (frame, expected) in [
            (0u64, "00:00:00;00"),
            (1799, "00:00:59;29"),
            (1800, "00:01:00;02"), // the minute starts at ;02
            (1801, "00:01:00;03"),
            (17_981, "00:09:59;29"),
            (17_982, "00:10:00;00"), // the tenth minute drops nothing
            (17_983, "00:10:00;01"),
            (107_892, "01:00:00;00"), // six ten-minute spans is exactly an hour
        ] {
            assert_eq!(
                Timecode::at(frame, ntsc()).unwrap().to_string(),
                expected,
                "frame {frame} at 29.97"
            );
        }
    }

    /// At 59.94 four numbers are dropped rather than two, which is what the `nominal / 30`
    /// scaling is for.
    #[test]
    fn timecode_should_drop_four_frame_numbers_a_minute_at_59_94() {
        for (frame, expected) in [
            (0u64, "00:00:00;00"),
            (3600, "00:01:00;04"),
            (35_964, "00:10:00;00"),
        ] {
            assert_eq!(
                Timecode::at(frame, ntsc_60()).unwrap().to_string(),
                expected,
                "frame {frame} at 59.94"
            );
        }
    }

    #[test]
    fn timecode_should_round_trip_through_parse_and_format() {
        // Every nominal rate SMPTE names, at the ratio each one is usually written as.
        for rate in [
            Rational::new(24, 1),
            film(),
            Rational::new(25, 1),
            Rational::new(30, 1),
            ntsc(),
            Rational::new(48, 1),
            Rational::new(50, 1),
            Rational::new(60, 1),
            ntsc_60(),
            Rational::new(100, 1),
            Rational::new(120, 1),
            Rational::new(150, 1),
        ] {
            let nominal = u64::from(nominal_fps(rate).unwrap());
            // Spread over an hour, and deliberately across minute boundaries where
            // drop-frame does its skipping. **Inside one day**: past that the hours field
            // wraps and the round trip is not an identity, which
            // `a_timecode_should_name_a_position_within_a_day` pins separately.
            for frame in [0, 1, nominal - 1, nominal, 1799, 1800, 17_982, 107_892] {
                let tc = Timecode::at(frame, rate).unwrap();
                let text = tc.to_string();
                let back = Timecode::parse(&text, rate).unwrap();
                assert_eq!(
                    back.frame(),
                    frame,
                    "{text} at {}/{} came back as frame {}",
                    rate.num(),
                    rate.den(),
                    back.frame()
                );
                assert_eq!(back.is_drop_frame(), tc.is_drop_frame());
            }
        }
    }

    /// The round trip holds within a day and not beyond it, because the hours field
    /// wraps. Asserted rather than left implicit: a caller reading
    /// `timecode_should_round_trip_through_parse_and_format` would otherwise take the
    /// property to be unconditional.
    #[test]
    fn a_timecode_should_name_a_position_within_a_day() {
        // 24 hours of drop-frame at 29.97 is 24 * 6 * 17982 frames.
        let day = 24 * 6 * 17_982;
        let last = Timecode::at(day - 1, ntsc()).unwrap();
        assert_eq!(last.to_string(), "23:59:59;29");

        let wrapped = Timecode::at(day, ntsc()).unwrap();
        assert_eq!(
            wrapped.to_string(),
            "00:00:00;00",
            "the day wraps, so this is the same string frame 0 writes"
        );
        assert_eq!(
            wrapped.frame(),
            day,
            "the frame count itself is kept, so only the name repeats"
        );
        assert_eq!(
            Timecode::parse(&wrapped.to_string(), ntsc())
                .unwrap()
                .frame(),
            0,
            "and reading that string back can only give the first day's frame"
        );
    }

    #[test]
    fn timecode_should_refuse_a_dropped_frame_number() {
        for text in ["00:01:00;00", "00:01:00;01", "00:09:00;01"] {
            let err = Timecode::parse(text, ntsc()).unwrap_err();
            assert!(
                matches!(err, TimecodeError::DroppedFrameNumber { .. }),
                "{text} names a frame drop-frame skips, got {err:?}"
            );
        }
        // The tenth minute drops nothing, so these do exist.
        for text in ["00:10:00;00", "00:00:00;00", "00:20:00;01"] {
            assert!(
                Timecode::parse(text, ntsc()).is_ok(),
                "{text} is a real frame at 29.97"
            );
        }
    }

    #[test]
    fn timecode_should_refuse_drop_frame_where_it_is_undefined() {
        let err = Timecode::new(0, film(), true).unwrap_err();
        assert!(matches!(err, TimecodeError::DropFrameUnavailable { .. }));

        let err = Timecode::parse("00:00:00;00", film()).unwrap_err();
        assert!(
            matches!(err, TimecodeError::DropFrameUnavailable { .. }),
            "a semicolon at 23.976 is a claim the rate cannot honour, got {err:?}"
        );
    }

    #[test]
    fn at_should_choose_drop_frame_only_where_the_rate_defines_it() {
        assert!(Timecode::at(0, ntsc()).unwrap().is_drop_frame());
        assert!(Timecode::at(0, ntsc_60()).unwrap().is_drop_frame());
        assert!(
            !Timecode::at(0, film()).unwrap().is_drop_frame(),
            "23.976 is not an integer rate either, but SMPTE defines no drop-frame there"
        );
        assert!(
            !Timecode::at(0, Rational::new(30, 1))
                .unwrap()
                .is_drop_frame(),
            "exactly 30 needs no dropping: it does not drift from the clock"
        );
        // An un-reduced whole rate is still whole. `den != 1` would have called these
        // drop-frame, which is why the predicate asks whether the rate divides.
        for (num, den) in [(30_000, 1000), (60_000, 1000), (90, 3)] {
            assert!(
                !Timecode::at(0, Rational::new(num, den))
                    .unwrap()
                    .is_drop_frame(),
                "{num}/{den} is a whole number of frames per second"
            );
        }
    }

    #[test]
    fn timecode_should_refuse_a_malformed_string() {
        for text in [
            "",
            "00:00:00",
            "00:00:00:00:00",
            "aa:00:00:00",
            "00-00-00-00",
        ] {
            let err = Timecode::parse(text, Rational::new(25, 1)).unwrap_err();
            assert!(
                matches!(err, TimecodeError::Malformed { .. }),
                "{text:?} is not a timecode, got {err:?}"
            );
        }
    }

    #[test]
    fn timecode_should_refuse_a_field_out_of_range() {
        for (text, field) in [
            ("24:00:00:00", "hours"),
            ("00:60:00:00", "minutes"),
            ("00:00:60:00", "seconds"),
            ("00:00:00:25", "frames"),
        ] {
            let err = Timecode::parse(text, Rational::new(25, 1)).unwrap_err();
            assert!(
                matches!(err, TimecodeError::FieldOutOfRange { field: got, .. } if got == field),
                "{text} should be out of range in {field}, got {err:?}"
            );
        }
    }

    #[test]
    fn timecode_should_refuse_an_unusable_rate() {
        for rate in [
            Rational::new(0, 1),
            Rational::new(-30, 1),
            Rational::new(30, 0),
        ] {
            assert!(matches!(
                Timecode::at(0, rate).unwrap_err(),
                TimecodeError::UnusableRate { .. }
            ));
        }
    }
}
