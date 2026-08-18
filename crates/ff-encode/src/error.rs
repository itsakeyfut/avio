//! Error types for encoding operations.

use ff_format::{ErrorSeverity, MediaError};
use std::path::PathBuf;
use thiserror::Error;

/// Encoding error type.
#[derive(Error, Debug)]
pub enum EncodeError {
    /// Cannot create output file
    #[error("Cannot create output file: {path}")]
    CannotCreateFile {
        /// File path that failed
        path: PathBuf,
    },

    /// Unsupported codec
    #[error("Unsupported codec: {codec}")]
    UnsupportedCodec {
        /// Codec name
        codec: String,
    },

    /// No suitable encoder found
    #[error("No suitable encoder found for {codec} (tried: {tried:?})")]
    NoSuitableEncoder {
        /// Requested codec
        codec: String,
        /// Attempted encoders
        tried: Vec<String>,
    },

    /// Encoding failed at specific frame
    #[error("Encoding failed at frame {frame}: {reason}")]
    EncodingFailed {
        /// Frame number where encoding failed
        frame: u64,
        /// Failure reason
        reason: String,
    },

    /// Invalid configuration
    #[error("Invalid configuration: {reason}")]
    InvalidConfig {
        /// Configuration issue description
        reason: String,
    },

    /// Hardware encoder unavailable
    #[error("Hardware encoder unavailable: {encoder}")]
    HwEncoderUnavailable {
        /// Hardware encoder name
        encoder: String,
    },

    /// Specific encoder is unavailable — the hint explains what is needed.
    #[error("encoder unavailable: codec={codec} hint={hint}")]
    EncoderUnavailable {
        /// Requested codec name (e.g. `"h265/hevc"`).
        codec: String,
        /// Human-readable guidance (e.g. how to build FFmpeg with this encoder).
        hint: String,
    },

    /// Muxing failed
    #[error("Muxing failed: {reason}")]
    MuxingFailed {
        /// Failure reason
        reason: String,
    },

    /// `FFmpeg` error
    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg {
        /// Raw `FFmpeg` error code (negative integer). `0` when no numeric code is available.
        code: i32,
        /// Human-readable error message from `av_strerror` or an internal description.
        message: String,
    },

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid option value
    #[error("Invalid option: {name} — {reason}")]
    InvalidOption {
        /// Option name
        name: String,
        /// Description of the constraint that was violated
        reason: String,
    },

    /// Codec is incompatible with the target container format
    #[error("codec {codec} is not supported by container {container} — {hint}")]
    UnsupportedContainerCodecCombination {
        /// Container format name (e.g. `"webm"`)
        container: String,
        /// Codec name that was rejected (e.g. `"h264"`)
        codec: String,
        /// Human-readable guidance on compatible codecs
        hint: String,
    },

    /// Video dimensions are outside the supported range [2, 32768].
    #[error("dimensions {width}x{height} out of range [2, 32768]")]
    InvalidDimensions {
        /// Requested frame width.
        width: u32,
        /// Requested frame height.
        height: u32,
    },

    /// Target bitrate exceeds the 800 Mbps maximum.
    #[error("bitrate {bitrate} bps exceeds maximum 800 Mbps (800,000,000 bps)")]
    InvalidBitrate {
        /// Requested bitrate in bits per second.
        bitrate: u64,
    },

    /// Audio channel count exceeds the supported maximum of 8.
    #[error("channel count {count} exceeds maximum 8")]
    InvalidChannelCount {
        /// Requested channel count.
        count: u32,
    },

    /// Audio sample rate is outside the supported range [8000, 384000] Hz.
    #[error("sample rate {rate} Hz outside supported range [8000, 384000]")]
    InvalidSampleRate {
        /// Requested sample rate in Hz.
        rate: u32,
    },

    /// Encoding cancelled by user
    #[error("Encoding cancelled by user")]
    Cancelled,

    /// Async encoder worker thread panicked or disconnected unexpectedly
    #[error("Async encoder worker panicked or disconnected")]
    WorkerPanicked,

    /// An export preset violated a platform-specific constraint.
    ///
    /// Returned by [`ExportPreset::validate()`](crate::ExportPreset::validate)
    /// when the preset's configuration conflicts with a platform rule (e.g.
    /// fps > 60 for a YouTube preset).
    #[error("preset constraint violated: preset={preset} reason={reason}")]
    PresetConstraintViolation {
        /// Name of the preset that failed validation.
        preset: String,
        /// Human-readable description of the violated constraint.
        reason: String,
    },
}

impl EncodeError {
    /// Create an error from an FFmpeg error code.
    ///
    /// This is more type-safe than implementing `From<i32>` globally,
    /// as it makes the conversion explicit and prevents accidental
    /// conversion of arbitrary i32 values.
    pub(crate) fn from_ffmpeg_error(errnum: i32) -> Self {
        EncodeError::Ffmpeg {
            code: errnum,
            message: ff_sys::av_error_string(errnum),
        }
    }
}

impl MediaError for EncodeError {
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Ffmpeg { .. } | Self::Cancelled => ErrorSeverity::Other,
            Self::CannotCreateFile { .. }
            | Self::UnsupportedCodec { .. }
            | Self::NoSuitableEncoder { .. }
            | Self::EncodingFailed { .. }
            | Self::InvalidConfig { .. }
            | Self::HwEncoderUnavailable { .. }
            | Self::EncoderUnavailable { .. }
            | Self::MuxingFailed { .. }
            | Self::Io(_)
            | Self::InvalidOption { .. }
            | Self::UnsupportedContainerCodecCombination { .. }
            | Self::InvalidDimensions { .. }
            | Self::InvalidBitrate { .. }
            | Self::InvalidChannelCount { .. }
            | Self::InvalidSampleRate { .. }
            | Self::WorkerPanicked
            | Self::PresetConstraintViolation { .. } => ErrorSeverity::Fatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EncodeError;

    #[test]
    fn from_ffmpeg_error_should_return_ffmpeg_variant() {
        let err = EncodeError::from_ffmpeg_error(ff_sys::error_codes::EINVAL);
        assert!(matches!(err, EncodeError::Ffmpeg { .. }));
    }

    #[test]
    fn from_ffmpeg_error_should_carry_numeric_code() {
        let err = EncodeError::from_ffmpeg_error(ff_sys::error_codes::EINVAL);
        match err {
            EncodeError::Ffmpeg { code, .. } => assert_eq!(code, ff_sys::error_codes::EINVAL),
            _ => panic!("expected Ffmpeg variant"),
        }
    }

    #[test]
    fn from_ffmpeg_error_should_format_with_code_in_display() {
        let err = EncodeError::from_ffmpeg_error(ff_sys::error_codes::EINVAL);
        let msg = err.to_string();
        assert!(msg.contains("code=-22"), "expected 'code=-22' in '{msg}'");
    }

    #[test]
    fn from_ffmpeg_error_message_should_be_nonempty() {
        let err = EncodeError::from_ffmpeg_error(ff_sys::error_codes::ENOMEM);
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn from_ffmpeg_error_eof_should_be_constructible() {
        let err = EncodeError::from_ffmpeg_error(ff_sys::error_codes::EOF);
        assert!(matches!(err, EncodeError::Ffmpeg { .. }));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn invalid_dimensions_display_should_contain_dimension_string() {
        let err = EncodeError::InvalidDimensions {
            width: 0,
            height: 720,
        };
        let msg = err.to_string();
        assert!(msg.contains("0x720"), "expected '0x720' in '{msg}'");
    }

    #[test]
    fn invalid_dimensions_display_should_contain_range_hint() {
        let err = EncodeError::InvalidDimensions {
            width: 99999,
            height: 99999,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("[2, 32768]"),
            "expected '[2, 32768]' in '{msg}'"
        );
    }

    #[test]
    fn invalid_bitrate_display_should_contain_bitrate_value() {
        let err = EncodeError::InvalidBitrate {
            bitrate: 900_000_000,
        };
        let msg = err.to_string();
        assert!(msg.contains("900000000"), "expected '900000000' in '{msg}'");
    }

    #[test]
    fn invalid_bitrate_display_should_contain_maximum_hint() {
        let err = EncodeError::InvalidBitrate {
            bitrate: 900_000_000,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("800,000,000"),
            "expected '800,000,000' in '{msg}'"
        );
    }

    #[test]
    fn invalid_channel_count_display_should_contain_count() {
        let err = EncodeError::InvalidChannelCount { count: 9 };
        let msg = err.to_string();
        assert!(msg.contains('9'), "expected '9' in '{msg}'");
    }

    #[test]
    fn invalid_channel_count_display_should_contain_maximum_hint() {
        let err = EncodeError::InvalidChannelCount { count: 9 };
        let msg = err.to_string();
        assert!(msg.contains('8'), "expected '8' in '{msg}'");
    }

    #[test]
    fn invalid_sample_rate_display_should_contain_rate() {
        let err = EncodeError::InvalidSampleRate { rate: 7999 };
        let msg = err.to_string();
        assert!(msg.contains("7999"), "expected '7999' in '{msg}'");
    }

    #[test]
    fn invalid_sample_rate_display_should_contain_range_hint() {
        let err = EncodeError::InvalidSampleRate { rate: 7999 };
        let msg = err.to_string();
        assert!(
            msg.contains("[8000, 384000]"),
            "expected '[8000, 384000]' in '{msg}'"
        );
    }
}
