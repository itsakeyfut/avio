//! AVCodec wrapper functions for libavcodec operations.
//!
//! This module provides thin wrapper functions around FFmpeg's libavcodec API
//! for encoding and decoding audio/video data.
//!
//! # Safety
//!
//! Callers are responsible for:
//! - Ensuring pointers are valid before passing to these functions
//! - Properly freeing resources using the corresponding free functions
//! - Not using pointers after they have been freed
//! - Calling functions in the correct order (e.g., alloc before open, open before send/receive)

use std::os::raw::c_int;

use crate::{
    AVCodec, AVCodecContext, AVCodecID, AVCodecParameters, AVDictionary, AVFrame, AVPacket,
    avcodec_alloc_context3 as ffi_avcodec_alloc_context3,
    avcodec_find_decoder as ffi_avcodec_find_decoder,
    avcodec_find_decoder_by_name as ffi_avcodec_find_decoder_by_name,
    avcodec_find_encoder as ffi_avcodec_find_encoder,
    avcodec_find_encoder_by_name as ffi_avcodec_find_encoder_by_name,
    avcodec_flush_buffers as ffi_avcodec_flush_buffers, avcodec_open2 as ffi_avcodec_open2,
    avcodec_parameters_from_context as ffi_avcodec_parameters_from_context,
    avcodec_parameters_to_context as ffi_avcodec_parameters_to_context,
    avcodec_receive_frame as ffi_avcodec_receive_frame,
    avcodec_receive_packet as ffi_avcodec_receive_packet,
    avcodec_send_frame as ffi_avcodec_send_frame, avcodec_send_packet as ffi_avcodec_send_packet,
    ensure_initialized,
};

// ============================================================================
// Codec lookup functions
// ============================================================================

/// Find a decoder by codec ID.
///
/// # Arguments
///
/// * `codec_id` - The codec ID to search for
///
/// # Returns
///
/// Returns `Some(pointer)` to the codec if found, `None` if no decoder exists
/// for the specified codec ID.
///
/// # Safety
///
/// The returned pointer is owned by FFmpeg and must not be freed.
/// It remains valid for the lifetime of the program.
pub unsafe fn find_decoder(codec_id: AVCodecID) -> Option<*const AVCodec> {
    ensure_initialized();

    // SAFETY: `ffi_avcodec_find_decoder` is a pure table lookup that takes a codec
    // ID by value and dereferences no caller pointer; it always returns either a
    // valid static codec pointer or null.
    let codec = unsafe { ffi_avcodec_find_decoder(codec_id) };
    if codec.is_null() { None } else { Some(codec) }
}

/// Find a decoder by name.
///
/// # Arguments
///
/// * `name` - The codec name to search for (e.g., "h264", "aac")
///
/// # Returns
///
/// Returns `Some(pointer)` to the codec if found, `None` if no decoder exists
/// with the specified name.
///
/// # Safety
///
/// - The name must be a valid null-terminated C string.
/// - The returned pointer is owned by FFmpeg and must not be freed.
pub unsafe fn find_decoder_by_name(name: *const i8) -> Option<*const AVCodec> {
    ensure_initialized();

    if name.is_null() {
        return None;
    }

    // SAFETY: `name` was null-checked just above, so it is non-null here; the caller
    // guarantees it is a valid null-terminated C string. The call reads only that
    // string and returns a valid static codec pointer or null.
    let codec = unsafe { ffi_avcodec_find_decoder_by_name(name) };
    if codec.is_null() { None } else { Some(codec) }
}

/// Find an encoder by codec ID.
///
/// # Arguments
///
/// * `codec_id` - The codec ID to search for
///
/// # Returns
///
/// Returns `Some(pointer)` to the codec if found, `None` if no encoder exists
/// for the specified codec ID.
///
/// # Safety
///
/// The returned pointer is owned by FFmpeg and must not be freed.
/// It remains valid for the lifetime of the program.
pub unsafe fn find_encoder(codec_id: AVCodecID) -> Option<*const AVCodec> {
    ensure_initialized();

    // SAFETY: `ffi_avcodec_find_encoder` is a pure table lookup that takes a codec
    // ID by value and dereferences no caller pointer; it always returns either a
    // valid static codec pointer or null.
    let codec = unsafe { ffi_avcodec_find_encoder(codec_id) };
    if codec.is_null() { None } else { Some(codec) }
}

/// Find an encoder by name.
///
/// # Arguments
///
/// * `name` - The codec name to search for (e.g., "libx264", "aac")
///
/// # Returns
///
/// Returns `Some(pointer)` to the codec if found, `None` if no encoder exists
/// with the specified name.
///
/// # Safety
///
/// - The name must be a valid null-terminated C string.
/// - The returned pointer is owned by FFmpeg and must not be freed.
pub unsafe fn find_encoder_by_name(name: *const i8) -> Option<*const AVCodec> {
    ensure_initialized();

    if name.is_null() {
        return None;
    }

    // SAFETY: `name` was null-checked just above, so it is non-null here; the caller
    // guarantees it is a valid null-terminated C string. The call reads only that
    // string and returns a valid static codec pointer or null.
    let codec = unsafe { ffi_avcodec_find_encoder_by_name(name) };
    if codec.is_null() { None } else { Some(codec) }
}

// ============================================================================
// Context allocation and management
// ============================================================================

/// Allocate a codec context for the given codec.
///
/// # Arguments
///
/// * `codec` - The codec to create a context for, or null for a generic context
///
/// # Returns
///
/// Returns a pointer to the allocated context on success,
/// or an error code on failure (typically `ENOMEM`).
///
/// # Safety
///
/// The returned context must be freed with `crate::avcodec_free_context` (or
/// wrapped in [`CodecContext`](crate::CodecContext)) when no longer needed.
pub unsafe fn alloc_context3(codec: *const AVCodec) -> Result<*mut AVCodecContext, c_int> {
    ensure_initialized();

    // SAFETY: `codec` is either null (allowed, yields a generic context) or a valid
    // codec pointer as guaranteed by the caller; the call only reads it and returns
    // a freshly allocated context or null on allocation failure.
    let ctx = unsafe { ffi_avcodec_alloc_context3(codec) };
    if ctx.is_null() {
        Err(crate::error_codes::ENOMEM)
    } else {
        Ok(ctx)
    }
}

/// Copy codec parameters from an open codec context to a stream's `codecpar`.
///
/// Must be called **after** `avcodec_open2` so that the codec has had a chance
/// to populate extradata (e.g. FLAC STREAMINFO, AAC AudioSpecificConfig).
/// Using this function instead of manual field copies ensures that extradata
/// and all other codec-specific fields are transferred correctly.
///
/// # Arguments
///
/// * `par` - The destination `AVCodecParameters` (e.g. from `AVStream.codecpar`)
/// * `ctx` - The source codec context (must be open)
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// Both pointers must be valid and non-null.
pub unsafe fn parameters_from_context(
    par: *mut AVCodecParameters,
    ctx: *const AVCodecContext,
) -> Result<(), c_int> {
    if par.is_null() || ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: both pointers were null-checked above, so they are non-null; the caller
    // guarantees `par` points to a valid destination and `ctx` to a valid, opened
    // codec context. The call only reads `ctx` and writes into `par`.
    let ret = unsafe { ffi_avcodec_parameters_from_context(par, ctx) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Copy codec parameters from a stream to a codec context.
///
/// # Arguments
///
/// * `ctx` - The destination codec context
/// * `par` - The source codec parameters (typically from AVStream.codecpar)
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// - Both pointers must be valid.
/// - The context must be allocated but not yet opened.
///
/// # Errors
///
/// Returns a negative error code if:
/// - Either pointer is null (`error_codes::EINVAL`)
/// - Copying fails (e.g., unsupported codec parameters)
pub unsafe fn parameters_to_context(
    ctx: *mut AVCodecContext,
    par: *const AVCodecParameters,
) -> Result<(), c_int> {
    if ctx.is_null() || par.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: both pointers were null-checked above, so they are non-null; the caller
    // guarantees `ctx` is an allocated but not-yet-opened context and `par` is valid
    // codec parameters. The call reads `par` and writes into `ctx`.
    let ret = unsafe { ffi_avcodec_parameters_to_context(ctx, par) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

// ============================================================================
// Codec opening
// ============================================================================

/// Open a codec context for encoding or decoding.
///
/// # Arguments
///
/// * `ctx` - The codec context to open
/// * `codec` - The codec to use, or null to use the codec set in the context
/// * `options` - Optional codec options dictionary, or null for defaults
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// - The context must be allocated by `alloc_context3()`.
/// - The context must not already be open.
/// - After opening, codec parameters should not be changed.
///
/// # Errors
///
/// Returns a negative error code if:
/// - Context is null (`error_codes::EINVAL`)
/// - The codec is not found or incompatible
/// - Required codec options are missing
pub unsafe fn open2(
    ctx: *mut AVCodecContext,
    codec: *const AVCodec,
    options: *mut *mut AVDictionary,
) -> Result<(), c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` was null-checked above, so it is non-null; the caller guarantees
    // it is an allocated, not-yet-opened context and that `codec`/`options` are
    // either null or valid. FFmpeg initialises the context in place.
    let ret = unsafe { ffi_avcodec_open2(ctx, codec, options) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

// ============================================================================
// Decoding functions (send packet, receive frame)
// ============================================================================

/// Send a packet to the decoder.
///
/// Supply raw packet data as input to a decoder.
///
/// # Arguments
///
/// * `ctx` - The decoder context
/// * `pkt` - The packet containing encoded data, or null to flush the decoder
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error code on failure.
///
/// # Error Codes
///
/// - `error_codes::EAGAIN` - Input is not accepted in the current state.
///   User must read output with `receive_frame()` before sending new input.
/// - `error_codes::EOF` - The decoder has been flushed and no new packets can be sent.
/// - Other negative values indicate an error.
///
/// # Safety
///
/// - The context must be a valid, open decoder context.
/// - The packet must contain valid encoded data or be null for flushing.
pub unsafe fn send_packet(ctx: *mut AVCodecContext, pkt: *const AVPacket) -> Result<(), c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` was null-checked above, so it is non-null; the caller guarantees
    // it is a valid, open decoder context and that `pkt` is either null (flush) or a
    // valid packet. FFmpeg reads the packet and mutates the decoder's internal state.
    let ret = unsafe { ffi_avcodec_send_packet(ctx, pkt) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Receive a decoded frame from the decoder.
///
/// Return decoded output data from a decoder.
///
/// # Arguments
///
/// * `ctx` - The decoder context
/// * `frame` - The frame to fill with decoded data
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error code on failure.
///
/// # Error Codes
///
/// - `error_codes::EAGAIN` - Output is not available in the current state.
///   User must send new input with `send_packet()`.
/// - `error_codes::EOF` - The decoder has been fully flushed and no more frames will be output.
/// - Other negative values indicate an error.
///
/// # Safety
///
/// - The context must be a valid, open decoder context.
/// - The frame must be allocated.
pub unsafe fn receive_frame(ctx: *mut AVCodecContext, frame: *mut AVFrame) -> Result<(), c_int> {
    if ctx.is_null() || frame.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: both pointers were null-checked above, so they are non-null; the caller
    // guarantees `ctx` is a valid, open decoder context and `frame` is an allocated
    // frame. FFmpeg reads the decoder state and writes the decoded frame into `frame`.
    let ret = unsafe { ffi_avcodec_receive_frame(ctx, frame) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

// ============================================================================
// Encoding functions (send frame, receive packet)
// ============================================================================

/// Send a frame to the encoder.
///
/// Supply raw video or audio frame to the encoder.
///
/// # Arguments
///
/// * `ctx` - The encoder context
/// * `frame` - The frame containing raw data, or null to flush the encoder
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error code on failure.
///
/// # Error Codes
///
/// - `error_codes::EAGAIN` - Input is not accepted in the current state.
///   User must read output with `receive_packet()` before sending new input.
/// - `error_codes::EOF` - The encoder has been flushed and no new frames can be sent.
/// - Other negative values indicate an error.
///
/// # Safety
///
/// - The context must be a valid, open encoder context.
/// - The frame must contain valid raw data or be null for flushing.
pub unsafe fn send_frame(ctx: *mut AVCodecContext, frame: *const AVFrame) -> Result<(), c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` was null-checked above, so it is non-null; the caller guarantees
    // it is a valid, open encoder context and that `frame` is either null (flush) or a
    // valid frame. FFmpeg reads the frame and mutates the encoder's internal state.
    let ret = unsafe { ffi_avcodec_send_frame(ctx, frame) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Receive an encoded packet from the encoder.
///
/// Return encoded output data from an encoder.
///
/// # Arguments
///
/// * `ctx` - The encoder context
/// * `pkt` - The packet to fill with encoded data
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error code on failure.
///
/// # Error Codes
///
/// - `error_codes::EAGAIN` - Output is not available in the current state.
///   User must send new input with `send_frame()`.
/// - `error_codes::EOF` - The encoder has been fully flushed and no more packets will be output.
/// - Other negative values indicate an error.
///
/// # Safety
///
/// - The context must be a valid, open encoder context.
/// - The packet must be allocated.
pub unsafe fn receive_packet(ctx: *mut AVCodecContext, pkt: *mut AVPacket) -> Result<(), c_int> {
    if ctx.is_null() || pkt.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: both pointers were null-checked above, so they are non-null; the caller
    // guarantees `ctx` is a valid, open encoder context and `pkt` is an allocated
    // packet. FFmpeg reads the encoder state and writes the encoded packet into `pkt`.
    let ret = unsafe { ffi_avcodec_receive_packet(ctx, pkt) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

// ============================================================================
// Buffer management
// ============================================================================

/// Flush internal codec buffers.
///
/// Reset the internal codec state. Should be called when seeking or switching
/// streams to discard any buffered data.
///
/// # Arguments
///
/// * `ctx` - The codec context to flush
///
/// # Safety
///
/// - The context must be a valid, open codec context.
/// - After flushing, the codec is ready to process new data.
///
/// # Note
///
/// This function does not return an error code as FFmpeg's `avcodec_flush_buffers()`
/// returns void.
pub unsafe fn flush_buffers(ctx: *mut AVCodecContext) {
    // SAFETY: `ctx` is null-checked before use; when non-null the caller guarantees it
    // is a valid, open codec context. FFmpeg only resets the context's internal state.
    unsafe {
        if !ctx.is_null() {
            ffi_avcodec_flush_buffers(ctx);
        }
    }
}

// ============================================================================
// Hardware acceleration helpers
// ============================================================================

/// Hardware acceleration configuration flags.
///
/// These flags can be used to configure hardware-accelerated decoding/encoding.
pub mod hw_config_flags {
    /// The codec supports this format via hardware decoding.
    pub const HW_DEVICE_CTX: i32 = 1;

    /// The codec supports this format as a hardware frame.
    pub const HW_FRAMES_REF: i32 = 2;

    /// This format should be used by default when using hardware acceleration.
    pub const INTERNAL: i32 = 4;

    /// The codec can be used with ad-hoc hardware device.
    pub const AD_HOC: i32 = 8;
}

/// Codec capability flags.
///
/// These flags indicate what capabilities a codec supports.
pub mod codec_caps {
    // Decoding capabilities

    /// Codec is experimental and is thus avoided in favor of stable codecs.
    pub const EXPERIMENTAL: u32 = 1 << 9;

    /// Codec supports hardware acceleration.
    pub const HARDWARE: u32 = 1 << 18;

    /// Codec is backed by a hardware implementation.
    /// Hardware codecs are not usable with software-only context.
    pub const HYBRID: u32 = 1 << 19;

    // Encoding capabilities

    /// Audio encoder supports receiving a different number of samples
    /// in each call.
    pub const VARIABLE_FRAME_SIZE: u32 = 1 << 16;

    /// Decoder is not a preferred choice for probing.
    pub const AVOID_PROBING: u32 = 1 << 17;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AVCodecID_AV_CODEC_ID_H264;
    use std::ffi::CString;
    use std::ptr;

    #[test]
    fn test_find_decoder_h264() {
        // H.264 decoder should be available in any FFmpeg build
        unsafe {
            let codec = find_decoder(AVCodecID_AV_CODEC_ID_H264);
            assert!(codec.is_some(), "H.264 decoder should be found");
        }
    }

    #[test]
    fn test_find_decoder_invalid() {
        // Invalid codec ID should return None
        unsafe {
            // Use a very high value that's unlikely to be a valid codec ID
            let codec = find_decoder(999_999);
            assert!(codec.is_none());
        }
    }

    #[test]
    fn test_find_decoder_by_name() {
        unsafe {
            let name = CString::new("h264").expect("CString creation failed");
            let codec = find_decoder_by_name(name.as_ptr());
            assert!(codec.is_some(), "H.264 decoder should be found by name");
        }
    }

    #[test]
    fn test_find_decoder_by_name_null() {
        unsafe {
            let codec = find_decoder_by_name(ptr::null());
            assert!(codec.is_none());
        }
    }

    #[test]
    fn test_find_encoder_h264() {
        // Note: H.264 encoder might not be available in all FFmpeg builds
        // (libx264 is a separate library), but we test the function works
        unsafe {
            // Just verify the function doesn't crash
            let _codec = find_encoder(AVCodecID_AV_CODEC_ID_H264);
        }
    }

    #[test]
    fn test_find_encoder_by_name_null() {
        unsafe {
            let codec = find_encoder_by_name(ptr::null());
            assert!(codec.is_none());
        }
    }

    #[test]
    fn test_parameters_to_context_null() {
        unsafe {
            // Null context
            let result = parameters_to_context(ptr::null_mut(), ptr::null());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_open2_null() {
        unsafe {
            let result = open2(ptr::null_mut(), ptr::null(), ptr::null_mut());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_send_packet_null() {
        unsafe {
            let result = send_packet(ptr::null_mut(), ptr::null());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_receive_frame_null() {
        unsafe {
            let result = receive_frame(ptr::null_mut(), ptr::null_mut());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_send_frame_null() {
        unsafe {
            let result = send_frame(ptr::null_mut(), ptr::null());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_receive_packet_null() {
        unsafe {
            let result = receive_packet(ptr::null_mut(), ptr::null_mut());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_flush_buffers_null() {
        // Flush with null context should not crash
        unsafe {
            flush_buffers(ptr::null_mut());
        }
    }

    #[test]
    fn test_hw_config_flags() {
        // Verify flags are defined with expected values
        assert_eq!(hw_config_flags::HW_DEVICE_CTX, 1);
        assert_eq!(hw_config_flags::HW_FRAMES_REF, 2);
        assert_eq!(hw_config_flags::INTERNAL, 4);
        assert_eq!(hw_config_flags::AD_HOC, 8);
    }

    #[test]
    fn test_codec_caps() {
        // Verify capability flags are powers of 2
        assert!(codec_caps::EXPERIMENTAL.is_power_of_two());
        assert!(codec_caps::HARDWARE.is_power_of_two());
        assert!(codec_caps::HYBRID.is_power_of_two());
        assert!(codec_caps::VARIABLE_FRAME_SIZE.is_power_of_two());
        assert!(codec_caps::AVOID_PROBING.is_power_of_two());
    }
}
