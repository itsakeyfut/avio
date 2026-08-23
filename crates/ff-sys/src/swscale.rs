//! SwScale wrapper functions for image scaling and pixel format conversion.
//!
//! This module provides thin wrapper functions around FFmpeg's libswscale API
//! for scaling video frames and converting between pixel formats.
//!
//! # Safety
//!
//! Callers are responsible for:
//! - Ensuring pointers are valid before passing to these functions
//! - Properly freeing resources using the corresponding free functions
//! - Not using pointers after they have been freed
//! - Ensuring source and destination buffers match the expected dimensions and formats

use std::os::raw::c_int;

use crate::{
    AVPixelFormat, SwsContext, ensure_initialized, sws_getContext as ffi_sws_getContext,
    sws_isSupportedEndiannessConversion as ffi_sws_isSupportedEndiannessConversion,
    sws_isSupportedInput as ffi_sws_isSupportedInput,
    sws_isSupportedOutput as ffi_sws_isSupportedOutput, sws_scale as ffi_sws_scale,
};

// ============================================================================
// Scaling algorithm flags
// ============================================================================

/// Scaling algorithm flags.
///
/// These flags can be combined with bitwise OR to specify the scaling algorithm
/// and additional options for `get_context()`.
pub mod scale_flags {
    // FFmpeg 7.x (libswscale 8.x): SWS_* are #define macros.
    // FFmpeg 8.x (libswscale 9.x): SWS_* are members of the C enum `SwsFlags`.
    // build.rs detects the version and emits `ffmpeg8` for the latter.

    /// Fast bilinear scaling (low quality, fast).
    ///
    /// Uses a simple bilinear interpolation that is faster but produces
    /// lower quality results. Good for real-time preview.
    #[cfg(ffmpeg8)]
    pub const FAST_BILINEAR: i32 = crate::SwsFlags_SWS_FAST_BILINEAR as i32;
    #[cfg(not(ffmpeg8))]
    pub const FAST_BILINEAR: i32 = crate::SWS_FAST_BILINEAR as i32;

    /// Bilinear scaling (medium quality, medium speed).
    ///
    /// Standard bilinear interpolation. Good balance between speed and quality.
    #[cfg(ffmpeg8)]
    pub const BILINEAR: i32 = crate::SwsFlags_SWS_BILINEAR as i32;
    #[cfg(not(ffmpeg8))]
    pub const BILINEAR: i32 = crate::SWS_BILINEAR as i32;

    /// Bicubic scaling (high quality, slower).
    ///
    /// Bicubic interpolation produces smoother results than bilinear,
    /// but is computationally more expensive.
    #[cfg(ffmpeg8)]
    pub const BICUBIC: i32 = crate::SwsFlags_SWS_BICUBIC as i32;
    #[cfg(not(ffmpeg8))]
    pub const BICUBIC: i32 = crate::SWS_BICUBIC as i32;

    /// Experimental algorithm.
    #[cfg(ffmpeg8)]
    pub const X: i32 = crate::SwsFlags_SWS_X as i32;
    #[cfg(not(ffmpeg8))]
    pub const X: i32 = crate::SWS_X as i32;

    /// Nearest neighbor scaling (no interpolation).
    ///
    /// Fastest algorithm but produces blocky results. Useful for pixel art
    /// or when exact pixel values must be preserved.
    #[cfg(ffmpeg8)]
    pub const POINT: i32 = crate::SwsFlags_SWS_POINT as i32;
    #[cfg(not(ffmpeg8))]
    pub const POINT: i32 = crate::SWS_POINT as i32;

    /// Area averaging scaling.
    ///
    /// Good for downscaling. Averages source pixels that map to each
    /// destination pixel.
    #[cfg(ffmpeg8)]
    pub const AREA: i32 = crate::SwsFlags_SWS_AREA as i32;
    #[cfg(not(ffmpeg8))]
    pub const AREA: i32 = crate::SWS_AREA as i32;

    /// Luma bicubic, chroma bilinear.
    ///
    /// Uses bicubic interpolation for luminance and bilinear for chrominance.
    /// Good compromise for video content.
    #[cfg(ffmpeg8)]
    pub const BICUBLIN: i32 = crate::SwsFlags_SWS_BICUBLIN as i32;
    #[cfg(not(ffmpeg8))]
    pub const BICUBLIN: i32 = crate::SWS_BICUBLIN as i32;

    /// Gaussian scaling.
    ///
    /// Uses a Gaussian filter for interpolation.
    #[cfg(ffmpeg8)]
    pub const GAUSS: i32 = crate::SwsFlags_SWS_GAUSS as i32;
    #[cfg(not(ffmpeg8))]
    pub const GAUSS: i32 = crate::SWS_GAUSS as i32;

    /// Sinc scaling.
    ///
    /// Uses a sinc filter. Produces high quality results but is slow.
    #[cfg(ffmpeg8)]
    pub const SINC: i32 = crate::SwsFlags_SWS_SINC as i32;
    #[cfg(not(ffmpeg8))]
    pub const SINC: i32 = crate::SWS_SINC as i32;

    /// Lanczos scaling (high quality).
    ///
    /// Uses a Lanczos windowed sinc filter. Produces very high quality
    /// results, especially for downscaling. Good for final export.
    #[cfg(ffmpeg8)]
    pub const LANCZOS: i32 = crate::SwsFlags_SWS_LANCZOS as i32;
    #[cfg(not(ffmpeg8))]
    pub const LANCZOS: i32 = crate::SWS_LANCZOS as i32;

    /// Spline scaling.
    ///
    /// Uses natural bicubic spline interpolation.
    #[cfg(ffmpeg8)]
    pub const SPLINE: i32 = crate::SwsFlags_SWS_SPLINE as i32;
    #[cfg(not(ffmpeg8))]
    pub const SPLINE: i32 = crate::SWS_SPLINE as i32;
}

// ============================================================================
// Context creation and management
// ============================================================================

/// Create a scaling context for converting and scaling video frames.
///
/// Allocates and returns a `SwsContext` configured for the specified source
/// and destination dimensions and pixel formats.
///
/// # Arguments
///
/// * `src_w` - Source width in pixels
/// * `src_h` - Source height in pixels
/// * `src_fmt` - Source pixel format
/// * `dst_w` - Destination width in pixels
/// * `dst_h` - Destination height in pixels
/// * `dst_fmt` - Destination pixel format
/// * `flags` - Scaling algorithm flags (see `scale_flags` module)
///
/// # Returns
///
/// Returns a pointer to the scaling context on success,
/// or an error code on failure (typically `ENOMEM` or `EINVAL`).
///
/// # Safety
///
/// - The returned context must be freed with `crate::sws_freeContext` (or wrapped in
///   [`ScaleContext`](crate::ScaleContext)) when no longer needed.
/// - The context is not thread-safe; use separate contexts for different threads.
///
/// # Errors
///
/// Returns a negative error code if:
/// - Width or height is zero or negative
/// - The pixel format combination is not supported
/// - Memory allocation fails
///
/// # Example
///
/// ```ignore
/// use ff_sys::swscale::{get_context, scale_flags};
/// use ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P;
/// use ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24;
///
/// unsafe {
///     let ctx = get_context(
///         1920, 1080, AVPixelFormat_AV_PIX_FMT_YUV420P,
///         1280, 720, AVPixelFormat_AV_PIX_FMT_RGB24,
///         scale_flags::LANCZOS,
///     )?;
///     // Use context...
///     crate::sws_freeContext(ctx);
/// }
/// ```
pub unsafe fn get_context(
    src_w: c_int,
    src_h: c_int,
    src_fmt: AVPixelFormat,
    dst_w: c_int,
    dst_h: c_int,
    dst_fmt: AVPixelFormat,
    flags: c_int,
) -> Result<*mut SwsContext, c_int> {
    ensure_initialized();

    // Validate dimensions
    if src_w <= 0 || src_h <= 0 || dst_w <= 0 || dst_h <= 0 {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ensure_initialized` has run and the dimensions are validated as
    // positive above. The srcFilter/dstFilter/param arguments are null, which
    // `sws_getContext` accepts. The returned pointer is checked for null before
    // it is handed back to the caller.
    let ctx = unsafe {
        ffi_sws_getContext(
            src_w,
            src_h,
            src_fmt,
            dst_w,
            dst_h,
            dst_fmt,
            flags,
            std::ptr::null_mut(), // srcFilter
            std::ptr::null_mut(), // dstFilter
            std::ptr::null(),     // param
        )
    };

    if ctx.is_null() {
        Err(crate::error_codes::ENOMEM)
    } else {
        Ok(ctx)
    }
}

// ============================================================================
// Scaling operations
// ============================================================================

/// Scale/convert an image using the given context.
///
/// Converts and scales source image data to the destination format and size.
///
/// # Arguments
///
/// * `ctx` - The scaling context created by `get_context()`
/// * `src` - Array of pointers to source image planes
/// * `src_stride` - Array of source image plane strides (line sizes in bytes)
/// * `src_slice_y` - Y position of the source slice (usually 0)
/// * `src_slice_h` - Height of the source slice (usually source height)
/// * `dst` - Array of pointers to destination image planes
/// * `dst_stride` - Array of destination image plane strides
///
/// # Returns
///
/// Returns the height of the output slice on success,
/// or a negative error code on failure.
///
/// # Safety
///
/// - All pointers must be valid and properly aligned.
/// - Source and destination buffers must be large enough for the configured
///   dimensions and pixel formats.
/// - The context must be valid and configured for the source/destination formats.
///
/// # Errors
///
/// Returns a negative error code if:
/// - Context is null (`error_codes::EINVAL`)
/// - Source or destination pointers are null (`error_codes::EINVAL`)
/// - Scaling fails due to internal error
///
/// # Example
///
/// ```ignore
/// unsafe {
///     let height = scale(
///         ctx,
///         src_data.as_ptr(),
///         src_linesize.as_ptr(),
///         0,          // Start at top
///         src_height, // Process entire image
///         dst_data.as_ptr(),
///         dst_linesize.as_ptr(),
///     )?;
/// }
/// ```
pub unsafe fn scale(
    ctx: *mut SwsContext,
    src: *const *const u8,
    src_stride: *const c_int,
    src_slice_y: c_int,
    src_slice_h: c_int,
    dst: *const *mut u8,
    dst_stride: *const c_int,
) -> Result<c_int, c_int> {
    if ctx.is_null() || src.is_null() || dst.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    if src_stride.is_null() || dst_stride.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx`, `src`, `dst`, and the two stride arrays are all non-null
    // (checked above). Per the safety contract the plane and stride arrays are
    // valid and sized for the formats and dimensions the context was created
    // for, so `sws_scale` reads and writes only within those buffers.
    let ret = unsafe {
        ffi_sws_scale(
            ctx,
            src,
            src_stride,
            src_slice_y,
            src_slice_h,
            dst,
            dst_stride,
        )
    };

    if ret < 0 { Err(ret) } else { Ok(ret) }
}

// ============================================================================
// Format support queries
// ============================================================================

/// Check if a pixel format is supported as input.
///
/// # Arguments
///
/// * `pix_fmt` - The pixel format to check
///
/// # Returns
///
/// Returns `true` if the format can be used as a source format for scaling.
///
/// # Safety
///
/// This function dereferences no caller-provided pointers; it is `unsafe` only
/// because it calls into the FFmpeg C API. `pix_fmt` must be a valid
/// `AVPixelFormat` value.
pub unsafe fn is_supported_input(pix_fmt: AVPixelFormat) -> bool {
    ensure_initialized();
    // SAFETY: the query takes a plain pixel-format enum by value and reads no
    // memory through pointers; the library is initialized above.
    unsafe { ffi_sws_isSupportedInput(pix_fmt) != 0 }
}

/// Check if a pixel format is supported as output.
///
/// # Arguments
///
/// * `pix_fmt` - The pixel format to check
///
/// # Returns
///
/// Returns `true` if the format can be used as a destination format for scaling.
///
/// # Safety
///
/// This function dereferences no caller-provided pointers; it is `unsafe` only
/// because it calls into the FFmpeg C API. `pix_fmt` must be a valid
/// `AVPixelFormat` value.
pub unsafe fn is_supported_output(pix_fmt: AVPixelFormat) -> bool {
    ensure_initialized();
    // SAFETY: the query takes a plain pixel-format enum by value and reads no
    // memory through pointers; the library is initialized above.
    unsafe { ffi_sws_isSupportedOutput(pix_fmt) != 0 }
}

/// Check if endianness conversion is supported for a pixel format.
///
/// # Arguments
///
/// * `pix_fmt` - The pixel format to check
///
/// # Returns
///
/// Returns `true` if endianness conversion is supported for the format.
///
/// # Safety
///
/// This function dereferences no caller-provided pointers; it is `unsafe` only
/// because it calls into the FFmpeg C API. `pix_fmt` must be a valid
/// `AVPixelFormat` value.
pub unsafe fn is_supported_endianness_conversion(pix_fmt: AVPixelFormat) -> bool {
    ensure_initialized();
    // SAFETY: the query takes a plain pixel-format enum by value and reads no
    // memory through pointers; the library is initialized above.
    unsafe { ffi_sws_isSupportedEndiannessConversion(pix_fmt) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AVPixelFormat_AV_PIX_FMT_RGB24, AVPixelFormat_AV_PIX_FMT_RGBA,
        AVPixelFormat_AV_PIX_FMT_YUV420P,
    };

    #[test]
    fn test_scale_flags_values() {
        // Verify scaling flags are non-zero and distinct
        assert!(scale_flags::FAST_BILINEAR > 0);
        assert!(scale_flags::BILINEAR > 0);
        assert!(scale_flags::BICUBIC > 0);
        assert!(scale_flags::LANCZOS > 0);

        // Common flags should be different values
        assert_ne!(scale_flags::BILINEAR, scale_flags::BICUBIC);
        assert_ne!(scale_flags::BILINEAR, scale_flags::LANCZOS);
        assert_ne!(scale_flags::BICUBIC, scale_flags::LANCZOS);
    }

    #[test]
    fn test_get_context_and_free() {
        unsafe {
            // Create a simple RGB to RGB scaling context
            let ctx_result = get_context(
                640,
                480,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                320,
                240,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::BILINEAR,
            );

            assert!(ctx_result.is_ok(), "Context creation should succeed");

            let ctx = ctx_result.unwrap();
            assert!(!ctx.is_null());

            // Free the context
            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_get_context_yuv_to_rgb() {
        unsafe {
            // Create YUV420P to RGB24 conversion context
            let ctx_result = get_context(
                1920,
                1080,
                AVPixelFormat_AV_PIX_FMT_YUV420P,
                1920,
                1080,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::BICUBIC,
            );

            assert!(ctx_result.is_ok(), "YUV to RGB context should succeed");

            let ctx = ctx_result.unwrap();
            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_get_context_with_lanczos() {
        unsafe {
            // Test with Lanczos scaling (high quality downscale)
            let ctx_result = get_context(
                3840,
                2160,
                AVPixelFormat_AV_PIX_FMT_YUV420P,
                1920,
                1080,
                AVPixelFormat_AV_PIX_FMT_YUV420P,
                scale_flags::LANCZOS,
            );

            assert!(ctx_result.is_ok(), "Lanczos context should succeed");

            let ctx = ctx_result.unwrap();
            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_get_context_invalid_dimensions() {
        unsafe {
            // Zero width should fail
            let result = get_context(
                0,
                480,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                320,
                240,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::BILINEAR,
            );
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);

            // Zero height should fail
            let result = get_context(
                640,
                0,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                320,
                240,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::BILINEAR,
            );
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);

            // Negative dimensions should fail
            let result = get_context(
                -640,
                480,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                320,
                240,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::BILINEAR,
            );
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_scale_null_context() {
        unsafe {
            let src: [*const u8; 4] = [std::ptr::null(); 4];
            let dst: [*mut u8; 4] = [std::ptr::null_mut(); 4];
            let src_stride: [c_int; 4] = [0; 4];
            let dst_stride: [c_int; 4] = [0; 4];

            let result = scale(
                std::ptr::null_mut(),
                src.as_ptr(),
                src_stride.as_ptr(),
                0,
                480,
                dst.as_ptr(),
                dst_stride.as_ptr(),
            );

            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_scale_null_src() {
        unsafe {
            // Create a valid context first
            let ctx = get_context(
                640,
                480,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                320,
                240,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::BILINEAR,
            )
            .unwrap();

            let dst: [*mut u8; 4] = [std::ptr::null_mut(); 4];
            let dst_stride: [c_int; 4] = [0; 4];

            // Null src pointer should fail
            let result = scale(
                ctx,
                std::ptr::null(),
                std::ptr::null(),
                0,
                480,
                dst.as_ptr(),
                dst_stride.as_ptr(),
            );

            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);

            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_scale_null_dst() {
        unsafe {
            let ctx = get_context(
                640,
                480,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                320,
                240,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::BILINEAR,
            )
            .unwrap();

            let src: [*const u8; 4] = [std::ptr::null(); 4];
            let src_stride: [c_int; 4] = [0; 4];

            // Null dst pointer should fail
            let result = scale(
                ctx,
                src.as_ptr(),
                src_stride.as_ptr(),
                0,
                480,
                std::ptr::null(),
                std::ptr::null(),
            );

            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);

            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_is_supported_input() {
        unsafe {
            // Common formats should be supported as input
            assert!(
                is_supported_input(AVPixelFormat_AV_PIX_FMT_YUV420P),
                "YUV420P should be supported as input"
            );
            assert!(
                is_supported_input(AVPixelFormat_AV_PIX_FMT_RGB24),
                "RGB24 should be supported as input"
            );
        }
    }

    #[test]
    fn test_is_supported_output() {
        unsafe {
            // Common formats should be supported as output
            assert!(
                is_supported_output(AVPixelFormat_AV_PIX_FMT_YUV420P),
                "YUV420P should be supported as output"
            );
            assert!(
                is_supported_output(AVPixelFormat_AV_PIX_FMT_RGB24),
                "RGB24 should be supported as output"
            );
        }
    }

    #[test]
    fn test_is_supported_endianness_conversion() {
        unsafe {
            // Just verify the function doesn't crash
            // Endianness conversion support varies by format
            let _rgb24 = is_supported_endianness_conversion(AVPixelFormat_AV_PIX_FMT_RGB24);
            let _yuv420p = is_supported_endianness_conversion(AVPixelFormat_AV_PIX_FMT_YUV420P);
        }
    }

    #[test]
    fn test_context_multiple_algorithms() {
        unsafe {
            let algorithms = [
                scale_flags::FAST_BILINEAR,
                scale_flags::BILINEAR,
                scale_flags::BICUBIC,
                scale_flags::POINT,
                scale_flags::AREA,
                scale_flags::LANCZOS,
            ];

            for &algo in &algorithms {
                let ctx_result = get_context(
                    640,
                    480,
                    AVPixelFormat_AV_PIX_FMT_YUV420P,
                    320,
                    240,
                    AVPixelFormat_AV_PIX_FMT_YUV420P,
                    algo,
                );

                assert!(
                    ctx_result.is_ok(),
                    "Algorithm {algo} should create valid context"
                );

                crate::sws_freeContext(ctx_result.unwrap());
            }
        }
    }

    // ========================================================================
    // Integration tests with actual image data
    // ========================================================================

    /// Load test image from assets directory.
    /// Returns (width, height, RGBA pixel data).
    fn load_test_image() -> (u32, u32, Vec<u8>) {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let image_path = format!("{}/../../assets/img/hello-triangle.png", manifest_dir);

        let img = image::open(&image_path)
            .unwrap_or_else(|e| panic!("Failed to load test image at {}: {}", image_path, e));
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        (width, height, rgba.into_raw())
    }

    #[test]
    fn test_scale_actual_image_downscale() {
        let (src_width, src_height, src_data) = load_test_image();

        // Downscale to half size
        let dst_width = src_width / 2;
        let dst_height = src_height / 2;

        unsafe {
            let ctx = get_context(
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                dst_width as c_int,
                dst_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                scale_flags::BILINEAR,
            )
            .unwrap_or_else(|e| panic!("Context creation failed with error code: {}", e));

            // Setup source buffer (RGBA has 1 plane)
            let src_stride: [c_int; 4] = [(src_width * 4) as c_int, 0, 0, 0];
            let src_ptrs: [*const u8; 4] = [
                src_data.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            ];

            // Allocate destination buffer
            let dst_size = (dst_width * dst_height * 4) as usize;
            let mut dst_data: Vec<u8> = vec![0u8; dst_size];
            let dst_stride: [c_int; 4] = [(dst_width * 4) as c_int, 0, 0, 0];
            let dst_ptrs: [*mut u8; 4] = [
                dst_data.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ];

            // Perform scaling
            let result = scale(
                ctx,
                src_ptrs.as_ptr(),
                src_stride.as_ptr(),
                0,
                src_height as c_int,
                dst_ptrs.as_ptr(),
                dst_stride.as_ptr(),
            );

            assert!(result.is_ok(), "Scaling should succeed");
            assert_eq!(
                result.unwrap(),
                dst_height as c_int,
                "Should process all output rows"
            );

            // Verify output has non-zero data (image was actually processed)
            let non_zero_count = dst_data.iter().filter(|&&b| b != 0).count();
            assert!(
                non_zero_count > dst_size / 2,
                "Output image should contain significant data"
            );

            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_scale_actual_image_upscale() {
        let (src_width, src_height, src_data) = load_test_image();

        // Upscale by 1.5x
        let dst_width = (src_width as f32 * 1.5) as u32;
        let dst_height = (src_height as f32 * 1.5) as u32;

        unsafe {
            let ctx = get_context(
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                dst_width as c_int,
                dst_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                scale_flags::LANCZOS,
            )
            .unwrap_or_else(|e| panic!("Context creation failed with error code: {}", e));

            let src_stride: [c_int; 4] = [(src_width * 4) as c_int, 0, 0, 0];
            let src_ptrs: [*const u8; 4] = [
                src_data.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            ];

            let dst_size = (dst_width * dst_height * 4) as usize;
            let mut dst_data: Vec<u8> = vec![0u8; dst_size];
            let dst_stride: [c_int; 4] = [(dst_width * 4) as c_int, 0, 0, 0];
            let dst_ptrs: [*mut u8; 4] = [
                dst_data.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ];

            let result = scale(
                ctx,
                src_ptrs.as_ptr(),
                src_stride.as_ptr(),
                0,
                src_height as c_int,
                dst_ptrs.as_ptr(),
                dst_stride.as_ptr(),
            );

            assert!(result.is_ok(), "Upscaling should succeed");
            assert_eq!(
                result.unwrap(),
                dst_height as c_int,
                "Should process all output rows"
            );

            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_scale_rgba_to_rgb24_conversion() {
        let (src_width, src_height, src_data) = load_test_image();

        // Convert RGBA to RGB24 (same dimensions)
        unsafe {
            let ctx = get_context(
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGBA,
                src_width as c_int,
                src_height as c_int,
                AVPixelFormat_AV_PIX_FMT_RGB24,
                scale_flags::POINT,
            )
            .unwrap_or_else(|e| panic!("Context creation failed with error code: {}", e));

            let src_stride: [c_int; 4] = [(src_width * 4) as c_int, 0, 0, 0];
            let src_ptrs: [*const u8; 4] = [
                src_data.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            ];

            // RGB24 uses 3 bytes per pixel
            let dst_size = (src_width * src_height * 3) as usize;
            let mut dst_data: Vec<u8> = vec![0u8; dst_size];
            let dst_stride: [c_int; 4] = [(src_width * 3) as c_int, 0, 0, 0];
            let dst_ptrs: [*mut u8; 4] = [
                dst_data.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ];

            let result = scale(
                ctx,
                src_ptrs.as_ptr(),
                src_stride.as_ptr(),
                0,
                src_height as c_int,
                dst_ptrs.as_ptr(),
                dst_stride.as_ptr(),
            );

            assert!(result.is_ok(), "Format conversion should succeed");
            assert_eq!(
                result.unwrap(),
                src_height as c_int,
                "Should process all output rows"
            );

            // Verify RGB values match first pixel (ignoring alpha)
            // Source RGBA pixel
            let src_r = src_data[0];
            let src_g = src_data[1];
            let src_b = src_data[2];

            // Destination RGB pixel
            let dst_r = dst_data[0];
            let dst_g = dst_data[1];
            let dst_b = dst_data[2];

            assert_eq!(src_r, dst_r, "Red channel should match");
            assert_eq!(src_g, dst_g, "Green channel should match");
            assert_eq!(src_b, dst_b, "Blue channel should match");

            crate::sws_freeContext(ctx);
        }
    }

    #[test]
    fn test_scale_multiple_algorithms_on_image() {
        let (src_width, src_height, src_data) = load_test_image();

        // Use smaller output for faster tests
        let dst_width = 256;
        let dst_height = 256;

        let algorithms = [
            ("FAST_BILINEAR", scale_flags::FAST_BILINEAR),
            ("BILINEAR", scale_flags::BILINEAR),
            ("BICUBIC", scale_flags::BICUBIC),
            ("POINT", scale_flags::POINT),
            ("AREA", scale_flags::AREA),
            ("LANCZOS", scale_flags::LANCZOS),
        ];

        for (name, algo) in algorithms {
            unsafe {
                let ctx = get_context(
                    src_width as c_int,
                    src_height as c_int,
                    AVPixelFormat_AV_PIX_FMT_RGBA,
                    dst_width,
                    dst_height,
                    AVPixelFormat_AV_PIX_FMT_RGBA,
                    algo,
                )
                .unwrap_or_else(|e| panic!("{} context creation failed: {}", name, e));

                let src_stride: [c_int; 4] = [(src_width * 4) as c_int, 0, 0, 0];
                let src_ptrs: [*const u8; 4] = [
                    src_data.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                ];

                let dst_size = (dst_width * dst_height * 4) as usize;
                let mut dst_data: Vec<u8> = vec![0u8; dst_size];
                let dst_stride: [c_int; 4] = [(dst_width * 4) as c_int, 0, 0, 0];
                let dst_ptrs: [*mut u8; 4] = [
                    dst_data.as_mut_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ];

                let result = scale(
                    ctx,
                    src_ptrs.as_ptr(),
                    src_stride.as_ptr(),
                    0,
                    src_height as c_int,
                    dst_ptrs.as_ptr(),
                    dst_stride.as_ptr(),
                );

                assert!(result.is_ok(), "{} scaling should succeed", name);
                assert_eq!(
                    result.unwrap(),
                    dst_height,
                    "{} should process all output rows",
                    name
                );

                crate::sws_freeContext(ctx);
            }
        }
    }
}
