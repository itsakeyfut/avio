//! Shared error classification for the `ff-*` crate family.
//!
//! Every `ff-*` error type implements [`MediaError`], which classifies an error
//! by [`ErrorSeverity`] so a caller can branch on recoverability generically,
//! without matching each crate's variants. This lives in `ff-format` (the lowest
//! shared, `FFmpeg`-free type crate) and adds no `FFmpeg` dependency.

/// Severity class of a media error: whether the failing operation can be retried
/// without rebuilding the component that raised it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorSeverity {
    /// The component cannot continue; it must be discarded or reconfigured
    /// (e.g. a missing file, an unsupported codec, an I/O failure).
    Fatal,
    /// Transient: the failing operation can be retried, or the stream reconnected,
    /// without rebuilding (e.g. a corrupt frame, a network timeout).
    Recoverable,
    /// Neither strictly fatal nor retryable: a one-off condition the caller can
    /// handle in context (e.g. a raw `FFmpeg` error, no frame at a timestamp).
    Other,
}

/// Shared classification for the `ff-*` crate error types.
///
/// Implement [`severity`](MediaError::severity) on each error type; the boolean
/// helpers are derived from it. This lets downstream code branch on recoverability
/// generically:
///
/// ```
/// use ff_format::{FormatError, MediaError, ErrorSeverity};
///
/// let err = FormatError::invalid_pixel_format("nope");
/// assert_eq!(err.severity(), ErrorSeverity::Other);
/// assert!(!err.is_recoverable());
/// assert!(!err.is_fatal());
/// ```
pub trait MediaError {
    /// Classifies this error.
    fn severity(&self) -> ErrorSeverity;

    /// Returns `true` if the failing operation can be retried without rebuilding.
    fn is_recoverable(&self) -> bool {
        matches!(self.severity(), ErrorSeverity::Recoverable)
    }

    /// Returns `true` if the component must be discarded or reconfigured.
    fn is_fatal(&self) -> bool {
        matches!(self.severity(), ErrorSeverity::Fatal)
    }
}

impl MediaError for crate::FormatError {
    fn severity(&self) -> ErrorSeverity {
        // Format errors are validation / conversion failures on caller-provided
        // data: not retryable, but a one-off input problem rather than a component
        // that must be torn down.
        ErrorSeverity::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_should_drive_is_recoverable_and_is_fatal() {
        struct Fatal;
        struct Recoverable;
        struct Other;
        impl MediaError for Fatal {
            fn severity(&self) -> ErrorSeverity {
                ErrorSeverity::Fatal
            }
        }
        impl MediaError for Recoverable {
            fn severity(&self) -> ErrorSeverity {
                ErrorSeverity::Recoverable
            }
        }
        impl MediaError for Other {
            fn severity(&self) -> ErrorSeverity {
                ErrorSeverity::Other
            }
        }

        assert!(Fatal.is_fatal() && !Fatal.is_recoverable());
        assert!(Recoverable.is_recoverable() && !Recoverable.is_fatal());
        assert!(!Other.is_fatal() && !Other.is_recoverable());
    }

    #[test]
    fn format_error_severity_should_be_other() {
        let err = crate::FormatError::invalid_pixel_format("x");
        assert_eq!(err.severity(), ErrorSeverity::Other);
    }
}
