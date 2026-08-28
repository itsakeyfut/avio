//! Common FFmpeg error codes for convenience.

/// Resource temporarily unavailable (try again).
///
/// `AVERROR(EAGAIN)` is platform-specific:
/// - Linux/Windows (MinGW): `EAGAIN = 11`, so `AVERROR(EAGAIN) = -11`
/// - macOS/BSD: `EAGAIN = 35`, so `AVERROR(EAGAIN) = -35`
#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
pub const EAGAIN: i32 = -35;
#[cfg(not(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd")))]
pub const EAGAIN: i32 = -11;

/// End of file/stream
pub const EOF: i32 = -541_478_725; // AVERROR_EOF

/// Out of memory
pub const ENOMEM: i32 = -12;

/// Invalid data
pub const EINVAL: i32 = -22;

// Network errno values (AVERROR = -errno on POSIX)
//
// These are used in ff-decode to map FFmpeg network errors to typed variants.
// errno numbering differs across platforms:
//   - macOS/BSD: uses BSD errno values
//   - Windows UCRT: uses its own POSIX-extension errno table (VS2015+)
//   - Linux: standard POSIX errno values
//
// Windows UCRT errno.h (relevant socket codes, added in VS2015/UCRT):
//   ECONNREFUSED=107, EHOSTUNREACH=110, ETIMEDOUT=138, ENETUNREACH=118

/// Connection timed out (`ETIMEDOUT`).
#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
pub const ETIMEDOUT: i32 = -60;
#[cfg(windows)]
pub const ETIMEDOUT: i32 = -138; // UCRT: ETIMEDOUT = 138
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
pub const ETIMEDOUT: i32 = -110;

/// Connection refused (`ECONNREFUSED`).
#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
pub const ECONNREFUSED: i32 = -61;
#[cfg(windows)]
pub const ECONNREFUSED: i32 = -107; // UCRT: ECONNREFUSED = 107
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
pub const ECONNREFUSED: i32 = -111;

/// No route to host (`EHOSTUNREACH`).
#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
pub const EHOSTUNREACH: i32 = -65;
#[cfg(windows)]
pub const EHOSTUNREACH: i32 = -110; // UCRT: EHOSTUNREACH = 110
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
pub const EHOSTUNREACH: i32 = -113;

/// Network unreachable (`ENETUNREACH`).
#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
pub const ENETUNREACH: i32 = -51;
#[cfg(windows)]
pub const ENETUNREACH: i32 = -118; // UCRT: ENETUNREACH = 118
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
pub const ENETUNREACH: i32 = -101;

/// I/O error (`EIO`). Same value on all POSIX platforms.
pub const EIO: i32 = -5;

/// Invalid data found when processing input (`AVERROR_INVALIDDATA`).
///
/// This is `FFERRTAG(0xF8,'I','N','V')` — a tag-based code, not an errno,
/// so it has the same value on all platforms.
pub const AVERROR_INVALIDDATA: i32 = -1_447_971_320;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_should_all_be_negative() {
        assert!(EAGAIN < 0);
        assert!(EOF < 0);
        assert!(ENOMEM < 0);
        assert!(EINVAL < 0);
    }

    #[test]
    fn averror_invaliddata_should_be_negative_and_distinct_from_einval() {
        assert!(AVERROR_INVALIDDATA < 0);
        assert_ne!(AVERROR_INVALIDDATA, EINVAL);
    }

    #[test]
    fn averror_invaliddata_should_have_expected_value() {
        // FFERRTAG(0xF8,'I','N','V') = -(0xF8 | 'I'<<8 | 'N'<<16 | 'V'<<24)
        assert_eq!(AVERROR_INVALIDDATA, -1_447_971_320);
    }
}
