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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    /// Parses an `FFmpeg` colour string, as `-vf drawbox=color=...` and friends
    /// accept it.
    ///
    /// Recognised forms:
    ///
    /// * `#RRGGBB` / `#RRGGBBAA` and `0xRRGGBB` / `0xRRGGBBAA`
    /// * a colour name, matched case-insensitively (`"green"`, `"DarkOrchid"`)
    /// * either of the above with an `@alpha` suffix, where alpha is a float in
    ///   `0.0..=1.0` or a two-digit `0xAA`
    ///
    /// Returns `None` for anything else, including `FFmpeg`'s `random` — it has no
    /// fixed value, so there is nothing to return.
    ///
    /// Alpha defaults to opaque when the form does not carry one.
    #[must_use]
    pub fn parse_ffmpeg(s: &str) -> Option<Self> {
        let s = s.trim();
        // `FFmpeg` splits the alpha suffix off first, so `0xRRGGBB@0.5` is as valid
        // as `red@0.5`; an `@` on an 8-digit hex overrides that hex's alpha byte.
        let (body, alpha) = match s.split_once('@') {
            Some((body, alpha)) => (body, Some(alpha)),
            None => (s, None),
        };

        let mut color = parse_hex_color(body).or_else(|| lookup_color_name(body))?;
        if let Some(alpha) = alpha {
            color.a = parse_alpha(alpha)?;
        }
        Some(color)
    }

    /// Every colour name [`parse_ffmpeg`](Self::parse_ffmpeg) accepts, with its
    /// value, in ascending name order.
    ///
    /// Lets a host offer the same palette `FFmpeg` understands (a colour picker,
    /// an autocomplete), and it is what
    /// `crates/ff-filter/tests/color_parse_reference_tests.rs` walks to re-check
    /// every row against the linked `FFmpeg`.
    #[must_use]
    pub fn ffmpeg_color_names() -> impl ExactSizeIterator<Item = (&'static str, Self)> {
        FFMPEG_COLOR_NAMES
            .iter()
            .map(|&(name, [r, g, b])| (name, Self::rgb(r, g, b)))
    }
}

/// Parses `#RRGGBB[AA]` / `0xRRGGBB[AA]`, or `None` for any other shape.
fn parse_hex_color(s: &str) -> Option<Color> {
    // `0x` lower-case only: `av_parse_color` compares the prefix case-sensitively,
    // so accepting `0X` here would map a colour on the GPU that the CPU filter path
    // cannot build — measured, and the only form the two disagreed on.
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix('#'))?;
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(Color {
        r: byte(0)?,
        g: byte(2)?,
        b: byte(4)?,
        a: if hex.len() == 8 { byte(6)? } else { 255 },
    })
}

/// Parses the `@alpha` suffix: a two-digit `0xAA`, or a float in `0.0..=1.0`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_alpha(s: &str) -> Option<u8> {
    if let Some(hex) = s.strip_prefix("0x") {
        // Exactly two digits, which is what the documentation above promises;
        // `from_str_radix` alone would also take `@0x8`.
        if hex.len() != 2 {
            return None;
        }
        return u8::from_str_radix(hex, 16).ok();
    }
    let f: f32 = s.parse().ok()?;
    if !(0.0..=1.0).contains(&f) {
        return None;
    }
    // Rounding, not truncation: 1.0 must reach 255 and 0.5 must not land a byte low.
    Some((f * 255.0).round() as u8)
}

/// Looks `name` up in [`FFMPEG_COLOR_NAMES`], case-insensitively.
fn lookup_color_name(name: &str) -> Option<Color> {
    let lower = name.to_ascii_lowercase();
    let idx = FFMPEG_COLOR_NAMES
        .binary_search_by(|(n, _)| (*n).cmp(lower.as_str()))
        .ok()?;
    let [r, g, b] = FFMPEG_COLOR_NAMES[idx].1;
    Some(Color::rgb(r, g, b))
}

/// The colour names the linked `FFmpeg` accepts, lower-cased and sorted so the
/// lookup can binary-search. Mirrors `libavutil/parseutils.c`'s `color_table`.
///
/// These values are **not** the CSS list from memory: each was read out of
/// `FFmpeg` itself by rendering `color=c=<name>` and sampling the pixel, which is
/// how the surprises got caught — `green` is `0x008000` (the HTML value, not X11's
/// `0x00FF00`), and `FFmpeg` carries `lightgrey` but no `lightgray` while every
/// other grey is spelled `gray`. `ff-format` has no `FFmpeg` to ask at runtime, so
/// `crates/ff-filter/tests/color_parse_reference_tests.rs` re-checks every row
/// against it (RK-005: verify tokens against the real thing, not against docs).
const FFMPEG_COLOR_NAMES: &[(&str, [u8; 3])] = &[
    ("aliceblue", [0xF0, 0xF8, 0xFF]),
    ("antiquewhite", [0xFA, 0xEB, 0xD7]),
    ("aqua", [0x00, 0xFF, 0xFF]),
    ("aquamarine", [0x7F, 0xFF, 0xD4]),
    ("azure", [0xF0, 0xFF, 0xFF]),
    ("beige", [0xF5, 0xF5, 0xDC]),
    ("bisque", [0xFF, 0xE4, 0xC4]),
    ("black", [0x00, 0x00, 0x00]),
    ("blanchedalmond", [0xFF, 0xEB, 0xCD]),
    ("blue", [0x00, 0x00, 0xFF]),
    ("blueviolet", [0x8A, 0x2B, 0xE2]),
    ("brown", [0xA5, 0x2A, 0x2A]),
    ("burlywood", [0xDE, 0xB8, 0x87]),
    ("cadetblue", [0x5F, 0x9E, 0xA0]),
    ("chartreuse", [0x7F, 0xFF, 0x00]),
    ("chocolate", [0xD2, 0x69, 0x1E]),
    ("coral", [0xFF, 0x7F, 0x50]),
    ("cornflowerblue", [0x64, 0x95, 0xED]),
    ("cornsilk", [0xFF, 0xF8, 0xDC]),
    ("crimson", [0xDC, 0x14, 0x3C]),
    ("cyan", [0x00, 0xFF, 0xFF]),
    ("darkblue", [0x00, 0x00, 0x8B]),
    ("darkcyan", [0x00, 0x8B, 0x8B]),
    ("darkgoldenrod", [0xB8, 0x86, 0x0B]),
    ("darkgray", [0xA9, 0xA9, 0xA9]),
    ("darkgreen", [0x00, 0x64, 0x00]),
    ("darkkhaki", [0xBD, 0xB7, 0x6B]),
    ("darkmagenta", [0x8B, 0x00, 0x8B]),
    ("darkolivegreen", [0x55, 0x6B, 0x2F]),
    ("darkorange", [0xFF, 0x8C, 0x00]),
    ("darkorchid", [0x99, 0x32, 0xCC]),
    ("darkred", [0x8B, 0x00, 0x00]),
    ("darksalmon", [0xE9, 0x96, 0x7A]),
    ("darkseagreen", [0x8F, 0xBC, 0x8F]),
    ("darkslateblue", [0x48, 0x3D, 0x8B]),
    ("darkslategray", [0x2F, 0x4F, 0x4F]),
    ("darkturquoise", [0x00, 0xCE, 0xD1]),
    ("darkviolet", [0x94, 0x00, 0xD3]),
    ("deeppink", [0xFF, 0x14, 0x93]),
    ("deepskyblue", [0x00, 0xBF, 0xFF]),
    ("dimgray", [0x69, 0x69, 0x69]),
    ("dodgerblue", [0x1E, 0x90, 0xFF]),
    ("firebrick", [0xB2, 0x22, 0x22]),
    ("floralwhite", [0xFF, 0xFA, 0xF0]),
    ("forestgreen", [0x22, 0x8B, 0x22]),
    ("fuchsia", [0xFF, 0x00, 0xFF]),
    ("gainsboro", [0xDC, 0xDC, 0xDC]),
    ("ghostwhite", [0xF8, 0xF8, 0xFF]),
    ("gold", [0xFF, 0xD7, 0x00]),
    ("goldenrod", [0xDA, 0xA5, 0x20]),
    ("gray", [0x80, 0x80, 0x80]),
    ("green", [0x00, 0x80, 0x00]),
    ("greenyellow", [0xAD, 0xFF, 0x2F]),
    ("honeydew", [0xF0, 0xFF, 0xF0]),
    ("hotpink", [0xFF, 0x69, 0xB4]),
    ("indianred", [0xCD, 0x5C, 0x5C]),
    ("indigo", [0x4B, 0x00, 0x82]),
    ("ivory", [0xFF, 0xFF, 0xF0]),
    ("khaki", [0xF0, 0xE6, 0x8C]),
    ("lavender", [0xE6, 0xE6, 0xFA]),
    ("lavenderblush", [0xFF, 0xF0, 0xF5]),
    ("lawngreen", [0x7C, 0xFC, 0x00]),
    ("lemonchiffon", [0xFF, 0xFA, 0xCD]),
    ("lightblue", [0xAD, 0xD8, 0xE6]),
    ("lightcoral", [0xF0, 0x80, 0x80]),
    ("lightcyan", [0xE0, 0xFF, 0xFF]),
    ("lightgoldenrodyellow", [0xFA, 0xFA, 0xD2]),
    ("lightgreen", [0x90, 0xEE, 0x90]),
    ("lightgrey", [0xD3, 0xD3, 0xD3]),
    ("lightpink", [0xFF, 0xB6, 0xC1]),
    ("lightsalmon", [0xFF, 0xA0, 0x7A]),
    ("lightseagreen", [0x20, 0xB2, 0xAA]),
    ("lightskyblue", [0x87, 0xCE, 0xFA]),
    ("lightslategray", [0x77, 0x88, 0x99]),
    ("lightsteelblue", [0xB0, 0xC4, 0xDE]),
    ("lightyellow", [0xFF, 0xFF, 0xE0]),
    ("lime", [0x00, 0xFF, 0x00]),
    ("limegreen", [0x32, 0xCD, 0x32]),
    ("linen", [0xFA, 0xF0, 0xE6]),
    ("magenta", [0xFF, 0x00, 0xFF]),
    ("maroon", [0x80, 0x00, 0x00]),
    ("mediumaquamarine", [0x66, 0xCD, 0xAA]),
    ("mediumblue", [0x00, 0x00, 0xCD]),
    ("mediumorchid", [0xBA, 0x55, 0xD3]),
    ("mediumpurple", [0x93, 0x70, 0xD8]),
    ("mediumseagreen", [0x3C, 0xB3, 0x71]),
    ("mediumslateblue", [0x7B, 0x68, 0xEE]),
    ("mediumspringgreen", [0x00, 0xFA, 0x9A]),
    ("mediumturquoise", [0x48, 0xD1, 0xCC]),
    ("mediumvioletred", [0xC7, 0x15, 0x85]),
    ("midnightblue", [0x19, 0x19, 0x70]),
    ("mintcream", [0xF5, 0xFF, 0xFA]),
    ("mistyrose", [0xFF, 0xE4, 0xE1]),
    ("moccasin", [0xFF, 0xE4, 0xB5]),
    ("navajowhite", [0xFF, 0xDE, 0xAD]),
    ("navy", [0x00, 0x00, 0x80]),
    ("oldlace", [0xFD, 0xF5, 0xE6]),
    ("olive", [0x80, 0x80, 0x00]),
    ("olivedrab", [0x6B, 0x8E, 0x23]),
    ("orange", [0xFF, 0xA5, 0x00]),
    ("orangered", [0xFF, 0x45, 0x00]),
    ("orchid", [0xDA, 0x70, 0xD6]),
    ("palegoldenrod", [0xEE, 0xE8, 0xAA]),
    ("palegreen", [0x98, 0xFB, 0x98]),
    ("paleturquoise", [0xAF, 0xEE, 0xEE]),
    ("palevioletred", [0xD8, 0x70, 0x93]),
    ("papayawhip", [0xFF, 0xEF, 0xD5]),
    ("peachpuff", [0xFF, 0xDA, 0xB9]),
    ("peru", [0xCD, 0x85, 0x3F]),
    ("pink", [0xFF, 0xC0, 0xCB]),
    ("plum", [0xDD, 0xA0, 0xDD]),
    ("powderblue", [0xB0, 0xE0, 0xE6]),
    ("purple", [0x80, 0x00, 0x80]),
    ("red", [0xFF, 0x00, 0x00]),
    ("rosybrown", [0xBC, 0x8F, 0x8F]),
    ("royalblue", [0x41, 0x69, 0xE1]),
    ("saddlebrown", [0x8B, 0x45, 0x13]),
    ("salmon", [0xFA, 0x80, 0x72]),
    ("sandybrown", [0xF4, 0xA4, 0x60]),
    ("seagreen", [0x2E, 0x8B, 0x57]),
    ("seashell", [0xFF, 0xF5, 0xEE]),
    ("sienna", [0xA0, 0x52, 0x2D]),
    ("silver", [0xC0, 0xC0, 0xC0]),
    ("skyblue", [0x87, 0xCE, 0xEB]),
    ("slateblue", [0x6A, 0x5A, 0xCD]),
    ("slategray", [0x70, 0x80, 0x90]),
    ("snow", [0xFF, 0xFA, 0xFA]),
    ("springgreen", [0x00, 0xFF, 0x7F]),
    ("steelblue", [0x46, 0x82, 0xB4]),
    ("tan", [0xD2, 0xB4, 0x8C]),
    ("teal", [0x00, 0x80, 0x80]),
    ("thistle", [0xD8, 0xBF, 0xD8]),
    ("tomato", [0xFF, 0x63, 0x47]),
    ("turquoise", [0x40, 0xE0, 0xD0]),
    ("violet", [0xEE, 0x82, 0xEE]),
    ("wheat", [0xF5, 0xDE, 0xB3]),
    ("white", [0xFF, 0xFF, 0xFF]),
    ("whitesmoke", [0xF5, 0xF5, 0xF5]),
    ("yellow", [0xFF, 0xFF, 0x00]),
    ("yellowgreen", [0x9A, 0xCD, 0x32]),
];

#[cfg(test)]
mod color_value_tests {
    use super::Color;

    // parse_ffmpeg

    #[test]
    fn parse_ffmpeg_should_read_hex_with_and_without_alpha() {
        assert_eq!(
            Color::parse_ffmpeg("0x123456"),
            Some(Color::rgb(18, 52, 86))
        );
        assert_eq!(Color::parse_ffmpeg("#123456"), Some(Color::rgb(18, 52, 86)));
        assert_eq!(
            Color::parse_ffmpeg("0x12345680"),
            Some(Color::rgba(18, 52, 86, 128))
        );
    }

    #[test]
    fn parse_ffmpeg_should_read_a_name_case_insensitively() {
        // `green` is the HTML value, not X11's `0x00FF00` — the distinction the
        // table was read out of FFmpeg to get right.
        let green = Some(Color::rgb(0, 128, 0));
        assert_eq!(Color::parse_ffmpeg("green"), green);
        assert_eq!(Color::parse_ffmpeg("Green"), green);
        assert_eq!(Color::parse_ffmpeg("GREEN"), green);
        assert_eq!(Color::parse_ffmpeg("lime"), Some(Color::rgb(0, 255, 0)));
        assert_eq!(
            Color::parse_ffmpeg("DarkOrchid"),
            Some(Color::rgb(0x99, 0x32, 0xCC))
        );
    }

    #[test]
    fn parse_ffmpeg_should_read_an_alpha_suffix_in_both_spellings() {
        assert_eq!(
            Color::parse_ffmpeg("black@0x80"),
            Some(Color::rgba(0, 0, 0, 128))
        );
        assert_eq!(
            Color::parse_ffmpeg("black@1"),
            Some(Color::rgba(0, 0, 0, 255))
        );
        assert_eq!(
            Color::parse_ffmpeg("black@0"),
            Some(Color::rgba(0, 0, 0, 0))
        );
        // The suffix overrides a hex form's own alpha byte, as FFmpeg's parser does.
        assert_eq!(
            Color::parse_ffmpeg("0x11223344@1.0"),
            Some(Color::rgba(0x11, 0x22, 0x33, 255))
        );
    }

    #[test]
    fn parse_ffmpeg_should_reject_unparseable_forms() {
        // `random` has no fixed value, so there is nothing to return.
        assert_eq!(Color::parse_ffmpeg("random"), None);
        assert_eq!(Color::parse_ffmpeg("no_such_colour"), None);
        // FFmpeg has `lightgrey` but no `lightgray`; the table says so, and this
        // pins that it is the table talking and not a fuzzy match.
        assert!(Color::parse_ffmpeg("lightgrey").is_some());
        assert_eq!(Color::parse_ffmpeg("lightgray"), None);
        assert_eq!(Color::parse_ffmpeg("0x1234"), None);
        // FFmpeg's prefix comparison is case-sensitive, so `0X` is not a colour.
        // Accepting it would map on the GPU what the CPU filter path rejects.
        assert_eq!(Color::parse_ffmpeg("0X123456"), None);
        // The alpha suffix's hex form is two digits, as the doc says.
        assert_eq!(Color::parse_ffmpeg("black@0x8"), None);
        assert_eq!(Color::parse_ffmpeg("black@0x080"), None);
        assert_eq!(Color::parse_ffmpeg("0xGGGGGG"), None);
        assert_eq!(Color::parse_ffmpeg("black@1.5"), None);
        assert_eq!(Color::parse_ffmpeg(""), None);
    }

    #[test]
    fn ffmpeg_color_names_should_be_sorted_and_lower_case() {
        // `lookup_color_name` binary-searches, so an unsorted or mixed-case row
        // would make some names silently unfindable rather than fail loudly.
        for pair in super::FFMPEG_COLOR_NAMES.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "table must be sorted and unique: {:?} then {:?}",
                pair[0].0,
                pair[1].0
            );
        }
        for (name, _) in super::FFMPEG_COLOR_NAMES {
            assert_eq!(
                *name,
                name.to_ascii_lowercase(),
                "table names must be lower-case: {name:?}"
            );
        }
    }

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
