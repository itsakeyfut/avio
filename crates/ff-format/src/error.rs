//! Error types for ff-format crate.
//!
//! This module defines error types used across the ff-* crate family.
//! [`FormatError`] is the main error type for format-related operations,
//! while [`FrameError`] handles frame-specific operations.
//!
//! # Examples
//!
//! ```
//! use ff_format::error::FormatError;
//!
//! fn validate_format(name: &str) -> Result<(), FormatError> {
//!     if name.is_empty() {
//!         return Err(FormatError::InvalidPixelFormat {
//!             format: name.to_string(),
//!         });
//!     }
//!     Ok(())
//! }
//! ```

use std::fmt;

use thiserror::Error;

use crate::{PixelFormat, Rational, SampleFormat};

/// Error type for format-related operations.
///
/// This is the main error type for the ff-format crate and is used
/// for errors related to pixel formats, sample formats, timestamps,
/// and format conversions.
///
/// # Error Variants
///
/// - [`InvalidPixelFormat`](FormatError::InvalidPixelFormat): Invalid or unsupported pixel format string
/// - [`InvalidSampleFormat`](FormatError::InvalidSampleFormat): Invalid or unsupported sample format string
/// - [`InvalidTimestamp`](FormatError::InvalidTimestamp): Invalid timestamp with PTS and time base
/// - [`PlaneIndexOutOfBounds`](FormatError::PlaneIndexOutOfBounds): Plane index exceeds available planes
/// - [`ConversionFailed`](FormatError::ConversionFailed): Pixel format conversion failed
/// - [`AudioConversionFailed`](FormatError::AudioConversionFailed): Audio sample format conversion failed
/// - [`InvalidFrameData`](FormatError::InvalidFrameData): General frame data validation error
///
/// # Examples
///
/// ```
/// use ff_format::{FormatError, PixelFormat, Rational};
///
/// // Create an invalid timestamp error
/// let error = FormatError::InvalidTimestamp {
///     pts: -1,
///     time_base: Rational::new(1, 90000),
/// };
/// assert!(error.to_string().contains("pts=-1"));
///
/// // Create a plane index out of bounds error
/// let error = FormatError::PlaneIndexOutOfBounds {
///     index: 5,
///     max: 3,
/// };
/// assert!(error.to_string().contains("out of bounds"));
/// ```
#[derive(Error, Debug, Clone, PartialEq)]
pub enum FormatError {
    /// Invalid or unrecognized pixel format string.
    ///
    /// This error occurs when parsing a pixel format name that is not
    /// recognized or supported.
    #[error("Invalid pixel format: {format}")]
    InvalidPixelFormat {
        /// The invalid pixel format string.
        format: String,
    },

    /// Invalid or unrecognized sample format string.
    ///
    /// This error occurs when parsing a sample format name that is not
    /// recognized or supported.
    #[error("Invalid sample format: {format}")]
    InvalidSampleFormat {
        /// The invalid sample format string.
        format: String,
    },

    /// Invalid timestamp value.
    ///
    /// This error occurs when a timestamp has an invalid PTS value
    /// or incompatible time base.
    #[error("Invalid timestamp: pts={pts}, time_base={time_base:?}")]
    InvalidTimestamp {
        /// The PTS (Presentation Timestamp) value.
        pts: i64,
        /// The time base used for the timestamp.
        time_base: Rational,
    },

    /// Plane index exceeds the number of available planes.
    ///
    /// This error occurs when trying to access a plane that doesn't exist
    /// in the frame. For example, accessing plane 3 of an RGB image that
    /// only has plane 0.
    #[error("Plane index {index} out of bounds (max: {max})")]
    PlaneIndexOutOfBounds {
        /// The requested plane index.
        index: usize,
        /// The maximum valid plane index.
        max: usize,
    },

    /// Pixel format conversion failed.
    ///
    /// This error occurs when attempting to convert between two pixel
    /// formats that is not supported or fails.
    #[error("Format conversion failed: {from:?} -> {to:?}")]
    ConversionFailed {
        /// The source pixel format.
        from: PixelFormat,
        /// The target pixel format.
        to: PixelFormat,
    },

    /// Audio sample format conversion failed.
    ///
    /// This error occurs when attempting to convert between two audio
    /// sample formats that is not supported or fails.
    #[error("Audio conversion failed: {from:?} -> {to:?}")]
    AudioConversionFailed {
        /// The source sample format.
        from: SampleFormat,
        /// The target sample format.
        to: SampleFormat,
    },

    /// Invalid or corrupted frame data.
    ///
    /// This error occurs when frame data is invalid, corrupted, or
    /// doesn't match the expected format parameters.
    #[error("Invalid frame data: {0}")]
    InvalidFrameData(String),
}

impl FormatError {
    /// Creates an `InvalidPixelFormat` error from a format string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::FormatError;
    ///
    /// let error = FormatError::invalid_pixel_format("unknown_format");
    /// assert!(error.to_string().contains("unknown_format"));
    /// ```
    #[inline]
    #[must_use]
    pub fn invalid_pixel_format(format: impl Into<String>) -> Self {
        Self::InvalidPixelFormat {
            format: format.into(),
        }
    }

    /// Creates an `InvalidSampleFormat` error from a format string.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::FormatError;
    ///
    /// let error = FormatError::invalid_sample_format("unknown_format");
    /// assert!(error.to_string().contains("unknown_format"));
    /// ```
    #[inline]
    #[must_use]
    pub fn invalid_sample_format(format: impl Into<String>) -> Self {
        Self::InvalidSampleFormat {
            format: format.into(),
        }
    }

    /// Creates an `InvalidFrameData` error with a description.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::FormatError;
    ///
    /// let error = FormatError::invalid_frame_data("buffer size mismatch");
    /// assert!(error.to_string().contains("buffer size"));
    /// ```
    #[inline]
    #[must_use]
    pub fn invalid_frame_data(reason: impl Into<String>) -> Self {
        Self::InvalidFrameData(reason.into())
    }

    /// Creates a `PlaneIndexOutOfBounds` error.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::FormatError;
    ///
    /// let error = FormatError::plane_out_of_bounds(5, 3);
    /// assert!(error.to_string().contains("5"));
    /// assert!(error.to_string().contains("3"));
    /// ```
    #[inline]
    #[must_use]
    pub fn plane_out_of_bounds(index: usize, max: usize) -> Self {
        Self::PlaneIndexOutOfBounds { index, max }
    }

    /// Creates a `ConversionFailed` error for pixel format conversion.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{FormatError, PixelFormat};
    ///
    /// let error = FormatError::conversion_failed(PixelFormat::Yuv420p, PixelFormat::Rgba);
    /// assert!(error.to_string().contains("Yuv420p"));
    /// assert!(error.to_string().contains("Rgba"));
    /// ```
    #[inline]
    #[must_use]
    pub fn conversion_failed(from: PixelFormat, to: PixelFormat) -> Self {
        Self::ConversionFailed { from, to }
    }

    /// Creates an `AudioConversionFailed` error for sample format conversion.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{FormatError, SampleFormat};
    ///
    /// let error = FormatError::audio_conversion_failed(SampleFormat::I16, SampleFormat::F32);
    /// assert!(error.to_string().contains("I16"));
    /// assert!(error.to_string().contains("F32"));
    /// ```
    #[inline]
    #[must_use]
    pub fn audio_conversion_failed(from: SampleFormat, to: SampleFormat) -> Self {
        Self::AudioConversionFailed { from, to }
    }

    /// Creates an `InvalidTimestamp` error.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{FormatError, Rational};
    ///
    /// let error = FormatError::invalid_timestamp(-1, Rational::new(1, 90000));
    /// assert!(error.to_string().contains("-1"));
    /// ```
    #[inline]
    #[must_use]
    pub fn invalid_timestamp(pts: i64, time_base: Rational) -> Self {
        Self::InvalidTimestamp { pts, time_base }
    }
}

/// Error type for frame operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The number of planes does not match the number of strides.
    MismatchedPlaneStride {
        /// Number of planes provided.
        planes: usize,
        /// Number of strides provided.
        strides: usize,
    },
    /// Cannot allocate a frame for an unknown pixel format.
    UnsupportedPixelFormat(PixelFormat),
    /// Cannot allocate an audio frame for an unknown sample format.
    UnsupportedSampleFormat(SampleFormat),
    /// The number of planes does not match the expected count for the format.
    InvalidPlaneCount {
        /// Expected number of planes.
        expected: usize,
        /// Actual number of planes provided.
        actual: usize,
    },
    /// Pixel data length does not match the expected size for the dimensions/format.
    InvalidDataSize {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length provided.
        actual: usize,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MismatchedPlaneStride { planes, strides } => {
                write!(
                    f,
                    "planes and strides length mismatch: {planes} planes, {strides} strides"
                )
            }
            Self::UnsupportedPixelFormat(format) => {
                write!(
                    f,
                    "cannot allocate frame for unsupported pixel format: {format:?}"
                )
            }
            Self::UnsupportedSampleFormat(format) => {
                write!(
                    f,
                    "cannot allocate frame for unsupported sample format: {format:?}"
                )
            }
            Self::InvalidPlaneCount { expected, actual } => {
                write!(f, "invalid plane count: expected {expected}, got {actual}")
            }
            Self::InvalidDataSize { expected, actual } => {
                write!(
                    f,
                    "invalid data size: expected {expected} bytes, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// Error type for subtitle parsing operations.
#[derive(Debug, Error)]
pub enum SubtitleError {
    /// I/O error reading a subtitle file.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// File extension is not a recognized subtitle format.
    #[error("unsupported subtitle format: {extension}")]
    UnsupportedFormat {
        /// The unrecognized file extension.
        extension: String,
    },

    /// A structural parse error prevents processing the file.
    #[error("parse error at line {line}: {reason}")]
    ParseError {
        /// 1-based line number where the error was detected.
        line: usize,
        /// Human-readable description of the problem.
        reason: String,
    },

    /// The input contained no valid subtitle events.
    #[error("no valid subtitle events found")]
    NoEvents,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // === FormatError Tests ===

    #[test]
    fn test_format_error_invalid_pixel_format() {
        let err = FormatError::InvalidPixelFormat {
            format: "unknown_xyz".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("Invalid pixel format"));
        assert!(msg.contains("unknown_xyz"));

        // Test helper function
        let err = FormatError::invalid_pixel_format("bad_format");
        let msg = format!("{err}");
        assert!(msg.contains("bad_format"));
    }

    #[test]
    fn test_format_error_invalid_sample_format() {
        let err = FormatError::InvalidSampleFormat {
            format: "unknown_audio".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("Invalid sample format"));
        assert!(msg.contains("unknown_audio"));

        // Test helper function
        let err = FormatError::invalid_sample_format("bad_audio");
        let msg = format!("{err}");
        assert!(msg.contains("bad_audio"));
    }

    #[test]
    fn test_format_error_invalid_timestamp() {
        let time_base = Rational::new(1, 90000);
        let err = FormatError::InvalidTimestamp {
            pts: -100,
            time_base,
        };
        let msg = format!("{err}");
        assert!(msg.contains("Invalid timestamp"));
        assert!(msg.contains("pts=-100"));
        assert!(msg.contains("time_base"));

        // Test helper function
        let err = FormatError::invalid_timestamp(-50, Rational::new(1, 1000));
        let msg = format!("{err}");
        assert!(msg.contains("-50"));
    }

    #[test]
    fn test_format_error_plane_out_of_bounds() {
        let err = FormatError::PlaneIndexOutOfBounds { index: 5, max: 3 };
        let msg = format!("{err}");
        assert!(msg.contains("Plane index 5"));
        assert!(msg.contains("out of bounds"));
        assert!(msg.contains("max: 3"));

        // Test helper function
        let err = FormatError::plane_out_of_bounds(10, 2);
        let msg = format!("{err}");
        assert!(msg.contains("10"));
        assert!(msg.contains("2"));
    }

    #[test]
    fn test_format_error_conversion_failed() {
        let err = FormatError::ConversionFailed {
            from: PixelFormat::Yuv420p,
            to: PixelFormat::Rgba,
        };
        let msg = format!("{err}");
        assert!(msg.contains("Format conversion failed"));
        assert!(msg.contains("Yuv420p"));
        assert!(msg.contains("Rgba"));

        // Test helper function
        let err = FormatError::conversion_failed(PixelFormat::Nv12, PixelFormat::Bgra);
        let msg = format!("{err}");
        assert!(msg.contains("Nv12"));
        assert!(msg.contains("Bgra"));
    }

    #[test]
    fn test_format_error_audio_conversion_failed() {
        let err = FormatError::AudioConversionFailed {
            from: SampleFormat::I16,
            to: SampleFormat::F32,
        };
        let msg = format!("{err}");
        assert!(msg.contains("Audio conversion failed"));
        assert!(msg.contains("I16"));
        assert!(msg.contains("F32"));

        // Test helper function
        let err = FormatError::audio_conversion_failed(SampleFormat::U8, SampleFormat::F64);
        let msg = format!("{err}");
        assert!(msg.contains("U8"));
        assert!(msg.contains("F64"));
    }

    #[test]
    fn test_format_error_invalid_frame_data() {
        let err = FormatError::InvalidFrameData("buffer too small".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("Invalid frame data"));
        assert!(msg.contains("buffer too small"));

        // Test helper function
        let err = FormatError::invalid_frame_data("corrupted header");
        let msg = format!("{err}");
        assert!(msg.contains("corrupted header"));
    }

    #[test]
    fn test_format_error_is_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(FormatError::InvalidPixelFormat {
            format: "test".to_string(),
        });
        // Verify it implements std::error::Error
        assert!(err.to_string().contains("test"));
    }

    #[test]
    fn test_format_error_equality() {
        let err1 = FormatError::InvalidPixelFormat {
            format: "test".to_string(),
        };
        let err2 = FormatError::InvalidPixelFormat {
            format: "test".to_string(),
        };
        let err3 = FormatError::InvalidPixelFormat {
            format: "other".to_string(),
        };

        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_format_error_clone() {
        let err1 = FormatError::ConversionFailed {
            from: PixelFormat::Yuv420p,
            to: PixelFormat::Rgba,
        };
        let err2 = err1.clone();
        assert_eq!(err1, err2);
    }

    #[test]
    fn test_format_error_debug() {
        let err = FormatError::PlaneIndexOutOfBounds { index: 3, max: 2 };
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("PlaneIndexOutOfBounds"));
        assert!(debug_str.contains("index"));
        assert!(debug_str.contains("max"));
    }

    // === FrameError Tests ===

    #[test]
    fn test_frame_error_display() {
        let err = FrameError::MismatchedPlaneStride {
            planes: 1,
            strides: 2,
        };
        let msg = format!("{err}");
        assert!(msg.contains("planes"));
        assert!(msg.contains("strides"));
        assert!(msg.contains("mismatch"));

        let err = FrameError::UnsupportedPixelFormat(PixelFormat::Other(42));
        let msg = format!("{err}");
        assert!(msg.contains("unsupported"));
        assert!(msg.contains("pixel format"));

        let err = FrameError::UnsupportedSampleFormat(SampleFormat::Other(42));
        let msg = format!("{err}");
        assert!(msg.contains("unsupported"));
        assert!(msg.contains("sample format"));

        let err = FrameError::InvalidPlaneCount {
            expected: 2,
            actual: 1,
        };
        let msg = format!("{err}");
        assert!(msg.contains("expected 2"));
        assert!(msg.contains("got 1"));
    }
}
