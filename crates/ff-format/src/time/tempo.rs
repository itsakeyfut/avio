//! A tempo, and the conversion between a beat position and a wall-clock position.

use std::time::Duration;

use crate::error::TempoError;
use crate::time::{Beats, Rational};

/// Nanoseconds in a second, and seconds in a minute, as the conversions use them.
const NANOS_PER_SEC: i128 = 1_000_000_000;
const SECS_PER_MIN: i128 = 60;

/// A tempo, in beats per minute.
///
/// A fraction rather than a decimal, for the reason the frame rate is one: 93.75 BPM is
/// `375/4` exactly, and a tempo that cannot be written down exactly puts every beat
/// slightly off.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use ff_format::{Beats, Rational, Tempo};
///
/// let tempo = Tempo::new(Rational::new(174, 1)).unwrap();
///
/// // One beat at 174 BPM is 60/174 s, which is 344.827586... ms.
/// let beat_one = tempo.position_of(Beats::new(Rational::new(1, 1)));
/// assert_eq!(beat_one, Duration::from_nanos(344_827_586));
///
/// // And it reads back as beat 1 on a grid it sits on.
/// assert_eq!(
///     tempo.beat_at(beat_one, 1).unwrap().count(),
///     Rational::new(1, 1)
/// );
/// ```
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tempo(Rational);

impl Tempo {
    /// A tempo of `bpm` beats per minute.
    ///
    /// # Errors
    ///
    /// [`TempoError::UnusableRate`] when `bpm` is not positive, which for a fraction
    /// includes a zero denominator. A tempo that does not divide time into beats cannot
    /// name one.
    pub fn new(bpm: Rational) -> Result<Self, TempoError> {
        if !bpm.is_positive() {
            return Err(TempoError::UnusableRate { bpm });
        }
        Ok(Self(bpm))
    }

    /// The tempo, in beats per minute.
    #[must_use]
    pub const fn bpm(self) -> Rational {
        self.0
    }

    /// Where `beat` falls on the wall clock.
    ///
    /// Exact to the nanosecond a `Duration` can hold: the position is
    /// `beat * 60 / bpm` seconds, computed from the components rather than through
    /// `Rational`'s operators, which would wrap silently on the way back to `i32`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use ff_format::{Beats, Rational, Tempo};
    ///
    /// let tempo = Tempo::new(Rational::new(174, 1)).unwrap();
    /// // Beat 3.5 is 7/2 beats, which is 1206.896551... ms.
    /// assert_eq!(
    ///     tempo.position_of(Beats::new(Rational::new(7, 2))),
    ///     Duration::from_nanos(1_206_896_552)
    /// );
    /// ```
    #[must_use]
    pub fn position_of(self, beat: Beats) -> Duration {
        let count = beat.count();
        let (bn, bd) = (i128::from(count.num()), i128::from(count.den()));
        let (tn, td) = (i128::from(self.0.num()), i128::from(self.0.den()));

        // A degenerate count or a beat before the start has no position on the clock.
        // `Tempo::new` has already refused a degenerate tempo.
        if bd == 0 || bn < 0 {
            return Duration::ZERO;
        }

        // ns = beat * 60 / bpm, in nanoseconds:
        //   bn/bd beats * 60 s/min * td/tn min/beat * 1e9 ns/s
        //
        // `i128` is wide enough by construction rather than by luck: the largest numerator
        // any `i32` inputs can make is i32::MAX * 60 * i32::MAX * 1e9, about 2.8e29,
        // against `i128`'s 1.7e38.
        let num = bn * SECS_PER_MIN * td * NANOS_PER_SEC;
        let den = bd * tn;
        let nanos = round_div(num, den);

        u64::try_from(nanos).map_or(Duration::MAX, Duration::from_nanos)
    }

    /// Which beat `position` falls on, rounded to the nearest `1/subdivision` of a beat.
    ///
    /// # Why the grid is a parameter
    ///
    /// Because the exact answer is neither representable nor the one anybody wants. Beat 1
    /// at 174 BPM is 344827586 ns, and converting *that* back exactly gives
    /// `4999999997/5000000000` rather than 1: a `Duration` holds whole nanoseconds, so the
    /// forward conversion already rounded. Reading it back therefore has to round too, and
    /// only the caller knows the grid it is authoring against. A fixed resolution would
    /// have to be chosen, and any choice loses the subdivisions it does not divide: 960
    /// cannot hold a septuplet.
    ///
    /// So a position written on a grid reads back exactly on that grid, which is the
    /// property a host snapping to one needs.
    ///
    /// `None` when `subdivision` is zero, or when the resulting count is not
    /// representable. Thirty minutes at 174 BPM on a 1/960 grid needs a numerator of
    /// 5,011,200, which is 0.23% of `i32`, so the refusal is a bound rather than a
    /// limitation in practice.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use ff_format::{Rational, Tempo};
    ///
    /// let tempo = Tempo::new(Rational::new(174, 1)).unwrap();
    /// // A third of a beat, read back on a grid of thirds.
    /// let third = tempo.position_of(ff_format::Beats::new(Rational::new(1, 3)));
    /// assert_eq!(tempo.beat_at(third, 3).unwrap().count(), Rational::new(1, 3));
    /// ```
    #[must_use]
    pub fn beat_at(self, position: Duration, subdivision: u32) -> Option<Beats> {
        if subdivision == 0 {
            return None;
        }
        let nanos = i128::try_from(position.as_nanos()).ok()?;
        let (tn, td) = (i128::from(self.0.num()), i128::from(self.0.den()));
        let sub = i128::from(subdivision);

        // grid points = position_s * bpm/60 * subdivision, rounded to nearest.
        let num = nanos * tn * sub;
        let den = NANOS_PER_SEC * SECS_PER_MIN * td;
        let points = round_div(num, den);

        let num = i32::try_from(points).ok()?;
        let den = i32::try_from(sub).ok()?;
        Some(Beats::new(Rational::new(num, den)))
    }
}

/// `num / den` for non-negative `num` and positive `den`, rounded to nearest.
///
/// Rounding rather than truncating, for the reason the frame conversion rounds: the value
/// being divided has already been quantised once, so truncating compounds that into a lost
/// beat instead of the nearest one.
///
/// # Why there is no sign handling
///
/// Both call sites have already established that neither argument can be negative, so a
/// signed branch here would be unreachable code that still had to be right. `position_of`
/// returns early for a negative beat count and a zero denominator, and `beat_at` starts
/// from a `Duration`, a positive tempo and a non-zero subdivision. `Rational::new`
/// normalises a negative denominator away, so no `Rational` reaching either one can carry
/// one. The debug assertion records that rather than leaving it to be re-derived.
fn round_div(num: i128, den: i128) -> i128 {
    debug_assert!(
        num >= 0 && den > 0,
        "round_div is for non-negative over positive: {num} / {den}"
    );
    (num + den / 2) / den
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::{Tempo, round_div};
    use crate::error::TempoError;
    use crate::time::{Beats, Rational};

    fn tempo(num: i32, den: i32) -> Tempo {
        Tempo::new(Rational::new(num, den)).unwrap()
    }

    fn beats(num: i32, den: i32) -> Beats {
        Beats::new(Rational::new(num, den))
    }

    #[test]
    fn round_div_should_round_to_nearest() {
        assert_eq!(round_div(7, 2), 4);
        assert_eq!(round_div(5, 2), 3);
        assert_eq!(round_div(4, 2), 2);
        assert_eq!(round_div(1, 3), 0);
        assert_eq!(round_div(2, 3), 1);
        assert_eq!(round_div(0, 7), 0);
        // A half rounds up, which is the tie this never has to break in anger but should
        // break predictably.
        assert_eq!(round_div(1, 2), 1);
        assert_eq!(round_div(3, 2), 2);
    }

    #[test]
    fn tempo_should_refuse_a_non_positive_rate() {
        for (num, den) in [(0, 1), (-174, 1), (174, 0), (0, 0)] {
            let err = Tempo::new(Rational::new(num, den)).unwrap_err();
            assert!(
                matches!(err, TempoError::UnusableRate { .. }),
                "{num}/{den} is not a tempo, got {err:?}"
            );
        }
    }

    /// Hand-computed nanoseconds, so the test cannot agree with the code by performing the
    /// same arithmetic twice. Each is `beat * 60 / bpm` seconds truncated to nanoseconds.
    #[test]
    fn position_of_should_place_a_beat_at_its_exact_nanosecond() {
        let t = tempo(174, 1);
        // Each figure is `beat * 60 / bpm` worked out as an exact fraction and rounded to
        // the nearest nanosecond, which is what a `Duration` holds. Rounding, not
        // truncating: 7/2 below differs between the two, and truncating is the mutation
        // this row catches.
        //
        // 60/174 s = 0.3448275862069... s
        assert_eq!(
            t.position_of(beats(1, 1)),
            Duration::from_nanos(344_827_586)
        );
        // 7/2 beats = 1.206896551724... s, whose nearest nanosecond is ...552.
        assert_eq!(
            t.position_of(beats(7, 2)),
            Duration::from_nanos(1_206_896_552)
        );
        // 1/3 beat = 0.114942528735... s
        assert_eq!(
            t.position_of(beats(1, 3)),
            Duration::from_nanos(114_942_529)
        );
        // A large count, which is the row an `i64` intermediate would get wrong.
        // 100000 beats at 174 BPM = 34482.758620689... s
        assert_eq!(
            t.position_of(beats(100_000, 1)),
            Duration::from_nanos(34_482_758_620_690)
        );
    }

    /// The row that needs the `i128` intermediate, and the only kind that does.
    ///
    /// `Beats` can hold a count up to `i32::MAX`, and the numerator of the conversion is
    /// `beat * 60 * bpm.den * 1e9`. At beat 200,000,000 and 2 BPM that is 1.2e19, which
    /// **overflows `i64`** (9.2e18) while the result, 6e18 ns, still fits the `u64` a
    /// `Duration` is built from. Computing the intermediate in `i64` panics here in debug
    /// and wraps in release, so this is what keeps the `i128` from looking like caution
    /// that could be simplified away.
    #[test]
    fn position_of_should_hold_a_product_that_overflows_i64() {
        let t = tempo(2, 1);
        assert_eq!(
            t.position_of(beats(200_000_000, 1)),
            Duration::from_nanos(6_000_000_000_000_000_000)
        );
    }

    /// A non-integer tempo, which is the case a decimal BPM could not hold exactly.
    #[test]
    fn position_of_should_handle_a_fractional_tempo() {
        // 93.75 BPM is 375/4. One beat is 60 * 4 / 375 = 0.64 s exactly.
        let t = tempo(375, 4);
        assert_eq!(
            t.position_of(beats(1, 1)),
            Duration::from_nanos(640_000_000)
        );
    }

    #[test]
    fn beat_at_should_round_trip_a_position_on_its_own_grid() {
        for (tn, td) in [(174, 1), (375, 4), (128, 1), (30_000, 1001)] {
            let t = tempo(tn, td);
            for sub in [1u32, 2, 3, 4, 7, 16, 960] {
                for step in [0i32, 1, 2, 5, 31] {
                    #[expect(
                        clippy::cast_possible_wrap,
                        reason = "the subdivisions tested are all far inside i32"
                    )]
                    let count = Rational::new(step, sub as i32);
                    let position = t.position_of(Beats::new(count));
                    let back = t.beat_at(position, sub).expect("representable");
                    assert_eq!(
                        back.count(),
                        count,
                        "{step}/{sub} at {tn}/{td} came back as {}/{}",
                        back.count().num(),
                        back.count().den()
                    );
                }
            }
        }
    }

    /// Rounding rather than truncating, pinned by a position deliberately off the grid.
    #[test]
    fn beat_at_should_round_to_the_nearest_grid_point() {
        let t = tempo(60, 1); // one beat per second, so the arithmetic is readable
        // 0.6 s is 0.6 beats. On a grid of halves the nearest point is 1/2 (0.5 is
        // 0.1 away) rather than 1 (0.4 away)... no: 0.6 is 0.1 from 0.5 and 0.4 from 1.0,
        // so the nearest half is 1/2.
        assert_eq!(
            t.beat_at(Duration::from_millis(600), 2).unwrap().count(),
            Rational::new(1, 2)
        );
        // 0.8 s on a grid of halves: 0.8 is 0.3 from 0.5 and 0.2 from 1.0, so it rounds
        // *up* to 2/2. Truncating would give 1/2, which is the mutation this catches.
        assert_eq!(
            t.beat_at(Duration::from_millis(800), 2).unwrap().count(),
            Rational::new(2, 2)
        );
    }

    #[test]
    fn beat_at_should_refuse_a_zero_subdivision() {
        assert_eq!(tempo(174, 1).beat_at(Duration::from_secs(1), 0), None);
    }

    #[test]
    fn beat_at_should_refuse_a_numerator_i32_cannot_hold() {
        // A very long position on a very fine grid: 4285 hours at 174 BPM on a 1/960 grid
        // needs about 4.3e10 grid points, which no `i32` holds.
        let t = tempo(174, 1);
        let far = Duration::from_secs(4285 * 3600);
        assert_eq!(t.beat_at(far, 960), None);
        // The same position on a coarse grid is fine, so the refusal is about the grid
        // rather than the length.
        assert!(t.beat_at(far, 1).is_some());
    }

    #[test]
    fn position_of_should_treat_a_degenerate_count_as_the_start() {
        let t = tempo(174, 1);
        assert_eq!(t.position_of(beats(1, 0)), Duration::ZERO);
        assert_eq!(t.position_of(beats(-1, 1)), Duration::ZERO);
    }
}
