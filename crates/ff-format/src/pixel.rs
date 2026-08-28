//! Pixel format definitions for video processing.
//!
//! This module provides the [`PixelFormat`] enum which represents various
//! pixel formats used in video processing. It supports both packed (RGB/BGRA)
//! and planar (YUV) formats commonly used in video editing.
//!
//! # Examples
//!
//! ```
//! use ff_format::PixelFormat;
//!
//! let format = PixelFormat::Yuv420p;
//! assert!(format.is_planar());
//! assert!(!format.is_packed());
//! assert_eq!(format.num_planes(), 3);
//!
//! let rgba = PixelFormat::Rgba;
//! assert!(rgba.has_alpha());
//! assert_eq!(rgba.bits_per_pixel(), Some(32));
//! ```

use std::fmt;

/// Pixel format for video frames.
///
/// This enum represents various pixel formats used in video processing.
/// It is designed to cover the most common formats used in video editing
/// while remaining extensible via the `Other` variant.
///
/// # Format Categories
///
/// - **Packed RGB**: Data stored contiguously (Rgb24, Rgba, Bgr24, Bgra)
/// - **Planar YUV**: Separate planes for Y, U, V components (Yuv420p, Yuv422p, Yuv444p)
/// - **Semi-planar**: Y plane + interleaved UV (Nv12, Nv21)
/// - **High bit depth**: 10-bit formats for HDR content (Yuv420p10le, Yuv422p10le, Yuv444p10le, Yuva444p10le, P010le)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PixelFormat {
    // Packed RGB
    /// 24-bit RGB (8:8:8) - 3 bytes per pixel
    Rgb24,
    /// 32-bit RGBA (8:8:8:8) - 4 bytes per pixel with alpha
    Rgba,
    /// 24-bit BGR (8:8:8) - 3 bytes per pixel, reversed channel order
    Bgr24,
    /// 32-bit BGRA (8:8:8:8) - 4 bytes per pixel with alpha, reversed channel order
    Bgra,

    // Planar YUV
    /// YUV 4:2:0 planar - most common video format (H.264, etc.)
    Yuv420p,
    /// YUV 4:2:2 planar - higher chroma resolution
    Yuv422p,
    /// YUV 4:4:4 planar - full chroma resolution
    Yuv444p,

    // Semi-planar (NV12/NV21)
    /// Y plane + interleaved UV - common in hardware decoders
    Nv12,
    /// Y plane + interleaved VU - Android camera format
    Nv21,

    // High bit depth
    /// 10-bit YUV 4:2:0 planar - HDR content
    Yuv420p10le,
    /// 10-bit YUV 4:2:2 planar - `ProRes` 422 profiles
    Yuv422p10le,
    /// 10-bit YUV 4:4:4 planar - `ProRes` 4444 (no alpha)
    Yuv444p10le,
    /// 10-bit YUVA 4:4:4 planar with alpha - `ProRes` 4444 with alpha
    Yuva444p10le,
    /// 10-bit semi-planar NV12 - HDR hardware decoding
    P010le,

    // Grayscale
    /// 8-bit grayscale
    Gray8,

    // Float formats
    /// 32-bit float planar GBR — native output of the `OpenEXR` decoder
    Gbrpf32le,

    // Extensibility
    /// Unknown or unsupported format with `FFmpeg`'s `AVPixelFormat` value
    Other(u32),
}

impl PixelFormat {
    /// Returns the format name as a human-readable string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Yuv420p.name(), "yuv420p");
    /// assert_eq!(PixelFormat::Rgba.name(), "rgba");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
            Self::Rgba => "rgba",
            Self::Bgr24 => "bgr24",
            Self::Bgra => "bgra",
            Self::Yuv420p => "yuv420p",
            Self::Yuv422p => "yuv422p",
            Self::Yuv444p => "yuv444p",
            Self::Nv12 => "nv12",
            Self::Nv21 => "nv21",
            Self::Yuv420p10le => "yuv420p10le",
            Self::Yuv422p10le => "yuv422p10le",
            Self::Yuv444p10le => "yuv444p10le",
            Self::Yuva444p10le => "yuva444p10le",
            Self::P010le => "p010le",
            Self::Gray8 => "gray8",
            Self::Gbrpf32le => "gbrpf32le",
            Self::Other(_) => "unknown",
        }
    }

    /// Returns the number of planes for this format.
    ///
    /// - Packed formats (RGB, RGBA, etc.) have 1 plane
    /// - Planar YUV formats have 3 planes (Y, U, V)
    /// - Semi-planar formats (NV12, NV21) have 2 planes (Y, UV)
    /// - Grayscale has 1 plane
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Rgba.num_planes(), 1);
    /// assert_eq!(PixelFormat::Yuv420p.num_planes(), 3);
    /// assert_eq!(PixelFormat::Nv12.num_planes(), 2);
    /// ```
    #[must_use]
    pub const fn num_planes(&self) -> usize {
        match self {
            // Planar YUV - Y, U, V planes (and YUVA with alpha as 4th plane)
            Self::Yuv420p
            | Self::Yuv422p
            | Self::Yuv444p
            | Self::Yuv420p10le
            | Self::Yuv422p10le
            | Self::Yuv444p10le
            | Self::Gbrpf32le => 3,
            Self::Yuva444p10le => 4,
            // Semi-planar - Y plane + interleaved UV plane
            Self::Nv12 | Self::Nv21 | Self::P010le => 2,
            // Packed formats and unknown - single plane
            Self::Rgb24 | Self::Rgba | Self::Bgr24 | Self::Bgra | Self::Gray8 | Self::Other(_) => 1,
        }
    }

    /// Alias for [`num_planes`](Self::num_planes) for API compatibility.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Yuv420p.plane_count(), 3);
    /// ```
    #[must_use]
    #[inline]
    pub const fn plane_count(&self) -> usize {
        self.num_planes()
    }

    /// Returns `true` if this is a packed format (single plane with interleaved components).
    ///
    /// Packed formats store all color components contiguously in memory,
    /// making them suitable for direct rendering but less efficient for
    /// video compression.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert!(PixelFormat::Rgba.is_packed());
    /// assert!(!PixelFormat::Yuv420p.is_packed());
    /// ```
    #[must_use]
    pub const fn is_packed(&self) -> bool {
        matches!(
            self,
            Self::Rgb24 | Self::Rgba | Self::Bgr24 | Self::Bgra | Self::Gray8
        )
    }

    /// Returns `true` if this is a planar format (separate planes for each component).
    ///
    /// Planar formats store each color component in a separate memory region,
    /// which is more efficient for video codecs and some GPU operations.
    ///
    /// Note: Semi-planar formats (NV12, NV21, P010le) are considered planar
    /// as they have multiple planes, even though UV is interleaved.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert!(PixelFormat::Yuv420p.is_planar());
    /// assert!(PixelFormat::Nv12.is_planar());  // Semi-planar is also planar
    /// assert!(!PixelFormat::Rgba.is_planar());
    /// ```
    #[must_use]
    pub const fn is_planar(&self) -> bool {
        !self.is_packed()
    }

    /// Returns `true` if this format has an alpha (transparency) channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert!(PixelFormat::Rgba.has_alpha());
    /// assert!(PixelFormat::Bgra.has_alpha());
    /// assert!(!PixelFormat::Rgb24.has_alpha());
    /// assert!(!PixelFormat::Yuv420p.has_alpha());
    /// ```
    #[must_use]
    pub const fn has_alpha(&self) -> bool {
        matches!(self, Self::Rgba | Self::Bgra | Self::Yuva444p10le)
    }

    /// Returns `true` if this is an RGB-based format.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert!(PixelFormat::Rgb24.is_rgb());
    /// assert!(PixelFormat::Rgba.is_rgb());
    /// assert!(PixelFormat::Bgra.is_rgb());  // BGR is still RGB family
    /// assert!(!PixelFormat::Yuv420p.is_rgb());
    /// ```
    #[must_use]
    pub const fn is_rgb(&self) -> bool {
        matches!(
            self,
            Self::Rgb24 | Self::Rgba | Self::Bgr24 | Self::Bgra | Self::Gbrpf32le
        )
    }

    /// Returns `true` if this is a YUV-based format.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert!(PixelFormat::Yuv420p.is_yuv());
    /// assert!(PixelFormat::Nv12.is_yuv());
    /// assert!(!PixelFormat::Rgba.is_yuv());
    /// ```
    #[must_use]
    pub const fn is_yuv(&self) -> bool {
        matches!(
            self,
            Self::Yuv420p
                | Self::Yuv422p
                | Self::Yuv444p
                | Self::Nv12
                | Self::Nv21
                | Self::Yuv420p10le
                | Self::Yuv422p10le
                | Self::Yuv444p10le
                | Self::Yuva444p10le
                | Self::P010le
        )
    }

    /// Returns the bits per pixel for packed formats.
    ///
    /// For planar formats, this returns `None` because the concept of
    /// "bits per pixel" doesn't apply directly - use [`bytes_per_pixel`](Self::bytes_per_pixel)
    /// to get the average bytes per pixel instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Rgb24.bits_per_pixel(), Some(24));
    /// assert_eq!(PixelFormat::Rgba.bits_per_pixel(), Some(32));
    /// assert_eq!(PixelFormat::Yuv420p.bits_per_pixel(), None);
    /// ```
    #[must_use]
    pub const fn bits_per_pixel(&self) -> Option<usize> {
        match self {
            Self::Rgb24 | Self::Bgr24 => Some(24),
            Self::Rgba | Self::Bgra => Some(32),
            Self::Gray8 => Some(8),
            // Planar formats don't have a simple bits-per-pixel value
            _ => None,
        }
    }

    /// Returns the average bytes per pixel.
    ///
    /// For packed formats, this is exact. For planar YUV formats, this
    /// returns the average considering subsampling:
    /// - YUV 4:2:0: 1.5 bytes/pixel (12 bits)
    /// - YUV 4:2:2: 2 bytes/pixel (16 bits)
    /// - YUV 4:4:4: 3 bytes/pixel (24 bits)
    ///
    /// Note: For formats with non-integer bytes per pixel (like `Yuv420p`),
    /// this rounds up to the nearest byte.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Rgba.bytes_per_pixel(), 4);
    /// assert_eq!(PixelFormat::Rgb24.bytes_per_pixel(), 3);
    /// assert_eq!(PixelFormat::Yuv420p.bytes_per_pixel(), 2);  // Actually 1.5, rounded up
    /// assert_eq!(PixelFormat::Yuv444p.bytes_per_pixel(), 3);
    /// ```
    #[must_use]
    pub const fn bytes_per_pixel(&self) -> usize {
        match self {
            // Grayscale - 1 byte per pixel
            Self::Gray8 => 1,

            // YUV 4:2:0 (8-bit and 10-bit) and YUV 4:2:2 - average ~2 bytes per pixel
            Self::Yuv420p
            | Self::Nv12
            | Self::Nv21
            | Self::Yuv420p10le
            | Self::P010le
            | Self::Yuv422p
            | Self::Yuv422p10le => 2,

            // RGB24/BGR24 and YUV 4:4:4 (8-bit and 10-bit) - 3 bytes per pixel
            Self::Rgb24 | Self::Bgr24 | Self::Yuv444p | Self::Yuv444p10le => 3,

            // 32-bit float planar GBR - 12 bytes per pixel (3 channels × 4 bytes)
            Self::Gbrpf32le => 12,

            // RGBA/BGRA, YUVA 4:4:4 with alpha, and unknown formats - 4 bytes per pixel
            Self::Yuva444p10le | Self::Rgba | Self::Bgra | Self::Other(_) => 4,
        }
    }

    /// Returns `true` if this is a high bit depth format (> 8 bits per component).
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert!(PixelFormat::Yuv420p10le.is_high_bit_depth());
    /// assert!(PixelFormat::P010le.is_high_bit_depth());
    /// assert!(!PixelFormat::Yuv420p.is_high_bit_depth());
    /// ```
    #[must_use]
    pub const fn is_high_bit_depth(&self) -> bool {
        matches!(
            self,
            Self::Yuv420p10le
                | Self::Yuv422p10le
                | Self::Yuv444p10le
                | Self::Yuva444p10le
                | Self::P010le
                | Self::Gbrpf32le
        )
    }

    /// Returns the bit depth per component.
    ///
    /// Most formats use 8 bits per component, while high bit depth
    /// formats use 10 bits.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Rgba.bit_depth(), 8);
    /// assert_eq!(PixelFormat::Yuv420p10le.bit_depth(), 10);
    /// ```
    #[must_use]
    pub const fn bit_depth(&self) -> usize {
        match self {
            Self::Yuv420p10le
            | Self::Yuv422p10le
            | Self::Yuv444p10le
            | Self::Yuva444p10le
            | Self::P010le => 10,
            Self::Gbrpf32le => 32,
            // All other formats including unknown are 8-bit
            _ => 8,
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Default for PixelFormat {
    /// Returns the default pixel format.
    ///
    /// The default is [`PixelFormat::Yuv420p`] as it's the most common
    /// format used in video encoding.
    fn default() -> Self {
        Self::Yuv420p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_names() {
        assert_eq!(PixelFormat::Rgb24.name(), "rgb24");
        assert_eq!(PixelFormat::Rgba.name(), "rgba");
        assert_eq!(PixelFormat::Bgr24.name(), "bgr24");
        assert_eq!(PixelFormat::Bgra.name(), "bgra");
        assert_eq!(PixelFormat::Yuv420p.name(), "yuv420p");
        assert_eq!(PixelFormat::Yuv422p.name(), "yuv422p");
        assert_eq!(PixelFormat::Yuv444p.name(), "yuv444p");
        assert_eq!(PixelFormat::Nv12.name(), "nv12");
        assert_eq!(PixelFormat::Nv21.name(), "nv21");
        assert_eq!(PixelFormat::Yuv420p10le.name(), "yuv420p10le");
        assert_eq!(PixelFormat::P010le.name(), "p010le");
        assert_eq!(PixelFormat::Gray8.name(), "gray8");
        assert_eq!(PixelFormat::Other(999).name(), "unknown");
    }

    #[test]
    fn test_plane_count() {
        // Packed formats - 1 plane
        assert_eq!(PixelFormat::Rgb24.num_planes(), 1);
        assert_eq!(PixelFormat::Rgba.num_planes(), 1);
        assert_eq!(PixelFormat::Bgr24.num_planes(), 1);
        assert_eq!(PixelFormat::Bgra.num_planes(), 1);
        assert_eq!(PixelFormat::Gray8.num_planes(), 1);

        // Planar YUV - 3 planes
        assert_eq!(PixelFormat::Yuv420p.num_planes(), 3);
        assert_eq!(PixelFormat::Yuv422p.num_planes(), 3);
        assert_eq!(PixelFormat::Yuv444p.num_planes(), 3);
        assert_eq!(PixelFormat::Yuv420p10le.num_planes(), 3);

        // Semi-planar - 2 planes
        assert_eq!(PixelFormat::Nv12.num_planes(), 2);
        assert_eq!(PixelFormat::Nv21.num_planes(), 2);
        assert_eq!(PixelFormat::P010le.num_planes(), 2);

        // plane_count is alias for num_planes
        assert_eq!(PixelFormat::Yuv420p.plane_count(), 3);
    }

    #[test]
    fn test_packed_vs_planar() {
        // Packed formats
        assert!(PixelFormat::Rgb24.is_packed());
        assert!(PixelFormat::Rgba.is_packed());
        assert!(PixelFormat::Bgr24.is_packed());
        assert!(PixelFormat::Bgra.is_packed());
        assert!(PixelFormat::Gray8.is_packed());
        assert!(!PixelFormat::Rgb24.is_planar());

        // Planar formats
        assert!(PixelFormat::Yuv420p.is_planar());
        assert!(PixelFormat::Yuv422p.is_planar());
        assert!(PixelFormat::Yuv444p.is_planar());
        assert!(PixelFormat::Nv12.is_planar());
        assert!(PixelFormat::Nv21.is_planar());
        assert!(!PixelFormat::Yuv420p.is_packed());
    }

    #[test]
    fn test_has_alpha() {
        assert!(PixelFormat::Rgba.has_alpha());
        assert!(PixelFormat::Bgra.has_alpha());
        assert!(!PixelFormat::Rgb24.has_alpha());
        assert!(!PixelFormat::Bgr24.has_alpha());
        assert!(!PixelFormat::Yuv420p.has_alpha());
        assert!(!PixelFormat::Gray8.has_alpha());
    }

    #[test]
    fn test_is_rgb() {
        assert!(PixelFormat::Rgb24.is_rgb());
        assert!(PixelFormat::Rgba.is_rgb());
        assert!(PixelFormat::Bgr24.is_rgb());
        assert!(PixelFormat::Bgra.is_rgb());
        assert!(!PixelFormat::Yuv420p.is_rgb());
        assert!(!PixelFormat::Nv12.is_rgb());
        assert!(!PixelFormat::Gray8.is_rgb());
    }

    #[test]
    fn test_is_yuv() {
        assert!(PixelFormat::Yuv420p.is_yuv());
        assert!(PixelFormat::Yuv422p.is_yuv());
        assert!(PixelFormat::Yuv444p.is_yuv());
        assert!(PixelFormat::Nv12.is_yuv());
        assert!(PixelFormat::Nv21.is_yuv());
        assert!(PixelFormat::Yuv420p10le.is_yuv());
        assert!(PixelFormat::P010le.is_yuv());
        assert!(!PixelFormat::Rgb24.is_yuv());
        assert!(!PixelFormat::Rgba.is_yuv());
        assert!(!PixelFormat::Gray8.is_yuv());
    }

    #[test]
    fn test_bits_per_pixel() {
        // Packed formats have defined bits per pixel
        assert_eq!(PixelFormat::Rgb24.bits_per_pixel(), Some(24));
        assert_eq!(PixelFormat::Bgr24.bits_per_pixel(), Some(24));
        assert_eq!(PixelFormat::Rgba.bits_per_pixel(), Some(32));
        assert_eq!(PixelFormat::Bgra.bits_per_pixel(), Some(32));
        assert_eq!(PixelFormat::Gray8.bits_per_pixel(), Some(8));

        // Planar formats don't have simple bits per pixel
        assert_eq!(PixelFormat::Yuv420p.bits_per_pixel(), None);
        assert_eq!(PixelFormat::Nv12.bits_per_pixel(), None);
    }

    #[test]
    fn test_bytes_per_pixel() {
        // Packed formats
        assert_eq!(PixelFormat::Rgb24.bytes_per_pixel(), 3);
        assert_eq!(PixelFormat::Bgr24.bytes_per_pixel(), 3);
        assert_eq!(PixelFormat::Rgba.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::Bgra.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::Gray8.bytes_per_pixel(), 1);

        // YUV 4:2:0 - 1.5 bytes average, rounded to 2
        assert_eq!(PixelFormat::Yuv420p.bytes_per_pixel(), 2);
        assert_eq!(PixelFormat::Nv12.bytes_per_pixel(), 2);
        assert_eq!(PixelFormat::Nv21.bytes_per_pixel(), 2);

        // YUV 4:2:2 - 2 bytes
        assert_eq!(PixelFormat::Yuv422p.bytes_per_pixel(), 2);

        // YUV 4:4:4 - 3 bytes
        assert_eq!(PixelFormat::Yuv444p.bytes_per_pixel(), 3);

        // High bit depth
        assert_eq!(PixelFormat::Yuv420p10le.bytes_per_pixel(), 2);
        assert_eq!(PixelFormat::P010le.bytes_per_pixel(), 2);
    }

    #[test]
    fn test_high_bit_depth() {
        assert!(PixelFormat::Yuv420p10le.is_high_bit_depth());
        assert!(PixelFormat::P010le.is_high_bit_depth());
        assert!(!PixelFormat::Yuv420p.is_high_bit_depth());
        assert!(!PixelFormat::Rgba.is_high_bit_depth());
    }

    #[test]
    fn test_bit_depth() {
        assert_eq!(PixelFormat::Rgba.bit_depth(), 8);
        assert_eq!(PixelFormat::Yuv420p.bit_depth(), 8);
        assert_eq!(PixelFormat::Yuv420p10le.bit_depth(), 10);
        assert_eq!(PixelFormat::P010le.bit_depth(), 10);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", PixelFormat::Yuv420p), "yuv420p");
        assert_eq!(format!("{}", PixelFormat::Rgba), "rgba");
        assert_eq!(format!("{}", PixelFormat::Other(123)), "unknown");
    }

    #[test]
    fn test_default() {
        assert_eq!(PixelFormat::default(), PixelFormat::Yuv420p);
    }

    #[test]
    fn test_debug() {
        assert_eq!(format!("{:?}", PixelFormat::Rgba), "Rgba");
        assert_eq!(format!("{:?}", PixelFormat::Yuv420p), "Yuv420p");
        assert_eq!(format!("{:?}", PixelFormat::Other(42)), "Other(42)");
    }

    #[test]
    fn test_equality_and_hash() {
        use std::collections::HashSet;

        assert_eq!(PixelFormat::Rgba, PixelFormat::Rgba);
        assert_ne!(PixelFormat::Rgba, PixelFormat::Bgra);
        assert_eq!(PixelFormat::Other(1), PixelFormat::Other(1));
        assert_ne!(PixelFormat::Other(1), PixelFormat::Other(2));

        // Test Hash implementation
        let mut set = HashSet::new();
        set.insert(PixelFormat::Rgba);
        set.insert(PixelFormat::Yuv420p);
        assert!(set.contains(&PixelFormat::Rgba));
        assert!(!set.contains(&PixelFormat::Bgra));
    }

    #[test]
    fn test_copy() {
        let format = PixelFormat::Yuv420p;
        let copied = format;
        // Both original and copy are still usable (Copy semantics)
        assert_eq!(format, copied);
        assert_eq!(format.name(), copied.name());
    }

    #[test]
    fn gbrpf32le_name_should_return_gbrpf32le() {
        assert_eq!(PixelFormat::Gbrpf32le.name(), "gbrpf32le");
    }

    #[test]
    fn gbrpf32le_should_have_three_planes() {
        assert_eq!(PixelFormat::Gbrpf32le.num_planes(), 3);
        assert_eq!(PixelFormat::Gbrpf32le.plane_count(), 3);
    }

    #[test]
    fn gbrpf32le_should_be_planar_not_packed() {
        assert!(!PixelFormat::Gbrpf32le.is_packed());
        assert!(PixelFormat::Gbrpf32le.is_planar());
    }

    #[test]
    fn gbrpf32le_should_be_rgb_family() {
        assert!(PixelFormat::Gbrpf32le.is_rgb());
        assert!(!PixelFormat::Gbrpf32le.is_yuv());
    }

    #[test]
    fn gbrpf32le_should_not_have_alpha() {
        assert!(!PixelFormat::Gbrpf32le.has_alpha());
    }

    #[test]
    fn gbrpf32le_should_have_twelve_bytes_per_pixel() {
        assert_eq!(PixelFormat::Gbrpf32le.bytes_per_pixel(), 12);
    }

    #[test]
    fn gbrpf32le_should_be_high_bit_depth() {
        assert!(PixelFormat::Gbrpf32le.is_high_bit_depth());
    }

    #[test]
    fn gbrpf32le_should_have_bit_depth_32() {
        assert_eq!(PixelFormat::Gbrpf32le.bit_depth(), 32);
    }

    #[test]
    fn gbrpf32le_bits_per_pixel_should_be_none() {
        assert_eq!(PixelFormat::Gbrpf32le.bits_per_pixel(), None);
    }
}
