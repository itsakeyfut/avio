//! SwrContext allocation, configuration, and cleanup.

use std::os::raw::c_int;

use crate::{
    AVChannelLayout, AVSampleFormat, SwrContext, ensure_initialized, swr_alloc as ffi_swr_alloc,
    swr_alloc_set_opts2 as ffi_swr_alloc_set_opts2, swr_free as ffi_swr_free,
    swr_init as ffi_swr_init, swr_is_initialized as ffi_swr_is_initialized,
};

/// Allocate an empty SwrContext.
///
/// This allocates an uninitialized context that must be configured
/// using `alloc_set_opts2()` or `swr_alloc_set_opts2()` before use.
///
/// # Returns
///
/// Returns a pointer to the allocated context on success,
/// or an error code on failure (typically `ENOMEM`).
///
/// # Safety
///
/// The returned context must be freed using `free()` when no longer needed.
pub unsafe fn alloc() -> Result<*mut SwrContext, c_int> {
    ensure_initialized();

    // SAFETY: swr_alloc takes no arguments and allocates a fresh context;
    // FFmpeg has been initialized above.
    let ctx = unsafe { ffi_swr_alloc() };
    if ctx.is_null() {
        Err(crate::error_codes::ENOMEM)
    } else {
        Ok(ctx)
    }
}

/// Allocate and configure a SwrContext for audio conversion.
///
/// This is the recommended way to create a resampling context in FFmpeg 7.x.
/// It uses the new `AVChannelLayout` API instead of the deprecated channel masks.
///
/// # Arguments
///
/// * `out_ch_layout` - Output channel layout
/// * `out_sample_fmt` - Output sample format
/// * `out_sample_rate` - Output sample rate in Hz
/// * `in_ch_layout` - Input channel layout
/// * `in_sample_fmt` - Input sample format
/// * `in_sample_rate` - Input sample rate in Hz
///
/// # Returns
///
/// Returns a pointer to the configured context on success,
/// or an error code on failure.
///
/// # Safety
///
/// - The returned context must be initialized with `init()` before use.
/// - The returned context must be freed using `free()` when no longer needed.
/// - Channel layout pointers must be valid for the duration of this call.
pub unsafe fn alloc_set_opts2(
    out_ch_layout: *const AVChannelLayout,
    out_sample_fmt: AVSampleFormat,
    out_sample_rate: c_int,
    in_ch_layout: *const AVChannelLayout,
    in_sample_fmt: AVSampleFormat,
    in_sample_rate: c_int,
) -> Result<*mut SwrContext, c_int> {
    ensure_initialized();

    // Validate inputs
    if out_ch_layout.is_null() || in_ch_layout.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    if out_sample_rate <= 0 || in_sample_rate <= 0 {
        return Err(crate::error_codes::EINVAL);
    }

    let mut ctx: *mut SwrContext = std::ptr::null_mut();

    // SAFETY: `&mut ctx` is a valid out-pointer; `out_ch_layout` and
    // `in_ch_layout` are non-null (checked above) and remain valid for the
    // duration of this call; the sample rates are positive (checked above).
    let ret = unsafe {
        ffi_swr_alloc_set_opts2(
            &mut ctx,
            out_ch_layout,
            out_sample_fmt,
            out_sample_rate,
            in_ch_layout,
            in_sample_fmt,
            in_sample_rate,
            0,                    // log_offset
            std::ptr::null_mut(), // log_ctx
        )
    };

    if ret < 0 {
        Err(ret)
    } else if ctx.is_null() {
        Err(crate::error_codes::ENOMEM)
    } else {
        Ok(ctx)
    }
}

/// Initialize a resampling context after all options have been set.
///
/// This must be called after `alloc_set_opts2()` or after manually setting
/// options on an allocated context.
///
/// # Arguments
///
/// * `ctx` - The context to initialize
///
/// # Returns
///
/// Returns `Ok(())` on success, or an FFmpeg error code on failure.
///
/// # Safety
///
/// - The context must be allocated and configured.
/// - The context must not already be initialized.
///
/// # Errors
///
/// Returns a negative error code if:
/// - Context is null (`error_codes::EINVAL`)
/// - Initialization fails (e.g., invalid configuration)
pub unsafe fn init(ctx: *mut SwrContext) -> Result<(), c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` is non-null (checked above) and points to a context
    // allocated and configured by the caller.
    let ret = unsafe { ffi_swr_init(ctx) };

    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Check if a context is initialized.
///
/// # Arguments
///
/// * `ctx` - The context to check
///
/// # Returns
///
/// Returns `true` if the context is initialized and ready for conversion.
///
/// # Safety
///
/// The context pointer must be valid or null.
pub unsafe fn is_initialized(ctx: *const SwrContext) -> bool {
    if ctx.is_null() {
        return false;
    }

    // SAFETY: `ctx` is non-null (checked above) and points to a valid context.
    unsafe { ffi_swr_is_initialized(ctx.cast_mut()) != 0 }
}

/// Free a resampling context.
///
/// # Arguments
///
/// * `ctx` - Pointer to a pointer to the context to free
///
/// # Safety
///
/// - The context must have been allocated by `alloc()` or `alloc_set_opts2()`.
/// - After this call, `*ctx` will be set to null.
/// - The context pointer must not be used after this call.
///
/// # Null Safety
///
/// This function safely handles:
/// - `ctx` being null
/// - `*ctx` being null
pub unsafe fn free(ctx: *mut *mut SwrContext) {
    // SAFETY: `ctx` is checked non-null before `*ctx` is dereferenced (via
    // short-circuit `&&`); `*ctx` is either null or a context allocated by
    // `alloc`/`alloc_set_opts2`, which swr_free frees and sets to null.
    unsafe {
        if !ctx.is_null() && !(*ctx).is_null() {
            ffi_swr_free(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swresample::{channel_layout, sample_format};

    #[test]
    fn test_alloc_and_free() {
        unsafe {
            let ctx_result = alloc();
            assert!(ctx_result.is_ok(), "Context allocation should succeed");

            let mut ctx = ctx_result.unwrap();
            assert!(!ctx.is_null());

            free(&mut ctx);
            assert!(ctx.is_null(), "Context should be null after free");
        }
    }

    #[test]
    fn test_free_null_safety() {
        unsafe {
            // Passing a null pointer should not crash
            free(std::ptr::null_mut());

            // Passing a pointer to null should not crash
            let mut null_ctx: *mut SwrContext = std::ptr::null_mut();
            free(&mut null_ctx);
        }
    }

    #[test]
    fn test_alloc_set_opts2_and_init() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let ctx_result = alloc_set_opts2(
                &out_layout,
                sample_format::FLTP,
                48000,
                &in_layout,
                sample_format::S16,
                44100,
            );

            assert!(ctx_result.is_ok(), "Context creation should succeed");

            let mut ctx = ctx_result.unwrap();
            assert!(!ctx.is_null());

            // Context should not be initialized yet
            assert!(!is_initialized(ctx));

            // Initialize the context
            let init_result = init(ctx);
            assert!(init_result.is_ok(), "Context initialization should succeed");

            // Now it should be initialized
            assert!(is_initialized(ctx));

            free(&mut ctx);
        }
    }

    #[test]
    fn test_alloc_set_opts2_mono_to_stereo() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::mono();

            let ctx_result = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                48000,
                &in_layout,
                sample_format::FLT,
                48000,
            );

            assert!(
                ctx_result.is_ok(),
                "Mono to stereo conversion should be supported"
            );

            let mut ctx = ctx_result.unwrap();
            let init_result = init(ctx);
            assert!(init_result.is_ok());

            free(&mut ctx);
        }
    }

    #[test]
    fn test_alloc_set_opts2_invalid_null_layout() {
        unsafe {
            let out_layout = channel_layout::stereo();

            let result = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                48000,
                std::ptr::null(),
                sample_format::FLT,
                44100,
            );

            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_alloc_set_opts2_invalid_sample_rate() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            // Zero sample rate
            let result = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                0,
                &in_layout,
                sample_format::FLT,
                44100,
            );

            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);

            // Negative sample rate
            let result = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                -1,
                &in_layout,
                sample_format::FLT,
                44100,
            );

            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_init_null() {
        unsafe {
            let result = init(std::ptr::null_mut());
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_is_initialized_null() {
        unsafe {
            assert!(!is_initialized(std::ptr::null()));
        }
    }
}
