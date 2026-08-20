//! Apple `ProRes` per-codec encoding options.

/// Apple `ProRes` encoding profile.
///
/// Controls quality and chroma sampling. 422 profiles use `yuv422p10le`;
/// 4444 profiles use `yuva444p10le`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProResProfile {
    /// 422 Proxy — lowest data rate, offline editing.
    Proxy,
    /// 422 LT — lightweight, good for editing proxies.
    Lt,
    /// 422 Standard — production quality (default).
    #[default]
    Standard,
    /// 422 HQ — high quality, recommended for mastering.
    Hq,
    /// 4444 — full chroma, supports alpha channel.
    P4444,
    /// 4444 XQ — maximum quality 4444 variant.
    P4444Xq,
}

impl ProResProfile {
    /// Returns the integer profile ID passed to `prores_ks` via `av_opt_set`.
    pub(in crate::video) fn profile_id(self) -> u8 {
        match self {
            Self::Proxy => 0,
            Self::Lt => 1,
            Self::Standard => 2,
            Self::Hq => 3,
            Self::P4444 => 4,
            Self::P4444Xq => 5,
        }
    }

    /// Returns `true` for 4444 profiles that require `yuva444p10le` pixel format.
    pub(in crate::video) fn is_4444(self) -> bool {
        matches!(self, Self::P4444 | Self::P4444Xq)
    }
}

/// Apple `ProRes` per-codec options.
///
/// Requires an `FFmpeg` build with `prores_ks` encoder support. Output should
/// use a `.mov` container.
#[derive(Debug, Clone)]
pub struct ProResOptions {
    /// `ProRes` encoding profile controlling quality and chroma sampling.
    pub profile: ProResProfile,
    /// Optional 4-byte `FourCC` vendor tag embedded in the stream.
    ///
    /// Set to `Some([b'a', b'p', b'p', b'l'])` to mimic Apple encoders.
    /// `None` leaves the encoder default.
    pub vendor: Option<[u8; 4]>,
}

impl Default for ProResOptions {
    fn default() -> Self {
        Self {
            profile: ProResProfile::Standard,
            vendor: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prores_options_default_should_have_standard_profile() {
        let opts = ProResOptions::default();
        assert_eq!(opts.profile, ProResProfile::Standard);
        assert!(opts.vendor.is_none());
    }

    #[test]
    fn prores_profile_ids_should_match_spec() {
        assert_eq!(ProResProfile::Proxy.profile_id(), 0);
        assert_eq!(ProResProfile::Lt.profile_id(), 1);
        assert_eq!(ProResProfile::Standard.profile_id(), 2);
        assert_eq!(ProResProfile::Hq.profile_id(), 3);
        assert_eq!(ProResProfile::P4444.profile_id(), 4);
        assert_eq!(ProResProfile::P4444Xq.profile_id(), 5);
    }

    #[test]
    fn prores_profile_is_4444_should_return_true_for_4444_variants() {
        assert!(!ProResProfile::Proxy.is_4444());
        assert!(!ProResProfile::Lt.is_4444());
        assert!(!ProResProfile::Standard.is_4444());
        assert!(!ProResProfile::Hq.is_4444());
        assert!(ProResProfile::P4444.is_4444());
        assert!(ProResProfile::P4444Xq.is_4444());
    }
}
