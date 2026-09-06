//! Input-context open helpers for the audio, image, and video decoders.
//!
//! The open helpers wrap [`ff_sys::InputFormatContext`] constructors with the
//! decoders' `DecodeError` mapping. All `unsafe` is isolated here per the
//! project's unsafe-code convention (`*_inner.rs` files only).

#![allow(unsafe_code)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::doc_markdown)]

use std::path::Path;

use ff_format::NetworkOptions;

use crate::error::DecodeError;
use crate::network::{map_network_error, sanitize_url};

/// Opens an input file, returning the owned demux context.
pub(crate) fn open_input_ctx(path: &Path) -> Result<ff_sys::InputFormatContext, DecodeError> {
    ff_sys::InputFormatContext::open(path).map_err(|e| DecodeError::Ffmpeg {
        code: e.code(),
        message: format!("Failed to open file: {}", ff_sys::av_error_string(e.code())),
    })
}

/// Opens an image sequence via the `image2` demuxer, returning the owned demux context.
pub(crate) fn open_image_sequence_ctx(
    path: &Path,
    framerate: u32,
) -> Result<ff_sys::InputFormatContext, DecodeError> {
    ff_sys::InputFormatContext::open_image_sequence(path, framerate).map_err(|e| {
        DecodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Failed to open image sequence: {}",
                ff_sys::av_error_string(e.code())
            ),
        }
    })
}

/// Opens a caller-supplied byte source, returning the owned demux context.
///
/// The format is probed from the bytes themselves, so there is no path or URL to
/// report in the error; the source is moved into the context and released with it.
pub(crate) fn open_custom_ctx(
    source: Box<dyn ff_sys::IoSource>,
) -> Result<ff_sys::InputFormatContext, DecodeError> {
    ff_sys::InputFormatContext::open_custom(source).map_err(|e| DecodeError::Ffmpeg {
        code: e.code(),
        message: format!(
            "Failed to open the supplied source: {}",
            ff_sys::av_error_string(e.code())
        ),
    })
}

/// Opens a network URL with the supplied network options, returning the owned demux context.
pub(crate) fn open_url_ctx(
    url: &str,
    network: &NetworkOptions,
) -> Result<ff_sys::InputFormatContext, DecodeError> {
    ff_sys::InputFormatContext::open_url(url, network.connect_timeout, network.read_timeout)
        .map_err(|e| map_network_error(e.code(), sanitize_url(url)))
}
