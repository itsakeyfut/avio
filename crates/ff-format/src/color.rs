//! Color space and related type definitions.
//!
//! This module provides enums for color-related metadata commonly found
//! in video streams, including color space, color range, and color primaries.
//!
//! # Examples
//!
//! ```
//! use ff_format::color::{ColorSpace, ColorRange, ColorPrimaries};
//!
//! // HD video typically uses BT.709
//! let space = ColorSpace::Bt709;
//! let range = ColorRange::Limited;
//! let primaries = ColorPrimaries::Bt709;
//!
//! assert!(space.is_hd());
//! assert!(!range.is_full());
//! ```

use std::fmt;

/// Color space (matrix coefficients) for YUV to RGB conversion.
///
/// The color space defines how YUV values are converted to RGB and vice versa.
/// Different standards use different matrix coefficients for this conversion.
///
/// # Common Usage
///
/// - **BT.709**: HD content (720p, 1080p)
/// - **BT.470BG / SMPTE-170M**: SD content (576i PAL / 480i NTSC)
/// - **BT.2020 NCL / CL**: UHD/HDR content (4K, 8K)
/// - **RGB**: Identity matrix for RGB/GBR content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorSpace {
    /// ITU-R BT.709 — HD television matrix (most common for HD video)
    #[default]
    Bt709,
    /// ITU-R BT.470BG — BT.601 625-line (PAL/SECAM SD) matrix
    Bt470bg,
    /// SMPTE 170M — BT.601 525-line (NTSC SD) matrix
    Smpte170m,
    /// ITU-R BT.2020 non-constant luminance — UHD/HDR matrix
    Bt2020Ncl,
    /// ITU-R BT.2020 constant luminance — UHD/HDR matrix
    Bt2020Cl,
    /// Identity / RGB (GBR planar) — no YUV matrix
    Rgb,
    /// FCC — legacy NTSC 1953 matrix
    Fcc,
    /// SMPTE 240M — legacy HD matrix
    Smpte240m,
    /// `YCgCo` — reversible `YCgCo` matrix
    Ycgco,
    /// Color space matrix is not specified or unknown
    Unknown,
}

impl ColorSpace {
    /// Returns the name of the color space as a human-readable string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorSpace;
    ///
    /// assert_eq!(ColorSpace::Bt709.name(), "bt709");
    /// assert_eq!(ColorSpace::Bt2020Ncl.name(), "bt2020ncl");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Bt709 => "bt709",
            Self::Bt470bg => "bt470bg",
            Self::Smpte170m => "smpte170m",
            Self::Bt2020Ncl => "bt2020ncl",
            Self::Bt2020Cl => "bt2020cl",
            Self::Rgb => "rgb",
            Self::Fcc => "fcc",
            Self::Smpte240m => "smpte240m",
            Self::Ycgco => "ycgco",
            Self::Unknown => "unknown",
        }
    }

    /// Returns `true` if this is the HD matrix (BT.709).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorSpace;
    ///
    /// assert!(ColorSpace::Bt709.is_hd());
    /// assert!(!ColorSpace::Smpte170m.is_hd());
    /// ```
    #[must_use]
    pub const fn is_hd(&self) -> bool {
        matches!(self, Self::Bt709)
    }

    /// Returns `true` if this is an SD matrix (BT.601: BT.470BG or SMPTE-170M).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorSpace;
    ///
    /// assert!(ColorSpace::Smpte170m.is_sd());
    /// assert!(!ColorSpace::Bt709.is_sd());
    /// ```
    #[must_use]
    pub const fn is_sd(&self) -> bool {
        matches!(self, Self::Bt470bg | Self::Smpte170m)
    }

    /// Returns `true` if this is a UHD/HDR matrix (BT.2020 NCL or CL).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorSpace;
    ///
    /// assert!(ColorSpace::Bt2020Ncl.is_uhd());
    /// assert!(!ColorSpace::Bt709.is_uhd());
    /// ```
    #[must_use]
    pub const fn is_uhd(&self) -> bool {
        matches!(self, Self::Bt2020Ncl | Self::Bt2020Cl)
    }

    /// Returns `true` if the color space is unknown.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorSpace;
    ///
    /// assert!(ColorSpace::Unknown.is_unknown());
    /// assert!(!ColorSpace::Bt709.is_unknown());
    /// ```
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl fmt::Display for ColorSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Color range defining the valid range of color values.
///
/// Video typically uses "limited" range where black is at level 16 and white
/// at level 235 (for 8-bit). Computer graphics typically use "full" range
/// where black is 0 and white is 255.
///
/// # Common Usage
///
/// - **Limited**: Broadcast video, Blu-ray, streaming services
/// - **Full**: Computer graphics, screenshots, game capture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorRange {
    /// Limited/TV range (16-235 for Y, 16-240 for UV in 8-bit)
    #[default]
    Limited,
    /// Full/PC range (0-255 for all components in 8-bit)
    Full,
    /// Color range is not specified or unknown
    Unknown,
}

impl ColorRange {
    /// Returns the name of the color range as a human-readable string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorRange;
    ///
    /// assert_eq!(ColorRange::Limited.name(), "limited");
    /// assert_eq!(ColorRange::Full.name(), "full");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Limited => "limited",
            Self::Full => "full",
            Self::Unknown => "unknown",
        }
    }

    /// Returns `true` if this is full (PC) range.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorRange;
    ///
    /// assert!(ColorRange::Full.is_full());
    /// assert!(!ColorRange::Limited.is_full());
    /// ```
    #[must_use]
    pub const fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// Returns `true` if this is limited (TV) range.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorRange;
    ///
    /// assert!(ColorRange::Limited.is_limited());
    /// assert!(!ColorRange::Full.is_limited());
    /// ```
    #[must_use]
    pub const fn is_limited(&self) -> bool {
        matches!(self, Self::Limited)
    }

    /// Returns `true` if the color range is unknown.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorRange;
    ///
    /// assert!(ColorRange::Unknown.is_unknown());
    /// assert!(!ColorRange::Limited.is_unknown());
    /// ```
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    /// Returns the minimum value for luma (Y) in 8-bit.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorRange;
    ///
    /// assert_eq!(ColorRange::Limited.luma_min_8bit(), 16);
    /// assert_eq!(ColorRange::Full.luma_min_8bit(), 0);
    /// ```
    #[must_use]
    pub const fn luma_min_8bit(&self) -> u8 {
        match self {
            Self::Limited => 16,
            Self::Full | Self::Unknown => 0,
        }
    }

    /// Returns the maximum value for luma (Y) in 8-bit.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorRange;
    ///
    /// assert_eq!(ColorRange::Limited.luma_max_8bit(), 235);
    /// assert_eq!(ColorRange::Full.luma_max_8bit(), 255);
    /// ```
    #[must_use]
    pub const fn luma_max_8bit(&self) -> u8 {
        match self {
            Self::Limited => 235,
            Self::Full | Self::Unknown => 255,
        }
    }
}

impl fmt::Display for ColorRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Color primaries defining the color gamut (the range of colors that can be represented).
///
/// Different standards define different primary colors (red, green, blue points)
/// which determine the overall range of colors that can be displayed.
///
/// # Common Usage
///
/// - **BT.709**: HD content, same as sRGB primaries
/// - **BT.470BG / SMPTE-170M**: SD content (PAL/SECAM / NTSC)
/// - **DCI-P3 / Display P3**: digital cinema and wide-gamut displays
/// - **BT.2020**: Wide color gamut for UHD/HDR
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorPrimaries {
    /// ITU-R BT.709 primaries (same as sRGB, most common)
    #[default]
    Bt709,
    /// ITU-R BT.470BG primaries (PAL/SECAM SD video)
    Bt470bg,
    /// SMPTE 170M primaries (NTSC SD video)
    Smpte170m,
    /// SMPTE 240M primaries (legacy HD)
    Smpte240m,
    /// Generic film primaries (Illuminant C)
    Film,
    /// DCI-P3 primaries (SMPTE RP 431-2, digital cinema)
    DciP3,
    /// Display P3 primaries (SMPTE EG 432-1, wide-gamut displays)
    DisplayP3,
    /// ITU-R BT.2020 primaries (wide color gamut for UHD/HDR)
    Bt2020,
    /// Color primaries are not specified or unknown
    Unknown,
}

impl ColorPrimaries {
    /// Returns the name of the color primaries as a human-readable string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorPrimaries;
    ///
    /// assert_eq!(ColorPrimaries::Bt709.name(), "bt709");
    /// assert_eq!(ColorPrimaries::Bt2020.name(), "bt2020");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Bt709 => "bt709",
            Self::Bt470bg => "bt470bg",
            Self::Smpte170m => "smpte170m",
            Self::Smpte240m => "smpte240m",
            Self::Film => "film",
            Self::DciP3 => "dci-p3",
            Self::DisplayP3 => "display-p3",
            Self::Bt2020 => "bt2020",
            Self::Unknown => "unknown",
        }
    }

    /// Returns `true` if this uses a wide color gamut (BT.2020, DCI-P3, or Display P3).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorPrimaries;
    ///
    /// assert!(ColorPrimaries::Bt2020.is_wide_gamut());
    /// assert!(ColorPrimaries::DciP3.is_wide_gamut());
    /// assert!(!ColorPrimaries::Bt709.is_wide_gamut());
    /// ```
    #[must_use]
    pub const fn is_wide_gamut(&self) -> bool {
        matches!(self, Self::Bt2020 | Self::DciP3 | Self::DisplayP3)
    }

    /// Returns `true` if the color primaries are unknown.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorPrimaries;
    ///
    /// assert!(ColorPrimaries::Unknown.is_unknown());
    /// assert!(!ColorPrimaries::Bt709.is_unknown());
    /// ```
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl fmt::Display for ColorPrimaries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Color transfer characteristic (opto-electronic transfer function).
///
/// The transfer characteristic defines how scene luminance maps to the signal
/// level stored in the video bitstream. Different HDR and SDR standards use
/// different curves.
///
/// # Common Usage
///
/// - **`Bt709`**: Standard SDR video (HD television)
/// - **`Gamma22`** / **`Gamma28`**: Pure power-law gamma 2.2 / 2.8 (legacy SDR)
/// - **`Smpte170m`** / **`Smpte240m`**: SD / legacy-HD transfer characteristics
/// - **`Srgb`**: sRGB / IEC 61966-2-1 (computer graphics, web)
/// - **`Pq`**: HDR10 and Dolby Vision (SMPTE ST 2084 / Perceptual Quantizer)
/// - **`Hlg`**: Hybrid Log-Gamma — broadcast-compatible HDR (ARIB STD-B67)
/// - **`Bt2020_10`** / **`Bt2020_12`**: BT.2020 SDR at 10/12-bit depth
/// - **`Linear`**: Linear light, no gamma applied
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorTransfer {
    /// ITU-R BT.709 transfer characteristic (standard SDR)
    #[default]
    Bt709,
    /// Pure power-law gamma 2.2 (assumed display gamma)
    Gamma22,
    /// Pure power-law gamma 2.8 (BT.470 System B/G)
    Gamma28,
    /// SMPTE 170M transfer characteristic (SD)
    Smpte170m,
    /// SMPTE 240M transfer characteristic (legacy HD)
    Smpte240m,
    /// Linear light transfer (no gamma)
    Linear,
    /// sRGB / IEC 61966-2-1 transfer characteristic
    Srgb,
    /// ITU-R BT.2020 for 10-bit content
    Bt2020_10,
    /// ITU-R BT.2020 for 12-bit content
    Bt2020_12,
    /// Perceptual Quantizer / SMPTE ST 2084 — HDR10
    Pq,
    /// Hybrid Log-Gamma (ARIB STD-B67) — broadcast HDR
    Hlg,
    /// Transfer characteristic is not specified or unknown
    Unknown,
}

impl ColorTransfer {
    /// Returns the name of the color transfer characteristic as a string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorTransfer;
    ///
    /// assert_eq!(ColorTransfer::Bt709.name(), "bt709");
    /// assert_eq!(ColorTransfer::Hlg.name(), "hlg");
    /// assert_eq!(ColorTransfer::Pq.name(), "pq");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Bt709 => "bt709",
            Self::Gamma22 => "gamma22",
            Self::Gamma28 => "gamma28",
            Self::Smpte170m => "smpte170m",
            Self::Smpte240m => "smpte240m",
            Self::Linear => "linear",
            Self::Srgb => "srgb",
            Self::Bt2020_10 => "bt2020-10",
            Self::Bt2020_12 => "bt2020-12",
            Self::Pq => "pq",
            Self::Hlg => "hlg",
            Self::Unknown => "unknown",
        }
    }

    /// Returns `true` if this is an HDR transfer characteristic (`Pq` or `Hlg`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorTransfer;
    ///
    /// assert!(ColorTransfer::Pq.is_hdr());
    /// assert!(ColorTransfer::Hlg.is_hdr());
    /// assert!(!ColorTransfer::Bt709.is_hdr());
    /// ```
    #[must_use]
    pub const fn is_hdr(&self) -> bool {
        matches!(self, Self::Pq | Self::Hlg)
    }

    /// Returns `true` if this is Hybrid Log-Gamma (HLG).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorTransfer;
    ///
    /// assert!(ColorTransfer::Hlg.is_hlg());
    /// assert!(!ColorTransfer::Pq.is_hlg());
    /// ```
    #[must_use]
    pub const fn is_hlg(&self) -> bool {
        matches!(self, Self::Hlg)
    }

    /// Returns `true` if this is Perceptual Quantizer / SMPTE ST 2084 (PQ).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorTransfer;
    ///
    /// assert!(ColorTransfer::Pq.is_pq());
    /// assert!(!ColorTransfer::Hlg.is_pq());
    /// ```
    #[must_use]
    pub const fn is_pq(&self) -> bool {
        matches!(self, Self::Pq)
    }

    /// Returns `true` if the transfer characteristic is unknown.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::ColorTransfer;
    ///
    /// assert!(ColorTransfer::Unknown.is_unknown());
    /// assert!(!ColorTransfer::Bt709.is_unknown());
    /// ```
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl fmt::Display for ColorTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Alpha modes.
///
/// The alpha mode defines how the alpha channel should be handled when
/// converting video frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum AlphaMode {
    /// Unassociated alpha.
    #[default]
    Straight,
    /// Associated alpha.
    Premultiplied,
    /// Alpha mode is not specified or unknown
    Unknown,
}

impl AlphaMode {
    /// Returns the name of the alpha mode as a string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::AlphaMode;
    ///
    /// assert_eq!(AlphaMode::Straight.name(), "straight");
    /// assert_eq!(AlphaMode::Premultiplied.name(), "premultiplied");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Straight => "straight",
            Self::Premultiplied => "premultiplied",
            Self::Unknown => "unknown",
        }
    }

    /// Returns `true` if this is a straight alpha mode.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::AlphaMode;
    ///
    /// assert!(AlphaMode::Straight.is_straight());
    /// assert!(!AlphaMode::Premultiplied.is_straight());
    /// ```
    #[must_use]
    pub const fn is_straight(&self) -> bool {
        matches!(self, Self::Straight)
    }

    /// Returns `true` if this is a premultiplied alpha mode.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::AlphaMode;
    ///
    /// assert!(AlphaMode::Premultiplied.is_premultiplied());
    /// assert!(!AlphaMode::Straight.is_premultiplied());
    /// ```
    #[must_use]
    pub const fn is_premultiplied(&self) -> bool {
        matches!(self, Self::Premultiplied)
    }

    /// Returns `true` if the alpha mode is unknown.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::color::AlphaMode;
    ///
    /// assert!(AlphaMode::Unknown.is_unknown());
    /// assert!(!AlphaMode::Straight.is_unknown());
    /// ```
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl fmt::Display for AlphaMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod color_space_tests {
        use super::*;

        #[test]
        fn test_names() {
            assert_eq!(ColorSpace::Bt709.name(), "bt709");
            assert_eq!(ColorSpace::Bt470bg.name(), "bt470bg");
            assert_eq!(ColorSpace::Smpte170m.name(), "smpte170m");
            assert_eq!(ColorSpace::Bt2020Ncl.name(), "bt2020ncl");
            assert_eq!(ColorSpace::Bt2020Cl.name(), "bt2020cl");
            assert_eq!(ColorSpace::Rgb.name(), "rgb");
            assert_eq!(ColorSpace::Fcc.name(), "fcc");
            assert_eq!(ColorSpace::Smpte240m.name(), "smpte240m");
            assert_eq!(ColorSpace::Ycgco.name(), "ycgco");
            assert_eq!(ColorSpace::Unknown.name(), "unknown");
        }

        #[test]
        fn test_display() {
            assert_eq!(format!("{}", ColorSpace::Bt709), "bt709");
            assert_eq!(format!("{}", ColorSpace::Bt2020Ncl), "bt2020ncl");
        }

        #[test]
        fn test_default() {
            assert_eq!(ColorSpace::default(), ColorSpace::Bt709);
        }

        #[test]
        fn test_is_hd_sd_uhd() {
            assert!(ColorSpace::Bt709.is_hd());
            assert!(!ColorSpace::Bt709.is_sd());
            assert!(!ColorSpace::Bt709.is_uhd());

            assert!(!ColorSpace::Smpte170m.is_hd());
            assert!(ColorSpace::Smpte170m.is_sd());
            assert!(ColorSpace::Bt470bg.is_sd());
            assert!(!ColorSpace::Smpte170m.is_uhd());

            assert!(!ColorSpace::Bt2020Ncl.is_hd());
            assert!(!ColorSpace::Bt2020Ncl.is_sd());
            assert!(ColorSpace::Bt2020Ncl.is_uhd());
            assert!(ColorSpace::Bt2020Cl.is_uhd());
        }

        #[test]
        fn test_is_unknown() {
            assert!(ColorSpace::Unknown.is_unknown());
            assert!(!ColorSpace::Bt709.is_unknown());
        }

        #[test]
        fn test_debug() {
            assert_eq!(format!("{:?}", ColorSpace::Bt709), "Bt709");
            assert_eq!(format!("{:?}", ColorSpace::Rgb), "Rgb");
        }

        #[test]
        fn test_equality_and_hash() {
            use std::collections::HashSet;

            assert_eq!(ColorSpace::Bt709, ColorSpace::Bt709);
            assert_ne!(ColorSpace::Bt709, ColorSpace::Smpte170m);

            let mut set = HashSet::new();
            set.insert(ColorSpace::Bt709);
            set.insert(ColorSpace::Smpte170m);
            assert!(set.contains(&ColorSpace::Bt709));
            assert!(!set.contains(&ColorSpace::Bt2020Ncl));
        }

        #[test]
        fn test_copy() {
            let space = ColorSpace::Bt709;
            let copied = space;
            assert_eq!(space, copied);
        }
    }

    mod color_range_tests {
        use super::*;

        #[test]
        fn test_names() {
            assert_eq!(ColorRange::Limited.name(), "limited");
            assert_eq!(ColorRange::Full.name(), "full");
            assert_eq!(ColorRange::Unknown.name(), "unknown");
        }

        #[test]
        fn test_display() {
            assert_eq!(format!("{}", ColorRange::Limited), "limited");
            assert_eq!(format!("{}", ColorRange::Full), "full");
        }

        #[test]
        fn test_default() {
            assert_eq!(ColorRange::default(), ColorRange::Limited);
        }

        #[test]
        fn test_is_full_limited() {
            assert!(ColorRange::Full.is_full());
            assert!(!ColorRange::Full.is_limited());

            assert!(!ColorRange::Limited.is_full());
            assert!(ColorRange::Limited.is_limited());
        }

        #[test]
        fn test_is_unknown() {
            assert!(ColorRange::Unknown.is_unknown());
            assert!(!ColorRange::Limited.is_unknown());
        }

        #[test]
        fn test_luma_values() {
            assert_eq!(ColorRange::Limited.luma_min_8bit(), 16);
            assert_eq!(ColorRange::Limited.luma_max_8bit(), 235);

            assert_eq!(ColorRange::Full.luma_min_8bit(), 0);
            assert_eq!(ColorRange::Full.luma_max_8bit(), 255);

            assert_eq!(ColorRange::Unknown.luma_min_8bit(), 0);
            assert_eq!(ColorRange::Unknown.luma_max_8bit(), 255);
        }

        #[test]
        fn test_equality_and_hash() {
            use std::collections::HashSet;

            assert_eq!(ColorRange::Limited, ColorRange::Limited);
            assert_ne!(ColorRange::Limited, ColorRange::Full);

            let mut set = HashSet::new();
            set.insert(ColorRange::Limited);
            set.insert(ColorRange::Full);
            assert!(set.contains(&ColorRange::Limited));
            assert!(!set.contains(&ColorRange::Unknown));
        }
    }

    mod color_primaries_tests {
        use super::*;

        #[test]
        fn test_names() {
            assert_eq!(ColorPrimaries::Bt709.name(), "bt709");
            assert_eq!(ColorPrimaries::Bt470bg.name(), "bt470bg");
            assert_eq!(ColorPrimaries::Smpte170m.name(), "smpte170m");
            assert_eq!(ColorPrimaries::Smpte240m.name(), "smpte240m");
            assert_eq!(ColorPrimaries::Film.name(), "film");
            assert_eq!(ColorPrimaries::DciP3.name(), "dci-p3");
            assert_eq!(ColorPrimaries::DisplayP3.name(), "display-p3");
            assert_eq!(ColorPrimaries::Bt2020.name(), "bt2020");
            assert_eq!(ColorPrimaries::Unknown.name(), "unknown");
        }

        #[test]
        fn test_display() {
            assert_eq!(format!("{}", ColorPrimaries::Bt709), "bt709");
            assert_eq!(format!("{}", ColorPrimaries::Bt2020), "bt2020");
        }

        #[test]
        fn test_default() {
            assert_eq!(ColorPrimaries::default(), ColorPrimaries::Bt709);
        }

        #[test]
        fn test_is_wide_gamut() {
            assert!(ColorPrimaries::Bt2020.is_wide_gamut());
            assert!(ColorPrimaries::DciP3.is_wide_gamut());
            assert!(ColorPrimaries::DisplayP3.is_wide_gamut());
            assert!(!ColorPrimaries::Bt709.is_wide_gamut());
            assert!(!ColorPrimaries::Smpte170m.is_wide_gamut());
        }

        #[test]
        fn test_is_unknown() {
            assert!(ColorPrimaries::Unknown.is_unknown());
            assert!(!ColorPrimaries::Bt709.is_unknown());
        }

        #[test]
        fn test_equality_and_hash() {
            use std::collections::HashSet;

            assert_eq!(ColorPrimaries::Bt709, ColorPrimaries::Bt709);
            assert_ne!(ColorPrimaries::Bt709, ColorPrimaries::Bt2020);

            let mut set = HashSet::new();
            set.insert(ColorPrimaries::Bt709);
            set.insert(ColorPrimaries::Bt2020);
            assert!(set.contains(&ColorPrimaries::Bt709));
            assert!(!set.contains(&ColorPrimaries::Smpte170m));
        }
    }

    mod color_transfer_tests {
        use super::*;

        #[test]
        fn test_names() {
            assert_eq!(ColorTransfer::Bt709.name(), "bt709");
            assert_eq!(ColorTransfer::Gamma22.name(), "gamma22");
            assert_eq!(ColorTransfer::Gamma28.name(), "gamma28");
            assert_eq!(ColorTransfer::Smpte170m.name(), "smpte170m");
            assert_eq!(ColorTransfer::Smpte240m.name(), "smpte240m");
            assert_eq!(ColorTransfer::Linear.name(), "linear");
            assert_eq!(ColorTransfer::Srgb.name(), "srgb");
            assert_eq!(ColorTransfer::Bt2020_10.name(), "bt2020-10");
            assert_eq!(ColorTransfer::Bt2020_12.name(), "bt2020-12");
            assert_eq!(ColorTransfer::Pq.name(), "pq");
            assert_eq!(ColorTransfer::Hlg.name(), "hlg");
            assert_eq!(ColorTransfer::Unknown.name(), "unknown");
        }

        #[test]
        fn test_display() {
            assert_eq!(format!("{}", ColorTransfer::Hlg), "hlg");
            assert_eq!(format!("{}", ColorTransfer::Pq), "pq");
            assert_eq!(format!("{}", ColorTransfer::Bt709), "bt709");
        }

        #[test]
        fn test_default() {
            assert_eq!(ColorTransfer::default(), ColorTransfer::Bt709);
        }

        #[test]
        fn hlg_is_hdr_should_return_true() {
            assert!(ColorTransfer::Hlg.is_hdr());
            assert!(ColorTransfer::Hlg.is_hlg());
            assert!(!ColorTransfer::Hlg.is_pq());
        }

        #[test]
        fn pq_is_hdr_should_return_true() {
            assert!(ColorTransfer::Pq.is_hdr());
            assert!(ColorTransfer::Pq.is_pq());
            assert!(!ColorTransfer::Pq.is_hlg());
        }

        #[test]
        fn sdr_transfers_are_not_hdr() {
            assert!(!ColorTransfer::Bt709.is_hdr());
            assert!(!ColorTransfer::Gamma22.is_hdr());
            assert!(!ColorTransfer::Gamma28.is_hdr());
            assert!(!ColorTransfer::Smpte170m.is_hdr());
            assert!(!ColorTransfer::Smpte240m.is_hdr());
            assert!(!ColorTransfer::Srgb.is_hdr());
            assert!(!ColorTransfer::Bt2020_10.is_hdr());
            assert!(!ColorTransfer::Bt2020_12.is_hdr());
            assert!(!ColorTransfer::Linear.is_hdr());
        }

        #[test]
        fn is_unknown_should_only_match_unknown() {
            assert!(ColorTransfer::Unknown.is_unknown());
            assert!(!ColorTransfer::Bt709.is_unknown());
            assert!(!ColorTransfer::Hlg.is_unknown());
        }

        #[test]
        fn test_equality_and_hash() {
            use std::collections::HashSet;

            assert_eq!(ColorTransfer::Hlg, ColorTransfer::Hlg);
            assert_ne!(ColorTransfer::Hlg, ColorTransfer::Pq);

            let mut set = HashSet::new();
            set.insert(ColorTransfer::Hlg);
            set.insert(ColorTransfer::Pq);
            assert!(set.contains(&ColorTransfer::Hlg));
            assert!(!set.contains(&ColorTransfer::Bt709));
        }
    }

    mod alpha_mode_tests {
        use super::*;

        #[test]
        fn test_names() {
            assert_eq!(AlphaMode::Straight.name(), "straight");
            assert_eq!(AlphaMode::Premultiplied.name(), "premultiplied");
            assert_eq!(AlphaMode::Unknown.name(), "unknown");
        }

        #[test]
        fn test_display() {
            assert_eq!(format!("{}", AlphaMode::Straight), "straight");
            assert_eq!(format!("{}", AlphaMode::Premultiplied), "premultiplied");
            assert_eq!(format!("{}", AlphaMode::Unknown), "unknown");
        }

        #[test]
        fn test_default() {
            assert_eq!(AlphaMode::default(), AlphaMode::Straight);
        }

        #[test]
        fn is_straight_should_only_match_straight() {
            assert!(AlphaMode::Straight.is_straight());
            assert!(!AlphaMode::Premultiplied.is_straight());
            assert!(!AlphaMode::Unknown.is_straight());
        }

        #[test]
        fn is_premultiplied_should_only_match_premultiplied() {
            assert!(AlphaMode::Premultiplied.is_premultiplied());
            assert!(!AlphaMode::Straight.is_premultiplied());
            assert!(!AlphaMode::Unknown.is_premultiplied());
        }

        #[test]
        fn is_unknown_should_only_match_unknown() {
            assert!(AlphaMode::Unknown.is_unknown());
            assert!(!AlphaMode::Straight.is_unknown());
            assert!(!AlphaMode::Premultiplied.is_unknown());
        }

        #[test]
        fn test_equality_and_hash() {
            use std::collections::HashSet;

            assert_eq!(AlphaMode::Straight, AlphaMode::Straight);
            assert_ne!(AlphaMode::Premultiplied, AlphaMode::Straight);

            let mut set = HashSet::new();
            set.insert(AlphaMode::Straight);
            assert!(set.contains(&AlphaMode::Straight));
            assert!(!set.contains(&AlphaMode::Premultiplied));
        }
    }
}

/// An 8-bit-per-channel RGBA color value.
///
/// A plain color value (fill / text / box color), independent of any colorimetry
/// metadata ([`ColorSpace`] et al.). `a` is the alpha channel: `255` = opaque,
/// `0` = fully transparent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red channel (0–255).
    pub r: u8,
    /// Green channel (0–255).
    pub g: u8,
    /// Blue channel (0–255).
    pub b: u8,
    /// Alpha channel (0 = transparent, 255 = opaque).
    pub a: u8,
}

impl Color {
    /// Opaque white.
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    /// Fully transparent (black with zero alpha).
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// Creates an opaque color (`a = 255`).
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates a color with an explicit alpha channel.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[cfg(test)]
mod color_value_tests {
    use super::Color;

    #[test]
    fn rgb_should_be_opaque() {
        let c = Color::rgb(10, 20, 30);
        assert_eq!(c, Color::rgba(10, 20, 30, 255));
        assert_eq!(c.a, 255);
    }

    #[test]
    fn consts_should_have_expected_channels() {
        assert_eq!(Color::WHITE, Color::rgba(255, 255, 255, 255));
        assert_eq!(Color::BLACK, Color::rgba(0, 0, 0, 255));
        assert_eq!(Color::TRANSPARENT.a, 0);
    }
}
