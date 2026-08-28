/// A value that supports linear component-wise interpolation.
///
/// `t = 0.0` returns a clone of `a`; `t = 1.0` returns a clone of `b`.
///
/// Implementations for `(f64, f64)` and `(f64, f64, f64)` are added
/// in issue #351.
pub trait Lerp: Clone {
    /// Linearly interpolates between `a` and `b` by the factor `t`.
    fn lerp(a: &Self, b: &Self, t: f64) -> Self;
}

impl Lerp for f64 {
    fn lerp(a: &Self, b: &Self, t: f64) -> Self {
        a + (b - a) * t
    }
}

impl Lerp for (f64, f64) {
    fn lerp(a: &Self, b: &Self, t: f64) -> Self {
        (f64::lerp(&a.0, &b.0, t), f64::lerp(&a.1, &b.1, t))
    }
}

impl Lerp for (f64, f64, f64) {
    fn lerp(a: &Self, b: &Self, t: f64) -> Self {
        (
            f64::lerp(&a.0, &b.0, t),
            f64::lerp(&a.1, &b.1, t),
            f64::lerp(&a.2, &b.2, t),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_tuple2d_should_interpolate_both_components() {
        let a = (0.0_f64, 10.0_f64);
        let b = (100.0_f64, 50.0_f64);

        let mid = <(f64, f64)>::lerp(&a, &b, 0.5);
        assert!(
            (mid.0 - 50.0).abs() < f64::EPSILON,
            "x: expected 50.0, got {}",
            mid.0
        );
        assert!(
            (mid.1 - 30.0).abs() < f64::EPSILON,
            "y: expected 30.0, got {}",
            mid.1
        );

        let start = <(f64, f64)>::lerp(&a, &b, 0.0);
        assert!((start.0 - 0.0).abs() < f64::EPSILON);
        assert!((start.1 - 10.0).abs() < f64::EPSILON);

        let end = <(f64, f64)>::lerp(&a, &b, 1.0);
        assert!((end.0 - 100.0).abs() < f64::EPSILON);
        assert!((end.1 - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn lerp_tuple3d_should_interpolate_all_components() {
        let a = (0.0_f64, 0.0_f64, 0.0_f64);
        let b = (255.0_f64, 128.0_f64, 64.0_f64);

        let mid = <(f64, f64, f64)>::lerp(&a, &b, 0.5);
        assert!(
            (mid.0 - 127.5).abs() < f64::EPSILON,
            "r: expected 127.5, got {}",
            mid.0
        );
        assert!(
            (mid.1 - 64.0).abs() < f64::EPSILON,
            "g: expected 64.0, got {}",
            mid.1
        );
        assert!(
            (mid.2 - 32.0).abs() < f64::EPSILON,
            "b: expected 32.0, got {}",
            mid.2
        );

        let start = <(f64, f64, f64)>::lerp(&a, &b, 0.0);
        assert!((start.0).abs() < f64::EPSILON);
        assert!((start.1).abs() < f64::EPSILON);
        assert!((start.2).abs() < f64::EPSILON);

        let end = <(f64, f64, f64)>::lerp(&a, &b, 1.0);
        assert!((end.0 - 255.0).abs() < f64::EPSILON);
        assert!((end.1 - 128.0).abs() < f64::EPSILON);
        assert!((end.2 - 64.0).abs() < f64::EPSILON);
    }
}
