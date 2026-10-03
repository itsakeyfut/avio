//! [`Rational`] number type for representing fractions.

// These casts are intentional for media timestamp arithmetic.
// The values involved (PTS, time bases, frame rates) are well within
// the safe ranges for these conversions in practical video/audio scenarios.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};

/// A rational number represented as a fraction (numerator / denominator).
///
/// This type is commonly used to represent:
/// - Time bases (e.g., 1/90000 for MPEG-TS, 1/1000 for milliseconds)
/// - Frame rates (e.g., 30000/1001 for 29.97 fps)
/// - Aspect ratios (e.g., 16/9)
///
/// # Invariants
///
/// - Denominator is always positive (sign is in numerator)
/// - Zero denominator is handled gracefully (returns infinity/NaN for conversions)
///
/// # Examples
///
/// ```
/// use ff_format::Rational;
///
/// // Common time base for MPEG-TS
/// let time_base = Rational::new(1, 90000);
///
/// // 29.97 fps (NTSC)
/// let fps = Rational::new(30000, 1001);
/// assert!((fps.as_f64() - 29.97).abs() < 0.01);
///
/// // Invert to get frame duration
/// let frame_duration = fps.invert();
/// assert_eq!(frame_duration.num(), 1001);
/// assert_eq!(frame_duration.den(), 30000);
/// ```
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy)]
pub struct Rational {
    num: i32,
    den: i32,
}

impl PartialEq for Rational {
    fn eq(&self, other: &Self) -> bool {
        // a/b == c/d iff a*d == b*c (cross-multiplication)
        // Use i64 to avoid overflow
        i64::from(self.num) * i64::from(other.den) == i64::from(other.num) * i64::from(self.den)
    }
}

impl Eq for Rational {}

impl std::hash::Hash for Rational {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Hash the reduced form to ensure equal values have equal hashes
        let reduced = self.reduce();
        reduced.num.hash(state);
        reduced.den.hash(state);
    }
}

impl Rational {
    /// Creates a new rational number.
    ///
    /// The denominator is normalized to always be positive (the sign is moved
    /// to the numerator).
    ///
    /// # Panics
    ///
    /// Does not panic. A zero denominator is allowed but will result in
    /// infinity or NaN when converted to floating-point.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let r = Rational::new(1, 2);
    /// assert_eq!(r.num(), 1);
    /// assert_eq!(r.den(), 2);
    ///
    /// // Negative denominator is normalized
    /// let r = Rational::new(1, -2);
    /// assert_eq!(r.num(), -1);
    /// assert_eq!(r.den(), 2);
    /// ```
    #[must_use]
    pub const fn new(num: i32, den: i32) -> Self {
        // Normalize: denominator should always be positive
        if den < 0 {
            Self {
                num: -num,
                den: -den,
            }
        } else {
            Self { num, den }
        }
    }

    /// Creates a rational number representing zero (0/1).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let zero = Rational::zero();
    /// assert_eq!(zero.as_f64(), 0.0);
    /// assert!(zero.is_zero());
    /// ```
    #[must_use]
    pub const fn zero() -> Self {
        Self { num: 0, den: 1 }
    }

    /// Creates a rational number representing one (1/1).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let one = Rational::one();
    /// assert_eq!(one.as_f64(), 1.0);
    /// ```
    #[must_use]
    pub const fn one() -> Self {
        Self { num: 1, den: 1 }
    }

    /// Returns the numerator.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let r = Rational::new(3, 4);
    /// assert_eq!(r.num(), 3);
    /// ```
    #[must_use]
    #[inline]
    pub const fn num(&self) -> i32 {
        self.num
    }

    /// Returns the denominator.
    ///
    /// The denominator is always non-negative.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let r = Rational::new(3, 4);
    /// assert_eq!(r.den(), 4);
    /// ```
    #[must_use]
    #[inline]
    pub const fn den(&self) -> i32 {
        self.den
    }

    /// Converts the rational number to a floating-point value.
    ///
    /// Returns `f64::INFINITY`, `f64::NEG_INFINITY`, or `f64::NAN` for
    /// edge cases (division by zero).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let r = Rational::new(1, 4);
    /// assert_eq!(r.as_f64(), 0.25);
    ///
    /// let r = Rational::new(1, 3);
    /// assert!((r.as_f64() - 0.333333).abs() < 0.001);
    /// ```
    #[must_use]
    #[inline]
    pub fn as_f64(self) -> f64 {
        if self.den == 0 {
            match self.num.cmp(&0) {
                Ordering::Greater => f64::INFINITY,
                Ordering::Less => f64::NEG_INFINITY,
                Ordering::Equal => f64::NAN,
            }
        } else {
            f64::from(self.num) / f64::from(self.den)
        }
    }

    /// Converts the rational number to a single-precision floating-point value.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let r = Rational::new(1, 2);
    /// assert_eq!(r.as_f32(), 0.5);
    /// ```
    #[must_use]
    #[inline]
    pub fn as_f32(self) -> f32 {
        self.as_f64() as f32
    }

    /// Returns the inverse (reciprocal) of this rational number.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let r = Rational::new(3, 4);
    /// let inv = r.invert();
    /// assert_eq!(inv.num(), 4);
    /// assert_eq!(inv.den(), 3);
    ///
    /// // Negative values
    /// let r = Rational::new(-3, 4);
    /// let inv = r.invert();
    /// assert_eq!(inv.num(), -4);
    /// assert_eq!(inv.den(), 3);
    /// ```
    #[must_use]
    pub const fn invert(self) -> Self {
        // Handle sign normalization when inverting
        if self.num < 0 {
            Self {
                num: -self.den,
                den: -self.num,
            }
        } else {
            Self {
                num: self.den,
                den: self.num,
            }
        }
    }

    /// Returns true if this rational number is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// assert!(Rational::new(0, 1).is_zero());
    /// assert!(Rational::new(0, 100).is_zero());
    /// assert!(!Rational::new(1, 100).is_zero());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_zero(self) -> bool {
        self.num == 0
    }

    /// Returns true if this rational number is positive.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// assert!(Rational::new(1, 2).is_positive());
    /// assert!(!Rational::new(-1, 2).is_positive());
    /// assert!(!Rational::new(0, 1).is_positive());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_positive(self) -> bool {
        self.num > 0 && self.den > 0
    }

    /// Returns true if this rational number is negative.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// assert!(Rational::new(-1, 2).is_negative());
    /// assert!(!Rational::new(1, 2).is_negative());
    /// assert!(!Rational::new(0, 1).is_negative());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_negative(self) -> bool {
        self.num < 0 && self.den > 0
    }

    /// Returns the absolute value of this rational number.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// assert_eq!(Rational::new(-3, 4).abs(), Rational::new(3, 4));
    /// assert_eq!(Rational::new(3, 4).abs(), Rational::new(3, 4));
    /// ```
    #[must_use]
    pub const fn abs(self) -> Self {
        Self {
            num: if self.num < 0 { -self.num } else { self.num },
            den: self.den,
        }
    }

    /// Reduces the rational to its lowest terms using GCD.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::Rational;
    ///
    /// let r = Rational::new(4, 8);
    /// let reduced = r.reduce();
    /// assert_eq!(reduced.num(), 1);
    /// assert_eq!(reduced.den(), 2);
    /// ```
    #[must_use]
    pub fn reduce(self) -> Self {
        if self.num == 0 {
            return Self::new(0, 1);
        }
        let g = gcd(self.num.unsigned_abs(), self.den.unsigned_abs());
        Self {
            num: self.num / g as i32,
            den: self.den / g as i32,
        }
    }
}

/// Computes the greatest common divisor using Euclidean algorithm.
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let temp = b;
        b = a % b;
        a = temp;
    }
    a
}

impl Default for Rational {
    /// Returns the default rational number (1/1).
    fn default() -> Self {
        Self::one()
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

impl From<i32> for Rational {
    fn from(n: i32) -> Self {
        Self::new(n, 1)
    }
}

impl From<(i32, i32)> for Rational {
    fn from((num, den): (i32, i32)) -> Self {
        Self::new(num, den)
    }
}

// Arithmetic operations for Rational

impl Add for Rational {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        // a/b + c/d = (ad + bc) / bd
        let num =
            i64::from(self.num) * i64::from(rhs.den) + i64::from(rhs.num) * i64::from(self.den);
        let den = i64::from(self.den) * i64::from(rhs.den);

        // Try to reduce to fit in i32
        let g = gcd(num.unsigned_abs() as u32, den.unsigned_abs() as u32);
        Self::new((num / i64::from(g)) as i32, (den / i64::from(g)) as i32)
    }
}

impl Sub for Rational {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        // a/b - c/d = (ad - bc) / bd
        let num =
            i64::from(self.num) * i64::from(rhs.den) - i64::from(rhs.num) * i64::from(self.den);
        let den = i64::from(self.den) * i64::from(rhs.den);

        let g = gcd(num.unsigned_abs() as u32, den.unsigned_abs() as u32);
        Self::new((num / i64::from(g)) as i32, (den / i64::from(g)) as i32)
    }
}

impl Mul for Rational {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        // a/b * c/d = ac / bd
        let num = i64::from(self.num) * i64::from(rhs.num);
        let den = i64::from(self.den) * i64::from(rhs.den);

        let g = gcd(num.unsigned_abs() as u32, den.unsigned_abs() as u32);
        Self::new((num / i64::from(g)) as i32, (den / i64::from(g)) as i32)
    }
}

impl Div for Rational {
    type Output = Self;

    #[allow(clippy::suspicious_arithmetic_impl)]
    fn div(self, rhs: Self) -> Self::Output {
        // a/b / c/d = a/b * d/c = ad / bc
        // Using multiplication by inverse is mathematically correct for rational division
        self * rhs.invert()
    }
}

impl Mul<i32> for Rational {
    type Output = Self;

    fn mul(self, rhs: i32) -> Self::Output {
        let num = i64::from(self.num) * i64::from(rhs);
        let g = gcd(num.unsigned_abs() as u32, self.den.unsigned_abs());
        Self::new((num / i64::from(g)) as i32, self.den / g as i32)
    }
}

impl Div<i32> for Rational {
    type Output = Self;

    fn div(self, rhs: i32) -> Self::Output {
        let den = i64::from(self.den) * i64::from(rhs);
        let g = gcd(self.num.unsigned_abs(), den.unsigned_abs() as u32);
        Self::new(self.num / g as i32, (den / i64::from(g)) as i32)
    }
}

impl Neg for Rational {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(-self.num, self.den)
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare a/b with c/d by comparing ad with bc
        let left = i64::from(self.num) * i64::from(other.den);
        let right = i64::from(other.num) * i64::from(self.den);
        left.cmp(&right)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::redundant_closure_for_method_calls
)]
mod tests {
    use super::*;

    /// Helper for approximate float comparison in tests
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    mod rational_tests {
        use super::*;

        #[test]
        fn test_new() {
            let r = Rational::new(1, 2);
            assert_eq!(r.num(), 1);
            assert_eq!(r.den(), 2);
        }

        #[test]
        fn test_new_negative_denominator() {
            // Negative denominator should be normalized
            let r = Rational::new(1, -2);
            assert_eq!(r.num(), -1);
            assert_eq!(r.den(), 2);

            let r = Rational::new(-1, -2);
            assert_eq!(r.num(), 1);
            assert_eq!(r.den(), 2);
        }

        #[test]
        fn test_zero_and_one() {
            let zero = Rational::zero();
            assert!(zero.is_zero());
            assert!(approx_eq(zero.as_f64(), 0.0));

            let one = Rational::one();
            assert!(approx_eq(one.as_f64(), 1.0));
            assert!(!one.is_zero());
        }

        #[test]
        fn test_as_f64() {
            assert!(approx_eq(Rational::new(1, 2).as_f64(), 0.5));
            assert!(approx_eq(Rational::new(1, 4).as_f64(), 0.25));
            assert!((Rational::new(1, 3).as_f64() - 0.333_333).abs() < 0.001);
            assert!(approx_eq(Rational::new(-1, 2).as_f64(), -0.5));
        }

        #[test]
        fn test_as_f64_division_by_zero() {
            assert!(Rational::new(1, 0).as_f64().is_infinite());
            assert!(Rational::new(1, 0).as_f64().is_sign_positive());
            assert!(Rational::new(-1, 0).as_f64().is_infinite());
            assert!(Rational::new(-1, 0).as_f64().is_sign_negative());
            assert!(Rational::new(0, 0).as_f64().is_nan());
        }

        #[test]
        fn test_as_f32() {
            assert_eq!(Rational::new(1, 2).as_f32(), 0.5);
        }

        #[test]
        fn test_invert() {
            let r = Rational::new(3, 4);
            let inv = r.invert();
            assert_eq!(inv.num(), 4);
            assert_eq!(inv.den(), 3);

            // Negative value
            let r = Rational::new(-3, 4);
            let inv = r.invert();
            assert_eq!(inv.num(), -4);
            assert_eq!(inv.den(), 3);
        }

        #[test]
        fn test_is_positive_negative() {
            assert!(Rational::new(1, 2).is_positive());
            assert!(!Rational::new(-1, 2).is_positive());
            assert!(!Rational::new(0, 1).is_positive());

            assert!(Rational::new(-1, 2).is_negative());
            assert!(!Rational::new(1, 2).is_negative());
            assert!(!Rational::new(0, 1).is_negative());
        }

        #[test]
        fn test_abs() {
            assert_eq!(Rational::new(-3, 4).abs(), Rational::new(3, 4));
            assert_eq!(Rational::new(3, 4).abs(), Rational::new(3, 4));
            assert_eq!(Rational::new(0, 4).abs(), Rational::new(0, 4));
        }

        #[test]
        fn test_reduce() {
            let r = Rational::new(4, 8);
            let reduced = r.reduce();
            assert_eq!(reduced.num(), 1);
            assert_eq!(reduced.den(), 2);

            let r = Rational::new(6, 9);
            let reduced = r.reduce();
            assert_eq!(reduced.num(), 2);
            assert_eq!(reduced.den(), 3);

            let r = Rational::new(0, 5);
            let reduced = r.reduce();
            assert_eq!(reduced.num(), 0);
            assert_eq!(reduced.den(), 1);
        }

        #[test]
        fn test_add() {
            let a = Rational::new(1, 2);
            let b = Rational::new(1, 4);
            let result = a + b;
            assert!((result.as_f64() - 0.75).abs() < 0.0001);
        }

        #[test]
        fn test_sub() {
            let a = Rational::new(1, 2);
            let b = Rational::new(1, 4);
            let result = a - b;
            assert!((result.as_f64() - 0.25).abs() < 0.0001);
        }

        #[test]
        fn test_mul() {
            let a = Rational::new(1, 2);
            let b = Rational::new(2, 3);
            let result = a * b;
            assert!((result.as_f64() - (1.0 / 3.0)).abs() < 0.0001);
        }

        #[test]
        fn test_div() {
            let a = Rational::new(1, 2);
            let b = Rational::new(2, 3);
            let result = a / b;
            assert!((result.as_f64() - 0.75).abs() < 0.0001);
        }

        #[test]
        fn test_mul_i32() {
            let r = Rational::new(1, 4);
            let result = r * 2;
            assert!((result.as_f64() - 0.5).abs() < 0.0001);
        }

        #[test]
        fn test_div_i32() {
            let r = Rational::new(1, 2);
            let result = r / 2;
            assert!((result.as_f64() - 0.25).abs() < 0.0001);
        }

        #[test]
        fn test_neg() {
            let r = Rational::new(1, 2);
            let neg = -r;
            assert_eq!(neg.num(), -1);
            assert_eq!(neg.den(), 2);
        }

        #[test]
        fn test_ord() {
            let a = Rational::new(1, 2);
            let b = Rational::new(1, 3);
            let c = Rational::new(2, 4);

            assert!(a > b);
            assert!(b < a);
            assert_eq!(a, c);
            assert!(a >= c);
            assert!(a <= c);
        }

        #[test]
        fn test_from_i32() {
            let r: Rational = 5.into();
            assert_eq!(r.num(), 5);
            assert_eq!(r.den(), 1);
        }

        #[test]
        fn test_from_tuple() {
            let r: Rational = (3, 4).into();
            assert_eq!(r.num(), 3);
            assert_eq!(r.den(), 4);
        }

        #[test]
        fn test_display() {
            assert_eq!(format!("{}", Rational::new(1, 2)), "1/2");
            assert_eq!(format!("{}", Rational::new(-3, 4)), "-3/4");
        }

        #[test]
        fn test_default() {
            assert_eq!(Rational::default(), Rational::one());
        }

        #[test]
        fn test_common_frame_rates() {
            // 23.976 fps (film)
            let fps = Rational::new(24000, 1001);
            assert!((fps.as_f64() - 23.976).abs() < 0.001);

            // 29.97 fps (NTSC)
            let fps = Rational::new(30000, 1001);
            assert!((fps.as_f64() - 29.97).abs() < 0.01);

            // 59.94 fps (NTSC interlaced as progressive)
            let fps = Rational::new(60000, 1001);
            assert!((fps.as_f64() - 59.94).abs() < 0.01);
        }
    }

    // ==================== GCD Tests ====================

    #[test]
    fn test_gcd() {
        assert_eq!(gcd(12, 8), 4);
        assert_eq!(gcd(17, 13), 1);
        assert_eq!(gcd(100, 25), 25);
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(gcd(5, 0), 5);
    }
}
