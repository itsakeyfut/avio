//! Shared RAII guards over raw `FFmpeg` pointers used by the audio, image, and
//! video decoders.
//!
//! Each guard owns a `*mut` `FFmpeg` handle and frees it in `Drop`. The
//! constructors are the union of what the three decoders need — every method is
//! used by at least one decoder. All `unsafe` is isolated here per the project's
//! unsafe-code convention (`*_inner.rs` files only).

#![allow(unsafe_code)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::doc_markdown)]

use std::path::Path;

use ff_format::NetworkOptions;
use ff_sys::{AVCodecContext, AVFormatContext, AVFrame, AVPacket};

use crate::error::DecodeError;
use crate::network::{map_network_error, sanitize_url};

/// RAII guard for `AVFormatContext` to ensure proper cleanup.
pub(crate) struct AvFormatContextGuard(*mut AVFormatContext);

impl AvFormatContextGuard {
    /// Creates a new guard by opening an input file.
    ///
    /// # Safety
    ///
    /// Caller must ensure FFmpeg is initialized and path is valid.
    pub(crate) unsafe fn new(path: &Path) -> Result<Self, DecodeError> {
        // SAFETY: Caller ensures FFmpeg is initialized and path is valid
        let format_ctx = unsafe {
            ff_sys::avformat::open_input(path).map_err(|e| DecodeError::Ffmpeg {
                code: e,
                message: format!("Failed to open file: {}", ff_sys::av_error_string(e)),
            })?
        };
        Ok(Self(format_ctx))
    }

    /// Opens an image sequence using the `image2` demuxer.
    ///
    /// # Safety
    ///
    /// Caller must ensure FFmpeg is initialized and path is valid.
    pub(crate) unsafe fn new_image_sequence(
        path: &Path,
        framerate: u32,
    ) -> Result<Self, DecodeError> {
        // SAFETY: Caller ensures FFmpeg is initialized and path is a valid image-sequence pattern
        let format_ctx = unsafe {
            ff_sys::avformat::open_input_image_sequence(path, framerate).map_err(|e| {
                DecodeError::Ffmpeg {
                    code: e,
                    message: format!(
                        "Failed to open image sequence: {}",
                        ff_sys::av_error_string(e)
                    ),
                }
            })?
        };
        Ok(Self(format_ctx))
    }

    /// Opens a network URL using the supplied network options.
    ///
    /// # Safety
    ///
    /// Caller must ensure FFmpeg is initialized and URL is valid.
    pub(crate) unsafe fn new_url(url: &str, network: &NetworkOptions) -> Result<Self, DecodeError> {
        // SAFETY: Caller ensures FFmpeg is initialized and URL is valid
        let format_ctx = unsafe {
            ff_sys::avformat::open_input_url(url, network.connect_timeout, network.read_timeout)
                .map_err(|e| map_network_error(e, sanitize_url(url)))?
        };
        Ok(Self(format_ctx))
    }

    /// Returns the raw pointer.
    pub(crate) const fn as_ptr(&self) -> *mut AVFormatContext {
        self.0
    }

    /// Consumes the guard and returns the raw pointer without dropping.
    pub(crate) fn into_raw(self) -> *mut AVFormatContext {
        let ptr = self.0;
        std::mem::forget(self);
        ptr
    }
}

impl Drop for AvFormatContextGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 is valid and owned by this guard
            unsafe {
                ff_sys::avformat::close_input(&mut (self.0 as *mut _));
            }
        }
    }
}

/// RAII guard for `AVCodecContext` to ensure proper cleanup.
pub(crate) struct AvCodecContextGuard(*mut AVCodecContext);

impl AvCodecContextGuard {
    /// Creates a new guard by allocating a codec context.
    ///
    /// # Safety
    ///
    /// Caller must ensure codec pointer is valid.
    pub(crate) unsafe fn new(codec: *const ff_sys::AVCodec) -> Result<Self, DecodeError> {
        // SAFETY: Caller ensures codec pointer is valid
        let codec_ctx = unsafe {
            ff_sys::avcodec::alloc_context3(codec).map_err(|e| DecodeError::Ffmpeg {
                code: e,
                message: format!("Failed to allocate codec context: {e}"),
            })?
        };
        Ok(Self(codec_ctx))
    }

    /// Returns the raw pointer.
    pub(crate) const fn as_ptr(&self) -> *mut AVCodecContext {
        self.0
    }

    /// Consumes the guard and returns the raw pointer without dropping.
    pub(crate) fn into_raw(self) -> *mut AVCodecContext {
        let ptr = self.0;
        std::mem::forget(self);
        ptr
    }
}

impl Drop for AvCodecContextGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 is valid and owned by this guard
            unsafe {
                ff_sys::avcodec::free_context(&mut (self.0 as *mut _));
            }
        }
    }
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
