//! Avid `DNxHD` / `DNxHR` per-codec encoding options.

/// `DNxHD` / `DNxHR` encoding variant.
///
/// Legacy `DNxHD` variants (`Dnxhd*`) are constrained to 1920×1080 or 1280×720
/// and require a fixed bitrate. `DNxHR` variants (`Dnxhr*`) work at any resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DnxhdVariant {
    // ── DNxHD (legacy fixed-bitrate, 1920×1080 or 1280×720 only) ─────────────
    /// 1080i/p 115 Mbps, 8-bit yuv422p.
    Dnxhd115,
    /// 1080i/p 145 Mbps, 8-bit yuv422p.
    Dnxhd145,
    /// 1080p 220 Mbps, 8-bit yuv422p.
    Dnxhd220,
    /// 1080p 220 Mbps, 10-bit yuv422p10le.
    Dnxhd220x,
    // ── DNxHR (resolution-agnostic) ───────────────────────────────────────────
    /// Low Bandwidth, 8-bit yuv422p.
    DnxhrLb,
    /// Standard Quality, 8-bit yuv422p (default).
    #[default]
    DnxhrSq,
    /// High Quality, 8-bit yuv422p.
    DnxhrHq,
    /// High Quality 10-bit, yuv422p10le.
    DnxhrHqx,
    /// 4:4:4 12-bit, yuv444p10le.
    DnxhrR444,
}

impl DnxhdVariant {
    /// Returns the `vprofile` string passed to the `dnxhd` encoder.
    pub(in crate::video) fn vprofile_str(self) -> &'static str {
        match self {
            Self::Dnxhd115 | Self::Dnxhd145 | Self::Dnxhd220 | Self::Dnxhd220x => "dnxhd",
            Self::DnxhrLb => "dnxhr_lb",
            Self::DnxhrSq => "dnxhr_sq",
            Self::DnxhrHq => "dnxhr_hq",
            Self::DnxhrHqx => "dnxhr_hqx",
            Self::DnxhrR444 => "dnxhr_444",
        }
    }

    /// Returns the required pixel format for this variant.
    pub(in crate::video) fn pixel_format(self) -> ff_format::PixelFormat {
        use ff_format::PixelFormat;
        match self {
            Self::Dnxhd115
            | Self::Dnxhd145
            | Self::Dnxhd220
            | Self::DnxhrLb
            | Self::DnxhrSq
            | Self::DnxhrHq => PixelFormat::Yuv422p,
            Self::Dnxhd220x | Self::DnxhrHqx => PixelFormat::Yuv422p10le,
            Self::DnxhrR444 => PixelFormat::Yuv444p10le,
        }
    }

    /// For legacy `DNxHD` variants, returns the required fixed bitrate in bps.
    ///
    /// `DNxHR` variants return `None` — the encoder selects the bitrate automatically.
    pub(in crate::video) fn fixed_bitrate_bps(self) -> Option<i64> {
        match self {
            Self::Dnxhd115 => Some(115_000_000),
            Self::Dnxhd145 => Some(145_000_000),
            Self::Dnxhd220 | Self::Dnxhd220x => Some(220_000_000),
            _ => None,
        }
    }

    /// Returns `true` for legacy `DNxHD` variants that require 1920×1080 or 1280×720.
    pub(in crate::video) fn is_dnxhd(self) -> bool {
        matches!(
            self,
            Self::Dnxhd115 | Self::Dnxhd145 | Self::Dnxhd220 | Self::Dnxhd220x
        )
    }
}

/// Avid `DNxHD` / `DNxHR` per-codec options.
///
/// Output should use a `.mxf` or `.mov` container. Legacy `DNxHD` variants
/// (`Dnxhd*`) are validated in `build()` to require 1920×1080 or 1280×720.
#[derive(Debug, Clone, Default)]
pub struct DnxhdOptions {
    /// DNxHD/DNxHR encoding variant.
    pub variant: DnxhdVariant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dnxhd_options_default_should_have_dnxhr_sq_variant() {
        let opts = DnxhdOptions::default();
        assert_eq!(opts.variant, DnxhdVariant::DnxhrSq);
    }

    #[test]
    fn dnxhd_variant_vprofile_str_should_match_spec() {
        assert_eq!(DnxhdVariant::Dnxhd115.vprofile_str(), "dnxhd");
        assert_eq!(DnxhdVariant::Dnxhd145.vprofile_str(), "dnxhd");
        assert_eq!(DnxhdVariant::Dnxhd220.vprofile_str(), "dnxhd");
        assert_eq!(DnxhdVariant::Dnxhd220x.vprofile_str(), "dnxhd");
        assert_eq!(DnxhdVariant::DnxhrLb.vprofile_str(), "dnxhr_lb");
        assert_eq!(DnxhdVariant::DnxhrSq.vprofile_str(), "dnxhr_sq");
        assert_eq!(DnxhdVariant::DnxhrHq.vprofile_str(), "dnxhr_hq");
        assert_eq!(DnxhdVariant::DnxhrHqx.vprofile_str(), "dnxhr_hqx");
        assert_eq!(DnxhdVariant::DnxhrR444.vprofile_str(), "dnxhr_444");
    }

    #[test]
    fn dnxhd_variant_pixel_format_should_match_spec() {
        use ff_format::PixelFormat;
        assert_eq!(DnxhdVariant::Dnxhd115.pixel_format(), PixelFormat::Yuv422p);
        assert_eq!(DnxhdVariant::Dnxhd145.pixel_format(), PixelFormat::Yuv422p);
        assert_eq!(DnxhdVariant::Dnxhd220.pixel_format(), PixelFormat::Yuv422p);
        assert_eq!(
            DnxhdVariant::Dnxhd220x.pixel_format(),
            PixelFormat::Yuv422p10le
        );
        assert_eq!(DnxhdVariant::DnxhrLb.pixel_format(), PixelFormat::Yuv422p);
        assert_eq!(DnxhdVariant::DnxhrSq.pixel_format(), PixelFormat::Yuv422p);
        assert_eq!(DnxhdVariant::DnxhrHq.pixel_format(), PixelFormat::Yuv422p);
        assert_eq!(
            DnxhdVariant::DnxhrHqx.pixel_format(),
            PixelFormat::Yuv422p10le
        );
        assert_eq!(
            DnxhdVariant::DnxhrR444.pixel_format(),
            PixelFormat::Yuv444p10le
        );
    }

    #[test]
    fn dnxhd_variant_fixed_bitrate_should_return_none_for_dnxhr() {
        assert_eq!(
            DnxhdVariant::Dnxhd115.fixed_bitrate_bps(),
            Some(115_000_000)
        );
        assert_eq!(
            DnxhdVariant::Dnxhd145.fixed_bitrate_bps(),
            Some(145_000_000)
        );
        assert_eq!(
            DnxhdVariant::Dnxhd220.fixed_bitrate_bps(),
            Some(220_000_000)
        );
        assert_eq!(
            DnxhdVariant::Dnxhd220x.fixed_bitrate_bps(),
            Some(220_000_000)
        );
        assert!(DnxhdVariant::DnxhrLb.fixed_bitrate_bps().is_none());
        assert!(DnxhdVariant::DnxhrSq.fixed_bitrate_bps().is_none());
        assert!(DnxhdVariant::DnxhrHq.fixed_bitrate_bps().is_none());
        assert!(DnxhdVariant::DnxhrHqx.fixed_bitrate_bps().is_none());
        assert!(DnxhdVariant::DnxhrR444.fixed_bitrate_bps().is_none());
    }

    #[test]
    fn dnxhd_variant_is_dnxhd_should_return_true_only_for_legacy_variants() {
        assert!(DnxhdVariant::Dnxhd115.is_dnxhd());
        assert!(DnxhdVariant::Dnxhd145.is_dnxhd());
        assert!(DnxhdVariant::Dnxhd220.is_dnxhd());
        assert!(DnxhdVariant::Dnxhd220x.is_dnxhd());
        assert!(!DnxhdVariant::DnxhrLb.is_dnxhd());
        assert!(!DnxhdVariant::DnxhrSq.is_dnxhd());
        assert!(!DnxhdVariant::DnxhrHq.is_dnxhd());
        assert!(!DnxhdVariant::DnxhrHqx.is_dnxhd());
        assert!(!DnxhdVariant::DnxhrR444.is_dnxhd());
    }
}
