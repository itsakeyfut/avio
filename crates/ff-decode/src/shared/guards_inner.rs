//! Shared RAII guards over raw `FFmpeg` pointers used by the audio, image, and
//! video decoders, plus input-context open helpers.
//!
//! Each guard owns a `*mut` `FFmpeg` handle and frees it in `Drop`. The
//! constructors are the union of what the three decoders need — every method is
//! used by at least one decoder. The open helpers wrap
//! [`ff_sys::InputFormatContext`] constructors with the decoders' `DecodeError`
//! mapping. All `unsafe` is isolated here per the project's unsafe-code
//! convention (`*_inner.rs` files only).

#![allow(unsafe_code)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::doc_markdown)]

use std::path::Path;

use ff_format::NetworkOptions;
use ff_sys::{AVFrame, AVPacket};

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

/// Opens a network URL with the supplied network options, returning the owned demux context.
pub(crate) fn open_url_ctx(
    url: &str,
    network: &NetworkOptions,
) -> Result<ff_sys::InputFormatContext, DecodeError> {
    ff_sys::InputFormatContext::open_url(url, network.connect_timeout, network.read_timeout)
        .map_err(|e| map_network_error(e.code(), sanitize_url(url)))
}

/// RAII guard for `AVPacket` to ensure proper cleanup.
pub(crate) struct AvPacketGuard(*mut AVPacket);

impl AvPacketGuard {
    /// Creates a new guard by allocating a packet.
    ///
    /// # Safety
    ///
    /// Must be called after FFmpeg initialization.
    pub(crate) unsafe fn new() -> Result<Self, DecodeError> {
        // SAFETY: Caller ensures FFmpeg is initialized
        let packet = unsafe { ff_sys::av_packet_alloc() };
        if packet.is_null() {
            return Err(DecodeError::Ffmpeg {
                code: 0,
                message: "Failed to allocate packet".to_string(),
            });
        }
        Ok(Self(packet))
    }

    /// Consumes the guard and returns the raw pointer without dropping.
    pub(crate) fn into_raw(self) -> *mut AVPacket {
        let ptr = self.0;
        std::mem::forget(self);
        ptr
    }
}

impl Drop for AvPacketGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 is valid and owned by this guard
            unsafe {
                ff_sys::av_packet_free(&mut (self.0 as *mut _));
            }
        }
    }
}

/// RAII guard for `AVFrame` to ensure proper cleanup.
pub(crate) struct AvFrameGuard(*mut AVFrame);

impl AvFrameGuard {
    /// Creates a new guard by allocating a frame.
    ///
    /// # Safety
    ///
    /// Must be called after FFmpeg initialization.
    pub(crate) unsafe fn new() -> Result<Self, DecodeError> {
        // SAFETY: Caller ensures FFmpeg is initialized
        let frame = unsafe { ff_sys::av_frame_alloc() };
        if frame.is_null() {
            return Err(DecodeError::Ffmpeg {
                code: 0,
                message: "Failed to allocate frame".to_string(),
            });
        }
        Ok(Self(frame))
    }

    /// Returns the raw pointer.
    pub(crate) const fn as_ptr(&self) -> *mut AVFrame {
        self.0
    }

    /// Consumes the guard and returns the raw pointer without dropping.
    pub(crate) fn into_raw(self) -> *mut AVFrame {
        let ptr = self.0;
        std::mem::forget(self);
        ptr
    }
}

impl Drop for AvFrameGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 is valid and owned by this guard
            unsafe {
                ff_sys::av_frame_free(&mut (self.0 as *mut _));
            }
        }
    }
}
