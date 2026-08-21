//! Typed representation of an FFmpeg return code.
//!
//! The safe wrapper layer currently returns bare `c_int` error codes; migrating
//! those signatures to [`AvError`] and updating the downstream consumers is
//! tracked in #1488 (ADR-0003). This module introduces the type: it wraps the
//! negative `c_int` FFmpeg returns, renders it through `av_strerror`, and exposes
//! the `EAGAIN` / `EOF` drain states as predicates so the send/receive loop can
//! read them without comparing raw codes.

use std::fmt;
use std::os::raw::c_int;

/// A typed FFmpeg return code.
///
/// Wraps the raw negative `c_int` error code returned by FFmpeg. [`Display`](fmt::Display)
/// renders it through `av_strerror`, and [`is_eagain`](Self::is_eagain) /
/// [`is_eof`](Self::is_eof) expose the drain states the send/receive loop cares
/// about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvError(c_int);

impl AvError {
    /// Wraps a raw FFmpeg error code.
    #[must_use]
    pub const fn new(code: c_int) -> Self {
        Self(code)
    }

    /// Returns the raw FFmpeg error code.
    #[must_use]
    pub const fn code(self) -> c_int {
        self.0
    }

    /// Returns `true` when the code is `EAGAIN`: the decoder or encoder needs
    /// more input before it can produce output.
    #[must_use]
    pub const fn is_eagain(self) -> bool {
        self.0 == crate::error_codes::EAGAIN
    }

    /// Returns `true` when the code is `AVERROR_EOF`: the stream is fully
    /// drained and no more output will be produced.
    #[must_use]
    pub const fn is_eof(self) -> bool {
        self.0 == crate::error_codes::EOF
    }
}

impl fmt::Display for AvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code={})", crate::av_error_string(self.0), self.0)
    }
}

impl std::error::Error for AvError {}

impl From<c_int> for AvError {
    fn from(code: c_int) -> Self {
        Self(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av_error_should_preserve_the_raw_code() {
        assert_eq!(AvError::new(-22).code(), -22);
    }

    #[test]
    fn av_error_should_detect_eagain() {
        assert!(AvError::new(crate::error_codes::EAGAIN).is_eagain());
        assert!(!AvError::new(crate::error_codes::EOF).is_eagain());
    }

    #[test]
    fn av_error_should_detect_eof() {
        assert!(AvError::new(crate::error_codes::EOF).is_eof());
        assert!(!AvError::new(crate::error_codes::EAGAIN).is_eof());
    }

    #[test]
    fn av_error_display_should_include_the_message_and_code() {
        let rendered = AvError::new(crate::error_codes::EOF).to_string();
        assert!(
            rendered.contains("code="),
            "display should include the raw code: {rendered}"
        );
        assert!(!rendered.is_empty());
    }

    #[test]
    fn av_error_should_convert_from_a_raw_code() {
        let err = AvError::from(-22);
        assert_eq!(err.code(), -22);
    }
}
