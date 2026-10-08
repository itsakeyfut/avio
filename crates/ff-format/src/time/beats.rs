//! A musical position, counted in beats.

use crate::time::Rational;

/// A count of beats, as an exact fraction.
///
/// # Why a fraction
///
/// Music subdivides a beat by whatever it needs: halves and quarters, thirds for triplets,
/// `3/2` for a dotted value, sevenths where the material asks for them. A fraction holds
/// every one of those exactly, and a sum of them stays exact because rational addition
/// does. A tick grid at a fixed resolution does not: 960 ticks to the beat cannot hold a
/// septuplet, and the error accumulates rather than cancelling.
///
/// # What is representable
///
/// The count is an [`Rational`], so a numerator and denominator each bounded by `i32`.
/// Measured against the subdivisions that occur in practice, that is a great deal of
/// material: accumulating 5948 subdivisions drawn from halves, thirds, quarters, fifths,
/// sixths, sevenths, eighths, twelfths and sixteenths across 1740 beats reached a reduced
/// numerator of 2,920,397 and a denominator of 1680, which is 0.14% of `i32`. For the
/// subdivisions a piece actually mixes (binary, triplets, dotted, whose denominators have
/// lowest common multiple 48) the range is **44.7 million beats, about 4285 hours at
/// 174 BPM**.
///
/// It is not unbounded, though. A position mixing *every* divisor from 1 to 16 has
/// denominator 720720, and `i32` then runs out at **2979 beats, about 17 minutes at
/// 174 BPM**, which is inside the length of the material this is for.
///
/// # Why the arithmetic is checked
///
/// Because of that bound, and because of how `Rational` reaches it. `Rational`'s `Add`
/// finishes with `(num / g) as i32`, and a Rust `i64 as i32` **wraps silently**:
/// 3,000,000,000 becomes -1,294,967,296. A beat position that overflowed through the
/// operator would be *negative*, placing a clip before the start of the timeline, with
/// nothing reported. So [`checked_add`](Self::checked_add) and
/// [`checked_sub`](Self::checked_sub) are the only arithmetic here, and `Add`/`Sub` are
/// deliberately not implemented: an operator has nowhere to put the refusal.
///
/// # Examples
///
/// ```
/// use ff_format::{Beats, Rational};
///
/// // A triplet: three of them make exactly one beat.
/// let third = Beats::new(Rational::new(1, 3));
/// let two_thirds = third.checked_add(third).unwrap();
/// let whole = two_thirds.checked_add(third).unwrap();
/// assert_eq!(whole.count(), Rational::new(1, 1));
/// ```
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Beats(Rational);

impl Beats {
    /// A position at `count` beats from the start.
    #[must_use]
    pub const fn new(count: Rational) -> Self {
        Self(count)
    }

    /// The start.
    #[must_use]
    pub const fn zero() -> Self {
        Self(Rational::zero())
    }

    /// The count, as the fraction it is.
    #[must_use]
    pub const fn count(self) -> Rational {
        self.0
    }

    /// `self + rhs`, or `None` when the sum is not representable.
    ///
    /// `None` rather than a wrapped value: see the type's documentation for why that
    /// matters here specifically.
    #[must_use]
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.combine(rhs, i64::checked_add)
    }

    /// `self - rhs`, or `None` when the difference is not representable.
    ///
    /// A negative result is representable and returned: a beat count before the start is a
    /// meaningful intermediate, and refusing it here would make subtraction useless for
    /// computing a span.
    #[must_use]
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.combine(rhs, i64::checked_sub)
    }

    /// The shared body of the checked operations.
    ///
    /// Cross-multiplies in `i64`, reduces, and refuses anything `i32` cannot hold. The
    /// `i64` intermediate cannot itself overflow: both products are at most
    /// `i32::MAX * i32::MAX`, about 4.6e18, against `i64`'s 9.2e18.
    fn combine(self, rhs: Self, op: fn(i64, i64) -> Option<i64>) -> Option<Self> {
        let (an, ad) = (i64::from(self.0.num()), i64::from(self.0.den()));
        let (bn, bd) = (i64::from(rhs.0.num()), i64::from(rhs.0.den()));
        if ad == 0 || bd == 0 {
            return None;
        }

        let num = op(an.checked_mul(bd)?, bn.checked_mul(ad)?)?;
        let den = ad.checked_mul(bd)?;

        let g = gcd(num.unsigned_abs(), den.unsigned_abs());
        let g = i64::try_from(g).ok()?;
        let (num, den) = if g == 0 {
            (num, den)
        } else {
            (num / g, den / g)
        };

        Some(Self(Rational::new(
            i32::try_from(num).ok()?,
            i32::try_from(den).ok()?,
        )))
    }
}

/// Greatest common divisor, for reducing before the `i32` check.
///
/// Reducing first is what makes the common case representable: the sum of two sixteenths
/// has denominator 256 before reduction and 8 after.
const fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{Beats, gcd};
    use crate::time::Rational;

    fn beats(num: i32, den: i32) -> Beats {
        Beats::new(Rational::new(num, den))
    }

    #[test]
    fn gcd_should_reduce_to_the_common_factor() {
        assert_eq!(gcd(256, 8), 8);
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(gcd(5, 0), 5);
    }

    /// Every subdivision this material uses, exactly, including a sum that mixes them.
    #[test]
    fn beats_should_add_every_subdivision_exactly() {
        // Three triplets make one beat, with nothing left over.
        let third = beats(1, 3);
        let whole = third
            .checked_add(third)
            .unwrap()
            .checked_add(third)
            .unwrap();
        assert_eq!(whole.count(), Rational::new(1, 1));

        // A dotted half is three quarters.
        let dotted = beats(1, 2).checked_add(beats(1, 4)).unwrap();
        assert_eq!(dotted.count(), Rational::new(3, 4));

        // Mixed denominators: 1/2 + 1/3 + 1/7 = (21 + 14 + 6) / 42 = 41/42.
        let mixed = beats(1, 2)
            .checked_add(beats(1, 3))
            .unwrap()
            .checked_add(beats(1, 7))
            .unwrap();
        assert_eq!(mixed.count(), Rational::new(41, 42));

        // And the result is reduced, not merely equal: `Rational`'s `PartialEq`
        // cross-multiplies, so comparing values alone would not notice an unreduced
        // denominator growing without bound.
        assert_eq!(mixed.count().num(), 41);
        assert_eq!(mixed.count().den(), 42);
    }

    /// Accumulating over a realistic length stays far inside the bound.
    #[test]
    fn beats_should_accumulate_over_a_realistic_piece() {
        // Ten minutes at 174 BPM is 1740 beats. Walk it in sixteenth-note triplets,
        // which is the finest grid this material ordinarily uses.
        let step = beats(1, 12);
        let mut pos = Beats::zero();
        for _ in 0..(1740 * 12) {
            pos = pos
                .checked_add(step)
                .expect("a realistic piece is representable");
        }
        assert_eq!(pos.count(), Rational::new(1740, 1));
    }

    /// The bound is refused rather than wrapped. Without this the sum would come back
    /// negative and place a clip before the start of the timeline.
    #[test]
    fn beats_checked_add_should_refuse_a_sum_i32_cannot_hold() {
        // Denominators whose product cannot reduce: two large coprime primes.
        let a = beats(1, 2_147_483_647);
        let b = beats(1, 2_147_483_629);
        assert_eq!(
            a.checked_add(b),
            None,
            "the common denominator is about 4.6e18, which no i32 holds"
        );

        // And a numerator that overflows on its own.
        let big = beats(i32::MAX, 1);
        assert_eq!(big.checked_add(big), None);
    }

    #[test]
    fn beats_checked_sub_should_allow_a_negative_result() {
        let back = beats(1, 4).checked_sub(beats(1, 2)).unwrap();
        assert_eq!(back.count(), Rational::new(-1, 4));
    }

    #[test]
    fn beats_should_refuse_arithmetic_on_a_degenerate_count() {
        // `Rational::new` accepts a zero denominator, so a `Beats` can hold one. It is not
        // a position, and arithmetic on it is refused rather than producing one.
        let degenerate = beats(1, 0);
        assert_eq!(degenerate.checked_add(beats(1, 4)), None);
        assert_eq!(beats(1, 4).checked_add(degenerate), None);
    }

    #[test]
    fn zero_should_be_the_additive_identity() {
        let p = beats(7, 2);
        assert_eq!(p.checked_add(Beats::zero()).unwrap().count(), p.count());
    }
}
