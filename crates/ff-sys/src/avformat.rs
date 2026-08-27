//! AVFormat wrapper functions for libavformat operations.
//!
//! This module provides thin wrapper functions around FFmpeg's libavformat API.
//! All functions are marked unsafe as they involve raw pointer manipulation.
//!
//! # Safety
//!
//! Callers are responsible for:
//! - Ensuring pointers are valid before passing to these functions
//! - Properly freeing resources using the corresponding close/free functions
//! - Not using pointers after they have been freed

use std::ffi::CString;
use std::os::raw::c_int;
use std::path::Path;
use std::ptr;
use std::time::Duration;

use crate::{
    AVFormatContext, AVIOContext, AVPacket, av_read_frame as ffi_av_read_frame,
    av_seek_frame as ffi_av_seek_frame, avformat_find_stream_info as ffi_avformat_find_stream_info,
    avformat_open_input as ffi_avformat_open_input, avformat_seek_file as ffi_avformat_seek_file,
    ensure_initialized,
};
// Only the test-only `close_input` / `write_frame` wrappers use these bindings.
#[cfg(test)]
use crate::{
    av_write_frame as ffi_av_write_frame, avformat_close_input as ffi_avformat_close_input,
};

// FFmpeg I/O functions (declared here as they may not be in bindgen output)
unsafe extern "C" {
    fn avio_open(s: *mut *mut AVIOContext, url: *const std::os::raw::c_char, flags: c_int)
    -> c_int;
    fn avio_closep(s: *mut *mut AVIOContext);
}

/// AVIO flags for opening files.
///
/// These flags control how files are opened for I/O operations.
///
/// # Examples
///
/// ```ignore
/// use ff_sys::avformat::avio_flags;
///
/// // Open file for reading
/// let flags = avio_flags::READ;
///
/// // Open file for writing
/// let flags = avio_flags::WRITE;
/// ```
pub mod avio_flags {
    use std::os::raw::c_int;

    /// Open file for reading.
    pub const READ: c_int = crate::AVIO_FLAG_READ as c_int;

    /// Open file for writing.
    pub const WRITE: c_int = crate::AVIO_FLAG_WRITE as c_int;

    /// Open file for reading and writing.
    pub const READ_WRITE: c_int = crate::AVIO_FLAG_READ_WRITE as c_int;
}

/// Seek flags for av_seek_frame and avformat_seek_file.
///
/// # Flag Combinations
///
/// Flags can be combined using bitwise OR:
///
/// ```ignore
/// use ff_sys::avformat::seek_flags;
///
/// // Seek backward to the nearest keyframe (most common)
/// let flags = seek_flags::BACKWARD;
///
/// // Seek to any frame (not just keyframes) going backward
/// let flags = seek_flags::BACKWARD | seek_flags::ANY;
///
/// // Seek by byte position
/// let flags = seek_flags::BYTE;
///
/// // Seek by frame number
/// let flags = seek_flags::FRAME;
/// ```
pub mod seek_flags {
    /// Seek backward to the nearest keyframe.
    ///
    /// When seeking, find the keyframe at or before the target timestamp.
    /// This is the most commonly used flag for video seeking.
    pub const BACKWARD: i32 = crate::AVSEEK_FLAG_BACKWARD as i32;

    /// Seek by byte position instead of timestamp.
    ///
    /// The timestamp parameter is interpreted as a byte offset in the file.
    /// Not supported by all demuxers.
    pub const BYTE: i32 = crate::AVSEEK_FLAG_BYTE as i32;

    /// Seek to any frame, not just keyframes.
    ///
    /// Allows seeking to non-keyframes, which may result in visual artifacts
    /// until the next keyframe is reached. Useful for precise seeking.
    pub const ANY: i32 = crate::AVSEEK_FLAG_ANY as i32;

    /// Seek by frame number instead of timestamp.
    ///
    /// The timestamp parameter is interpreted as a frame number.
    /// Not supported by all demuxers.
    pub const FRAME: i32 = crate::AVSEEK_FLAG_FRAME as i32;
}

/// Open a media file and read its header.
///
/// This function opens the file at the given path, reads the format header,
/// and optionally finds stream information.
///
/// # Arguments
///
/// * `path` - Path to the media file to open
///
/// # Returns
///
/// Returns a pointer to the allocated AVFormatContext on success,
/// or an FFmpeg error code on failure.
///
/// # Safety
///
/// The returned pointer must be freed using `close_input()` when no longer needed.
///
/// # Errors
///
/// Returns a negative error code if:
/// - The path contains invalid UTF-8 or null bytes
/// - The file cannot be opened
/// - The file format is not recognized
pub(crate) unsafe fn open_input(path: &Path) -> Result<*mut AVFormatContext, c_int> {
    ensure_initialized();

    // Convert path to C string
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return Err(crate::error_codes::EINVAL),
    };

    let c_path = match CString::new(path_str) {
        Ok(s) => s,
        Err(_) => return Err(crate::error_codes::EINVAL),
    };

    let mut ctx: *mut AVFormatContext = ptr::null_mut();

    // Open input file
    // SAFETY: `c_path` is a valid NUL-terminated C string whose binding outlives
    //         this call; `ctx` is a valid out-pointer initialised to null.
    let ret =
        unsafe { ffi_avformat_open_input(&mut ctx, c_path.as_ptr(), ptr::null(), ptr::null_mut()) };

    if ret < 0 {
        return Err(ret);
    }

    Ok(ctx)
}

/// Open a network URL with connect/read timeout options.
///
/// Builds an `AVDictionary` with `timeout` (connect) and `rw_timeout` (read/write)
/// keys set to the corresponding microsecond values, then calls
/// `avformat_open_input`. Both keys are freed via `av_dict_free` regardless of
/// whether the open succeeds.
///
/// The returned pointer must be freed using `close_input()`.
///
/// # Errors
///
/// Returns a negative `FFmpeg` error code if the URL is invalid, the host is
/// unreachable, the connection times out, or the format is not recognised.
///
/// # Safety
///
/// The caller must call `close_input()` on the returned context when done.
pub(crate) unsafe fn open_input_url(
    url: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<*mut AVFormatContext, c_int> {
    ensure_initialized();

    let c_url = CString::new(url).map_err(|_| crate::error_codes::EINVAL)?;

    // Build AVDictionary with timeout options.
    let mut opts: *mut crate::AVDictionary = ptr::null_mut();
    // SAFETY: string literals and computed strings have no null bytes.
    let timeout_key = CString::new("timeout").expect("no null in literal");
    let rw_timeout_key = CString::new("rw_timeout").expect("no null in literal");
    let timeout_val = CString::new(connect_timeout.as_micros().to_string())
        .expect("decimal string has no null bytes");
    let rw_timeout_val = CString::new(read_timeout.as_micros().to_string())
        .expect("decimal string has no null bytes");

    // SAFETY: av_dict_set does not retain the C strings after the call;
    //         opts is initialised to null and is populated by av_dict_set.
    unsafe {
        crate::av_dict_set(
            ptr::addr_of_mut!(opts),
            timeout_key.as_ptr(),
            timeout_val.as_ptr(),
            0,
        );
        crate::av_dict_set(
            ptr::addr_of_mut!(opts),
            rw_timeout_key.as_ptr(),
            rw_timeout_val.as_ptr(),
            0,
        );

        // UDP-specific options: enlarge the receive buffer and suppress
        // overrun errors so that high-bitrate MPEG-TS streams do not drop
        // entire frames due to transient kernel-buffer exhaustion.
        if url.starts_with("udp://") {
            let buf_key = CString::new("buffer_size").expect("no null in literal");
            let buf_val = CString::new("65536").expect("no null in literal");
            let overrun_key = CString::new("overrun_nonfatal").expect("no null in literal");
            let overrun_val = CString::new("1").expect("no null in literal");
            crate::av_dict_set(
                ptr::addr_of_mut!(opts),
                buf_key.as_ptr(),
                buf_val.as_ptr(),
                0,
            );
            crate::av_dict_set(
                ptr::addr_of_mut!(opts),
                overrun_key.as_ptr(),
                overrun_val.as_ptr(),
                0,
            );
        }
    }

    let mut ctx: *mut AVFormatContext = ptr::null_mut();
    // SAFETY: c_url is a valid C string; opts is valid or null.
    let ret = unsafe { ffi_avformat_open_input(&mut ctx, c_url.as_ptr(), ptr::null(), &mut opts) };

    // Free any options FFmpeg did not consume.
    // SAFETY: opts is either null or allocated by av_dict_set above.
    if !opts.is_null() {
        unsafe { crate::av_dict_free(ptr::addr_of_mut!(opts)) };
    }

    if ret < 0 {
        return Err(ret);
    }
    Ok(ctx)
}

/// Returns `true` when the `libsrt` protocol is available in the linked FFmpeg build.
///
/// Calls `av_find_input_format("libsrt")` at runtime. When FFmpeg was built
/// without libsrt support this returns `false`.
pub fn srt_available() -> bool {
    ensure_initialized();
    // SAFETY: string literal has no interior null bytes.
    let name = CString::new("libsrt").unwrap();
    let fmt = unsafe { crate::av_find_input_format(name.as_ptr()) };
    !fmt.is_null()
}

/// Open an image sequence using the `image2` demuxer.
///
/// Sets `framerate` in the demuxer options so FFmpeg assigns the correct PTS
/// to each frame. The returned pointer must be freed using `close_input()`.
///
/// # Errors
///
/// Returns a negative error code if the path is invalid, the sequence cannot
/// be opened, or the `image2` demuxer is unavailable.
///
/// # Safety
///
/// The caller must call `close_input()` on the returned context when done.
pub(crate) unsafe fn open_input_image_sequence(
    path: &Path,
    framerate: u32,
) -> Result<*mut AVFormatContext, c_int> {
    ensure_initialized();

    let path_str = match path.to_str() {
        Some(s) => s,
        None => return Err(crate::error_codes::EINVAL),
    };
    let c_path = match CString::new(path_str) {
        Ok(s) => s,
        Err(_) => return Err(crate::error_codes::EINVAL),
    };

    // Locate the image2 demuxer.  Always present in standard FFmpeg builds;
    // passing null falls back to FFmpeg's auto-detection from file extension.
    // SAFETY: string literal has no null bytes
    let image2_name = CString::new("image2").unwrap();
    // SAFETY: `image2_name` is a valid C string whose binding outlives the call.
    let input_fmt = unsafe { crate::av_find_input_format(image2_name.as_ptr()) };

    // Build options dictionary: framerate=<n>
    let mut opts: *mut crate::AVDictionary = ptr::null_mut();
    // SAFETY: string literals have no null bytes
    let framerate_key = CString::new("framerate").unwrap();
    let framerate_str = CString::new(framerate.to_string()).unwrap();
    // SAFETY: `opts` is a valid dictionary out-pointer initialised to null; the
    //         key/value C string bindings outlive the call and are not retained
    //         by FFmpeg after it returns.
    unsafe {
        crate::av_dict_set(
            ptr::addr_of_mut!(opts),
            framerate_key.as_ptr(),
            framerate_str.as_ptr(),
            0,
        );
    }

    let mut ctx: *mut AVFormatContext = ptr::null_mut();
    // SAFETY: `c_path` is a valid C string whose binding outlives the call;
    //         `input_fmt` is null or a valid demuxer; `opts` and `ctx` are valid
    //         out-pointers.
    let ret = unsafe { ffi_avformat_open_input(&mut ctx, c_path.as_ptr(), input_fmt, &mut opts) };

    // Free any options that FFmpeg did not consume.
    if !opts.is_null() {
        // SAFETY: `opts` is non-null (checked) and was allocated by `av_dict_set`.
        unsafe {
            crate::av_dict_free(ptr::addr_of_mut!(opts));
        }
    }

    if ret < 0 {
        return Err(ret);
    }
    Ok(ctx)
}

/// Close an opened media file and free its resources.
///
/// This function closes the input file, frees the format context and all its
/// contents, and sets the context pointer to null.
///
/// # Arguments
///
/// * `ctx` - Pointer to a pointer to the AVFormatContext to close
///
/// # Safety
///
/// - The context must have been allocated by `open_input()` or `avformat_alloc_context()`.
/// - After this call, `*ctx` will be set to null by FFmpeg.
/// - The context pointer must not be used after this call.
///
/// # Null Safety
///
/// This function safely handles:
/// - `ctx` being null
/// - `*ctx` being null
#[cfg(test)]
pub(crate) unsafe fn close_input(ctx: *mut *mut AVFormatContext) {
    // SAFETY: The caller guarantees `ctx` is either null or a valid pointer to a
    //         context pointer allocated by `open_input`. Dereferencing `ctx` is
    //         guarded by the null check, and `avformat_close_input` tolerates a
    //         null `*ctx`.
    unsafe {
        if !ctx.is_null() && !(*ctx).is_null() {
            ffi_avformat_close_input(ctx);
        }
    }
}

/// Read the stream information from a media file.
///
/// This function populates stream information in the format context.
/// Should be called after `open_input()` to get detailed codec information.
///
/// # Arguments
///
/// * `ctx` - The format context to read stream info for
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// The context must be a valid pointer from `open_input()`.
///
/// # Errors
///
/// Returns a negative error code if stream information cannot be read.
pub(crate) unsafe fn find_stream_info(ctx: *mut AVFormatContext) -> Result<(), c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` is non-null (checked) and a valid context from `open_input`.
    let ret = unsafe { ffi_avformat_find_stream_info(ctx, ptr::null_mut()) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Seek to a specified timestamp in the stream.
///
/// # Arguments
///
/// * `ctx` - The format context
/// * `stream_index` - Index of the stream to seek in, or -1 for default
/// * `timestamp` - Target timestamp in stream time base units
/// * `flags` - Seek flags (see `seek_flags` module)
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// The context must be a valid pointer from `open_input()`.
///
/// # Errors
///
/// Returns a negative error code if seeking fails.
pub(crate) unsafe fn seek_frame(
    ctx: *mut AVFormatContext,
    stream_index: c_int,
    timestamp: i64,
    flags: c_int,
) -> Result<(), c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` is non-null (checked) and a valid context from `open_input`.
    let ret = unsafe { ffi_av_seek_frame(ctx, stream_index, timestamp, flags) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Seek to a specified timestamp with min/max bounds.
///
/// This is a more precise seeking function that allows specifying
/// minimum and maximum acceptable timestamps.
///
/// # Arguments
///
/// * `ctx` - The format context
/// * `stream_index` - Index of the stream to seek in, or -1 for default
/// * `min_ts` - Minimum acceptable timestamp
/// * `ts` - Target timestamp
/// * `max_ts` - Maximum acceptable timestamp
/// * `flags` - Seek flags (see `seek_flags` module)
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// The context must be a valid pointer from `open_input()`.
///
/// # Errors
///
/// Returns a negative error code if seeking fails.
pub(crate) unsafe fn seek_file(
    ctx: *mut AVFormatContext,
    stream_index: c_int,
    min_ts: i64,
    ts: i64,
    max_ts: i64,
    flags: c_int,
) -> Result<(), c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` is non-null (checked) and a valid context from `open_input`.
    let ret = unsafe { ffi_avformat_seek_file(ctx, stream_index, min_ts, ts, max_ts, flags) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Read the next frame of a stream.
///
/// This function reads the next packet from the stream and stores it
/// in the provided packet structure.
///
/// # Arguments
///
/// * `ctx` - The format context
/// * `pkt` - Pointer to the packet to fill
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
/// Returns `error_codes::EOF` when the end of file is reached.
///
/// # Safety
///
/// - The context must be a valid pointer from `open_input()`.
/// - The packet must be allocated and initialized.
///
/// # Errors
///
/// Returns a negative error code if:
/// - Reading fails
/// - End of file is reached (`error_codes::EOF`)
pub(crate) unsafe fn read_frame(
    ctx: *mut AVFormatContext,
    pkt: *mut AVPacket,
) -> Result<(), c_int> {
    if ctx.is_null() || pkt.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` and `pkt` are non-null (checked); `ctx` is a valid context
    //         from `open_input` and `pkt` is an allocated, initialised packet.
    let ret = unsafe { ffi_av_read_frame(ctx, pkt) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Write a frame to an output media file.
///
/// This function writes an encoded packet to the output stream.
///
/// # Arguments
///
/// * `ctx` - The output format context
/// * `pkt` - Pointer to the packet to write
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// - The context must be a valid output format context.
/// - The packet must contain valid encoded data.
///
/// # Errors
///
/// Returns a negative error code if:
/// - Context or packet is null (`error_codes::EINVAL`)
/// - Writing fails
#[cfg(test)]
pub(crate) unsafe fn write_frame(
    ctx: *mut AVFormatContext,
    pkt: *mut AVPacket,
) -> Result<(), c_int> {
    if ctx.is_null() || pkt.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` and `pkt` are non-null (checked); `ctx` is a valid output
    //         format context and `pkt` holds valid encoded data.
    let ret = unsafe { ffi_av_write_frame(ctx, pkt) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

// ============================================================================
// Output file I/O operations
// ============================================================================

/// Open a file for output.
///
/// This function opens a file for writing media data. The file must be
/// associated with an allocated format context.
///
/// # Arguments
///
/// * `path` - Path to the output file
/// * `flags` - Open flags (see `avio_flags` module)
///
/// # Returns
///
/// Returns a pointer to the allocated `AVIOContext` on success,
/// or an FFmpeg error code on failure.
///
/// # Safety
///
/// The returned pointer must be freed using `close_output()` when no longer needed.
///
/// # Errors
///
/// Returns a negative error code if:
/// - The path contains invalid UTF-8 or null bytes (`error_codes::EINVAL`)
/// - The file cannot be opened for writing
/// - Memory allocation fails
///
/// # Examples
///
/// ```ignore
/// use ff_sys::avformat::{open_output, avio_flags};
///
/// unsafe {
///     let pb = open_output("/path/to/output.mp4", avio_flags::WRITE)?;
///     // Use pb...
///     close_output(pb);
/// }
/// ```
pub(crate) unsafe fn open_output(path: &Path, flags: c_int) -> Result<*mut AVIOContext, c_int> {
    ensure_initialized();

    // Convert path to C string
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return Err(crate::error_codes::EINVAL),
    };

    let c_path = match CString::new(path_str) {
        Ok(s) => s,
        Err(_) => return Err(crate::error_codes::EINVAL),
    };

    let mut pb: *mut AVIOContext = ptr::null_mut();

    // Open output file
    // SAFETY: `c_path` is a valid C string whose binding outlives the call; `pb`
    //         is a valid out-pointer initialised to null.
    let ret = unsafe { avio_open(&mut pb, c_path.as_ptr(), flags) };

    if ret < 0 {
        return Err(ret);
    }

    Ok(pb)
}

/// Close an output file and free its resources.
///
/// This function closes the output file, flushes any buffered data,
/// frees the I/O context, and sets the pointer to null.
///
/// # Arguments
///
/// * `pb` - Pointer to a pointer to the `AVIOContext` to close
///
/// # Safety
///
/// - The context must have been allocated by `open_output()` or `avio_open()`.
/// - After this call, `*pb` will be set to null by FFmpeg.
/// - The context pointer must not be used after this call.
///
/// # Null Safety
///
/// This function safely handles:
/// - `pb` being null
/// - `*pb` being null
///
/// # Examples
///
/// ```ignore
/// use ff_sys::avformat::{open_output, close_output, avio_flags};
///
/// unsafe {
///     let mut pb = open_output("/path/to/output.mp4", avio_flags::WRITE)?;
///     // Write data...
///     close_output(&mut pb);
///     assert!(pb.is_null());
/// }
/// ```
pub(crate) unsafe fn close_output(pb: *mut *mut AVIOContext) {
    // SAFETY: The caller guarantees `pb` is either null or a valid pointer to an
    //         AVIOContext pointer from `open_output`. Dereferencing `pb` is
    //         guarded by the null check, and `avio_closep` tolerates a null `*pb`.
    unsafe {
        if !pb.is_null() && !(*pb).is_null() {
            avio_closep(pb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seek_flags() {
        // Verify seek flags are defined correctly
        assert_eq!(seek_flags::BACKWARD, 1);
        assert_eq!(seek_flags::BYTE, 2);
        assert_eq!(seek_flags::ANY, 4);
        assert_eq!(seek_flags::FRAME, 8);
    }

    #[test]
    fn test_open_input_invalid_path() {
        // Test that opening a non-existent file returns an error
        let path = Path::new("/nonexistent/path/to/file.mp4");
        unsafe {
            let result = open_input(path);
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_close_input_null_safety() {
        // Test that close_input handles null pointers safely
        unsafe {
            // Passing a null pointer should not crash
            close_input(ptr::null_mut());

            // Passing a pointer to null should not crash
            let mut null_ctx: *mut AVFormatContext = ptr::null_mut();
            close_input(&mut null_ctx);
        }
    }

    #[test]
    fn test_find_stream_info_null() {
        // Test that find_stream_info rejects null pointers
        unsafe {
            let result = find_stream_info(ptr::null_mut());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_seek_frame_null() {
        // Test that seek_frame rejects null pointers
        unsafe {
            let result = seek_frame(ptr::null_mut(), 0, 0, 0);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_seek_file_null() {
        // Test that seek_file rejects null pointers
        unsafe {
            let result = seek_file(ptr::null_mut(), 0, 0, 0, 0, 0);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_read_frame_null() {
        // Test that read_frame rejects null pointers
        unsafe {
            let result = read_frame(ptr::null_mut(), ptr::null_mut());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_write_frame_null() {
        // Test that write_frame rejects null context and null packet
        unsafe {
            // Both null
            let result = write_frame(ptr::null_mut(), ptr::null_mut());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_avio_flags() {
        // Verify AVIO flags are defined
        assert!(avio_flags::READ >= 0);
        assert!(avio_flags::WRITE >= 0);
        assert!(avio_flags::READ_WRITE >= 0);

        // WRITE flag should be non-zero
        assert!(avio_flags::WRITE > 0);
    }

    #[test]
    fn test_open_output_invalid_path() {
        // Test that opening with invalid path returns an error
        let path = Path::new("/nonexistent/directory/output.mp4");
        unsafe {
            let result = open_output(path, avio_flags::WRITE);
            // Should return error (directory doesn't exist)
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_close_output_null_safety() {
        // Test that close_output handles null pointers safely
        unsafe {
            // Passing a null pointer should not crash
            close_output(ptr::null_mut());

            // Passing a pointer to null should not crash
            let mut null_pb: *mut AVIOContext = ptr::null_mut();
            close_output(&mut null_pb);
        }
    }
}
