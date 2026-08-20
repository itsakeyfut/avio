//! H.265 (HEVC) per-codec encoding options.

/// H.265 (HEVC) per-codec options.
#[derive(Debug, Clone)]
pub struct H265Options {
    /// Encoding profile.
    pub profile: H265Profile,
    /// Encoding tier.
    pub tier: H265Tier,
    /// Encoding level as an integer (e.g. `31` = 3.1, `51` = 5.1).
    ///
    /// `None` leaves the encoder default.
    pub level: Option<u32>,
    /// libx265 encoding speed/quality preset (e.g. `"ultrafast"`, `"medium"`, `"slow"`).
    ///
    /// `None` leaves the encoder default. Invalid or unsupported values are logged as a
    /// warning and skipped — `build()` never fails due to an unsupported preset.
    /// Hardware HEVC encoders (`hevc_nvenc`, etc.) ignore this option.
    pub preset: Option<String>,
    /// Raw x265-params string passed verbatim to libx265 (e.g. `"ctu=32:ref=4"`).
    ///
    /// **Note**: H.265 encoding requires an `FFmpeg` build with `--enable-libx265`.
    ///
    /// An invalid parameter string is logged as a warning and skipped. It never causes
    /// `build()` to return an error.
    pub x265_params: Option<String>,
}

impl Default for H265Options {
    fn default() -> Self {
        Self {
            profile: H265Profile::Main,
            tier: H265Tier::Main,
            level: None,
            preset: None,
            x265_params: None,
        }
    }
}

/// H.265 encoding profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum H265Profile {
    /// Main profile (8-bit, 4:2:0).
    #[default]
    Main,
    /// Main 10-bit profile (HDR-capable).
    Main10,
}

impl H265Profile {
    pub(in crate::video) fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Main10 => "main10",
        }
    }
}

/// H.265 encoding tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum H265Tier {
    /// Main tier — lower bitrate ceiling (consumer content).
    #[default]
    Main,
    /// High tier — higher bitrate ceiling (broadcast / professional).
    High,
}

impl H265Tier {
    pub(in crate::video) fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::High => "high",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h265_profile_should_return_correct_str() {
        assert_eq!(H265Profile::Main.as_str(), "main");
        assert_eq!(H265Profile::Main10.as_str(), "main10");
    }

    #[test]
    fn h265_tier_should_return_correct_str() {
        assert_eq!(H265Tier::Main.as_str(), "main");
        assert_eq!(H265Tier::High.as_str(), "high");
    }

    #[test]
    fn h265_options_default_should_have_main_profile() {
        let opts = H265Options::default();
        assert_eq!(opts.profile, H265Profile::Main);
        assert_eq!(opts.tier, H265Tier::Main);
        assert_eq!(opts.level, None);
        assert!(opts.preset.is_none());
        assert!(opts.x265_params.is_none());
    }
}
