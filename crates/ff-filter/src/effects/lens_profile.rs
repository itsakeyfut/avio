//! Predefined lens distortion correction profiles for common cameras.

/// Predefined lens distortion correction profiles for common cameras.
///
/// Each variant stores the radial coefficients (`k1`, `k2`) and a `scale`
/// factor that zooms slightly to hide the warped border pixels left after
/// correction.
///
/// Use with [`FilterGraph::lens_profile`](crate::FilterGraph::lens_profile).
// Open catalog: more camera/lens profiles are added over time.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LensProfile {
    /// `GoPro` Hero 9 / 10 / 11 wide-angle mode (heavy barrel distortion).
    GoproHero9Wide,
    /// `GoPro` Hero 11 linear mode (mild distortion).
    GoproHero11Linear,
    /// Apple iPhone 14 Pro main camera (mild barrel).
    Iphone14ProMain,
    /// DJI Mini 3 Pro wide-angle lens.
    DjiMini3ProWide,
    /// User-supplied coefficients for any other camera.
    Custom {
        /// First-order radial coefficient (−1.0 to 1.0).
        k1: f32,
        /// Second-order radial coefficient (−1.0 to 1.0).
        k2: f32,
        /// Uniform scale applied after correction to hide warped border pixels.
        /// 1.0 = no scale.
        scale: f32,
    },
}

impl LensProfile {
    /// Return `(k1, k2, scale)` for this profile.
    pub fn coefficients(&self) -> (f32, f32, f32) {
        match self {
            Self::GoproHero9Wide => (-0.21, 0.05, 1.05),
            Self::GoproHero11Linear => (-0.04, 0.01, 1.01),
            Self::Iphone14ProMain => (-0.03, 0.00, 1.01),
            Self::DjiMini3ProWide => (-0.16, 0.03, 1.03),
            Self::Custom { k1, k2, scale } => (*k1, *k2, *scale),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gopro_hero9_wide_should_have_expected_coefficients() {
        let (k1, k2, scale) = LensProfile::GoproHero9Wide.coefficients();
        assert!((k1 - (-0.21_f32)).abs() < f32::EPSILON);
        assert!((k2 - 0.05_f32).abs() < f32::EPSILON);
        assert!((scale - 1.05_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn iphone14_pro_main_should_have_expected_coefficients() {
        let (k1, k2, scale) = LensProfile::Iphone14ProMain.coefficients();
        assert!((k1 - (-0.03_f32)).abs() < f32::EPSILON);
        assert!((k2 - 0.00_f32).abs() < f32::EPSILON);
        assert!((scale - 1.01_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn custom_should_return_supplied_values() {
        let (k1, k2, scale) = LensProfile::Custom {
            k1: -0.1,
            k2: 0.02,
            scale: 1.02,
        }
        .coefficients();
        assert!((k1 - (-0.1_f32)).abs() < f32::EPSILON);
        assert!((k2 - 0.02_f32).abs() < f32::EPSILON);
        assert!((scale - 1.02_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn custom_identity_should_return_zero_k1_k2_and_unit_scale() {
        let (k1, k2, scale) = LensProfile::Custom {
            k1: 0.0,
            k2: 0.0,
            scale: 1.0,
        }
        .coefficients();
        assert!((k1 - 0.0_f32).abs() < f32::EPSILON);
        assert!((k2 - 0.0_f32).abs() < f32::EPSILON);
        assert!((scale - 1.0_f32).abs() < f32::EPSILON);
    }
}
