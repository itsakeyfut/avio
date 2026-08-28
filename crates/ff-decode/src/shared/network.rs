//! Network helpers for URL detection, sanitization, and error mapping.
//!
//! This module is crate-private. All items are `pub(crate)`.

use crate::error::DecodeError;
use ff_sys::error_codes;

/// URL scheme prefixes that indicate a network source rather than a local file.
const URL_SCHEMES: &[&str] = &[
    "http://", "https://", "rtmp://", "rtsp://", "udp://", "srt://", "rtp://",
];

/// Returns `true` when `path` starts with a recognised network URL scheme.
///
/// Used by the builder to skip the file-existence check and by the inner
/// decoder to select the network-aware open path.
pub(crate) fn is_url(path: &str) -> bool {
    URL_SCHEMES.iter().any(|scheme| path.starts_with(scheme))
}

/// Strips the password and query string from a URL for safe logging.
///
/// - `user:password@host` → `user:***@host`
/// - Query string (`?…`) is removed entirely (may contain tokens)
/// - If the URL cannot be parsed, the scheme+`://`+authority portion is
///   returned as a fallback.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(
///     sanitize_url("rtmp://admin:s3cr3t@live.example.com/app/key?token=abc"),
///     "rtmp://admin:***@live.example.com/app/key",
/// );
/// assert_eq!(
///     sanitize_url("rtmp://live.example.com/app/key"),
///     "rtmp://live.example.com/app/key",
/// );
/// ```
pub(crate) fn sanitize_url(url: &str) -> String {
    // Locate the "://" separator to find where the authority begins.
    let Some(scheme_end) = url.find("://") else {
        return url.to_owned();
    };
    let after_scheme = scheme_end + 3; // skip "://"

    // The authority ends at the first '/' after the scheme, or at the end of the string.
    let authority_end = url[after_scheme..]
        .find('/')
        .map_or(url.len(), |i| after_scheme + i);

    let authority = &url[after_scheme..authority_end];

    // Build the sanitized URL.
    let scheme_prefix = &url[..after_scheme]; // e.g. "rtmp://"
    let path_part = &url[authority_end..]; // e.g. "/app/key?token=abc"

    // Strip query string from the path portion.
    let path_clean = path_part.find('?').map_or(path_part, |i| &path_part[..i]);

    if let Some(at) = authority.rfind('@') {
        // Credentials present: strip the password.
        let user_info = &authority[..at];
        let host = &authority[at + 1..];

        let safe_user_info = user_info.find(':').map_or_else(
            || user_info.to_owned(),
            |colon| format!("{}:***", &user_info[..colon]),
        );

        format!("{scheme_prefix}{safe_user_info}@{host}{path_clean}")
    } else {
        // No credentials: just strip the query string.
        format!("{scheme_prefix}{authority}{path_clean}")
    }
}

/// Checks whether `url` can be opened as an SRT source.
///
/// Returns `Ok(())` immediately when `url` does not start with `srt://`.
///
/// For `srt://` URLs:
/// - Without the `srt` feature flag: always returns [`DecodeError::ConnectionFailed`]
///   with an instruction to recompile with `features = ["srt"]`.
/// - With the `srt` feature flag: checks at runtime whether the linked `FFmpeg`
///   was built with libsrt and returns [`DecodeError::ConnectionFailed`] if not.
pub(crate) fn check_srt_url(url: &str) -> Result<(), DecodeError> {
    if !url.starts_with("srt://") {
        return Ok(());
    }
    check_srt_available(url)
}

#[cfg(not(feature = "srt"))]
fn check_srt_available(url: &str) -> Result<(), DecodeError> {
    Err(DecodeError::ConnectionFailed {
        code: 0,
        endpoint: sanitize_url(url),
        message: "SRT protocol is not enabled; recompile with feature = \"srt\"".to_string(),
    })
}

#[cfg(feature = "srt")]
fn check_srt_available(url: &str) -> Result<(), DecodeError> {
    if !ff_sys::avformat::srt_available() {
        return Err(DecodeError::ConnectionFailed {
            code: 0,
            endpoint: sanitize_url(url),
            message: "SRT protocol is not available in the linked FFmpeg build; \
                      recompile FFmpeg with --enable-libsrt"
                .to_string(),
        });
    }
    Ok(())
}

/// Maps an `FFmpeg` network error code to the most specific [`DecodeError`] variant.
///
/// The `endpoint` string must already be sanitized (no password, no query string).
pub(crate) fn map_network_error(code: i32, endpoint: String) -> DecodeError {
    let message = ff_sys::av_error_string(code);
    match code {
        c if c == error_codes::ETIMEDOUT => DecodeError::NetworkTimeout {
            code,
            endpoint,
            message,
        },
        c if c == error_codes::ECONNREFUSED
            || c == error_codes::EHOSTUNREACH
            || c == error_codes::ENETUNREACH =>
        {
            DecodeError::ConnectionFailed {
                code,
                endpoint,
                message,
            }
        }
        c if c == error_codes::EIO => DecodeError::StreamInterrupted {
            code,
            endpoint,
            message,
        },
        _ => DecodeError::Ffmpeg { code, message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // is_url

    #[test]
    fn is_url_should_return_true_for_http() {
        assert!(is_url("http://example.com/stream"));
    }

    #[test]
    fn is_url_should_return_true_for_https() {
        assert!(is_url("https://cdn.example.com/index.m3u8"));
    }

    #[test]
    fn is_url_should_return_true_for_rtmp() {
        assert!(is_url("rtmp://live.example.com/app/key"));
    }

    #[test]
    fn is_url_should_return_true_for_rtsp() {
        assert!(is_url("rtsp://camera.local/stream"));
    }

    #[test]
    fn is_url_should_return_true_for_udp() {
        assert!(is_url("udp://239.0.0.1:1234"));
    }

    #[test]
    fn is_url_should_return_true_for_srt() {
        assert!(is_url("srt://ingest.example.com:4200"));
    }

    #[test]
    fn is_url_should_return_true_for_rtp() {
        assert!(is_url("rtp://239.0.0.1:5004"));
    }

    #[test]
    fn is_url_should_return_false_for_local_path() {
        assert!(!is_url("/home/user/video.mp4"));
    }

    #[test]
    fn is_url_should_return_false_for_relative_path() {
        assert!(!is_url("video.mp4"));
    }

    #[test]
    fn is_url_should_return_false_for_windows_path() {
        assert!(!is_url("C:/Users/user/video.mp4"));
    }

    // sanitize_url

    #[test]
    fn sanitize_url_should_strip_password_from_rtmp_url() {
        assert_eq!(
            sanitize_url("rtmp://admin:s3cr3t@live.example.com/app/key"),
            "rtmp://admin:***@live.example.com/app/key",
        );
    }

    #[test]
    fn sanitize_url_should_strip_password_and_query_string() {
        assert_eq!(
            sanitize_url("rtmp://admin:s3cr3t@live.example.com/app/key?token=abc"),
            "rtmp://admin:***@live.example.com/app/key",
        );
    }

    #[test]
    fn sanitize_url_should_strip_query_string_without_credentials() {
        assert_eq!(
            sanitize_url("http://cdn.example.com/live.m3u8?token=secret"),
            "http://cdn.example.com/live.m3u8",
        );
    }

    #[test]
    fn sanitize_url_should_leave_url_without_credentials_unchanged() {
        assert_eq!(
            sanitize_url("rtmp://live.example.com/app/key"),
            "rtmp://live.example.com/app/key",
        );
    }

    #[test]
    fn sanitize_url_should_handle_username_without_password() {
        // "user@host" — no colon, so user info is kept as-is.
        assert_eq!(
            sanitize_url("rtmp://admin@live.example.com/app/key"),
            "rtmp://admin@live.example.com/app/key",
        );
    }

    #[test]
    fn sanitize_url_should_return_input_when_no_scheme_found() {
        let raw = "not-a-url";
        assert_eq!(sanitize_url(raw), raw);
    }

    // map_network_error

    #[test]
    fn map_network_error_should_map_etimedout_to_network_timeout() {
        let err = map_network_error(error_codes::ETIMEDOUT, "rtmp://host/app".to_string());
        assert!(matches!(err, DecodeError::NetworkTimeout { .. }));
    }

    #[test]
    fn map_network_error_should_map_econnrefused_to_connection_failed() {
        let err = map_network_error(error_codes::ECONNREFUSED, "rtmp://host/app".to_string());
        assert!(matches!(err, DecodeError::ConnectionFailed { .. }));
    }

    #[test]
    fn map_network_error_should_map_ehostunreach_to_connection_failed() {
        let err = map_network_error(error_codes::EHOSTUNREACH, "rtmp://host/app".to_string());
        assert!(matches!(err, DecodeError::ConnectionFailed { .. }));
    }

    #[test]
    fn map_network_error_should_map_enetunreach_to_connection_failed() {
        let err = map_network_error(error_codes::ENETUNREACH, "rtmp://host/app".to_string());
        assert!(matches!(err, DecodeError::ConnectionFailed { .. }));
    }

    #[test]
    fn map_network_error_should_map_eio_to_stream_interrupted() {
        let err = map_network_error(error_codes::EIO, "rtmp://host/app".to_string());
        assert!(matches!(err, DecodeError::StreamInterrupted { .. }));
    }

    #[test]
    fn map_network_error_should_map_unknown_code_to_ffmpeg() {
        let err = map_network_error(-22, "rtmp://host/app".to_string());
        assert!(matches!(err, DecodeError::Ffmpeg { .. }));
    }

    #[test]
    fn map_network_error_should_include_sanitized_endpoint_in_connection_failed() {
        let endpoint = "rtmp://admin:***@host/app".to_string();
        let err = map_network_error(error_codes::ECONNREFUSED, endpoint.clone());
        match err {
            DecodeError::ConnectionFailed { endpoint: ep, .. } => assert_eq!(ep, endpoint),
            _ => panic!("wrong variant"),
        }
    }
}
