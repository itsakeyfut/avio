//! Sample format helpers and constants.

use crate::{
    AVSampleFormat, AVSampleFormat_AV_SAMPLE_FMT_DBL, AVSampleFormat_AV_SAMPLE_FMT_DBLP,
    AVSampleFormat_AV_SAMPLE_FMT_FLT, AVSampleFormat_AV_SAMPLE_FMT_FLTP,
    AVSampleFormat_AV_SAMPLE_FMT_NONE, AVSampleFormat_AV_SAMPLE_FMT_S16,
    AVSampleFormat_AV_SAMPLE_FMT_S16P, AVSampleFormat_AV_SAMPLE_FMT_S32,
    AVSampleFormat_AV_SAMPLE_FMT_S32P, AVSampleFormat_AV_SAMPLE_FMT_S64,
    AVSampleFormat_AV_SAMPLE_FMT_S64P, AVSampleFormat_AV_SAMPLE_FMT_U8,
    AVSampleFormat_AV_SAMPLE_FMT_U8P, av_get_bytes_per_sample as ffi_av_get_bytes_per_sample,
    av_sample_fmt_is_planar as ffi_av_sample_fmt_is_planar,
};

// Re-export common sample formats
pub const NONE: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_NONE;
pub const U8: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_U8;
pub const S16: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S16;
pub const S32: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S32;
pub const S64: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S64;
pub const FLT: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_FLT;
pub const DBL: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_DBL;

// Planar formats
pub const U8P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_U8P;
pub const S16P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S16P;
pub const S32P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S32P;
pub const S64P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S64P;
pub const FLTP: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_FLTP;
pub const DBLP: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_DBLP;

/// Get the number of bytes per sample for a given format.
///
/// # Arguments
///
/// * `sample_fmt` - The sample format
///
/// # Returns
///
/// Returns the number of bytes per sample, or a negative value for invalid formats.
pub fn bytes_per_sample(sample_fmt: AVSampleFormat) -> i32 {
    // SAFETY: av_get_bytes_per_sample reads only the enum value passed by
    // value and dereferences no pointers.
    unsafe { ffi_av_get_bytes_per_sample(sample_fmt) }
}

/// Check if a sample format is planar.
///
/// Planar formats store each channel in a separate plane,
/// while packed (interleaved) formats store all channels together.
///
/// # Arguments
///
/// * `sample_fmt` - The sample format to check
///
/// # Returns
///
/// Returns `true` if the format is planar, `false` if packed.
pub fn is_planar(sample_fmt: AVSampleFormat) -> bool {
    // SAFETY: av_sample_fmt_is_planar reads only the enum value passed by
    // value and dereferences no pointers.
    unsafe { ffi_av_sample_fmt_is_planar(sample_fmt) != 0 }
}

/// Check if a sample format is valid (not NONE).
///
/// # Arguments
///
/// * `sample_fmt` - The sample format to check
///
/// # Returns
///
/// Returns `true` if the format is valid.
pub fn is_valid(sample_fmt: AVSampleFormat) -> bool {
    sample_fmt != NONE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_format_bytes() {
        assert_eq!(bytes_per_sample(U8), 1);
        assert_eq!(bytes_per_sample(S16), 2);
        assert_eq!(bytes_per_sample(S32), 4);
        assert_eq!(bytes_per_sample(FLT), 4);
        assert_eq!(bytes_per_sample(DBL), 8);
    }

    #[test]
    fn test_sample_format_is_planar() {
        // Packed formats
        assert!(!is_planar(U8));
        assert!(!is_planar(S16));
        assert!(!is_planar(FLT));

        // Planar formats
        assert!(is_planar(U8P));
        assert!(is_planar(S16P));
        assert!(is_planar(FLTP));
    }

    #[test]
    fn test_sample_format_is_valid() {
        assert!(is_valid(S16));
        assert!(is_valid(FLT));
        assert!(!is_valid(NONE));
    }
}
