//! Audio conversion and resampling operations.

use std::os::raw::c_int;

use crate::{SwrContext, swr_convert as ffi_swr_convert};
// Only the test-only `get_delay` wrapper uses this binding.
#[cfg(test)]
use crate::swr_get_delay as ffi_swr_get_delay;

/// Headroom samples added to output buffer estimates.
///
/// This accounts for resampling filter delay and rounding errors.
/// The value of 256 samples provides sufficient headroom for most
/// resampling scenarios without significant memory overhead.
pub(super) const RESAMPLE_HEADROOM_SAMPLES: i32 = 256;

/// Convert audio samples.
///
/// Converts input audio samples to the output format configured in the context.
///
/// # Arguments
///
/// * `ctx` - The initialized resampling context
/// * `out` - Array of pointers to output sample planes (planar) or single buffer (packed)
/// * `out_count` - Maximum number of output samples per channel
/// * `in_` - Array of pointers to input sample planes (planar) or single buffer (packed)
/// * `in_count` - Number of input samples per channel
///
/// # Returns
///
/// Returns the number of samples output per channel on success,
/// or a negative error code on failure.
///
/// # Safety
///
/// - The context must be initialized.
/// - Output buffers must be large enough to hold `out_count` samples.
/// - Input buffers must contain at least `in_count` samples.
/// - For planar formats, each plane pointer must be valid.
/// - For packed formats, only the first pointer is used.
///
/// # Flushing
///
/// To flush remaining samples after all input has been processed,
/// call with `in_` set to null and `in_count` set to 0.
pub(crate) unsafe fn convert(
    ctx: *mut SwrContext,
    out: *mut *mut u8,
    out_count: c_int,
    in_: *const *const u8,
    in_count: c_int,
) -> Result<c_int, c_int> {
    if ctx.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // Note: out and in_ can be null for certain operations (e.g., flushing)
    // but out_count must be valid

    if out_count < 0 || in_count < 0 {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `ctx` is non-null (checked above) and initialized per the caller
    // contract; `out`/`in_` and their counts describe buffers valid for the
    // context's configured formats (or null with a zero count when flushing).
    let ret = unsafe { ffi_swr_convert(ctx, out, out_count, in_, in_count) };

    if ret < 0 { Err(ret) } else { Ok(ret) }
}

/// Get the delay (in samples) caused by the resampling filter.
///
/// This is useful for determining how many samples are buffered internally
/// and need to be flushed at the end of a stream.
///
/// # Arguments
///
/// * `ctx` - The resampling context
/// * `base` - Time base for the returned delay (e.g., output sample rate)
///
/// # Returns
///
/// Returns the delay in units of `1/base` seconds.
///
/// # Safety
///
/// The context pointer must be valid.
#[cfg(test)]
pub(crate) unsafe fn get_delay(ctx: *mut SwrContext, base: i64) -> i64 {
    if ctx.is_null() {
        return 0;
    }

    // SAFETY: `ctx` is non-null (checked above) and points to a valid context.
    unsafe { ffi_swr_get_delay(ctx, base) }
}

/// Calculate the required output buffer size for a given input size.
///
/// This accounts for the sample rate conversion ratio and any delay
/// in the resampling filter.
///
/// # Arguments
///
/// * `out_sample_rate` - Output sample rate
/// * `in_sample_rate` - Input sample rate
/// * `in_samples` - Number of input samples per channel
///
/// # Returns
///
/// Returns the recommended number of output samples to allocate,
/// or 0 if inputs are invalid.
pub fn estimate_output_samples(out_sample_rate: i32, in_sample_rate: i32, in_samples: i32) -> i32 {
    if out_sample_rate <= 0 || in_sample_rate <= 0 || in_samples < 0 {
        return 0;
    }

    // Calculate output samples based on sample rate ratio
    // Add extra samples to account for rounding and filter delay
    let ratio = out_sample_rate as f64 / in_sample_rate as f64;
    let estimated = (in_samples as f64 * ratio).ceil() as i32;

    // Add headroom for filter delay
    estimated + RESAMPLE_HEADROOM_SAMPLES
}

#[cfg(test)]
mod tests {
    // Test-only scaffolding calls the raw wrapper fns directly; keep the terse
    // pre-2024 style here. The library's safe layer is held to
    // `unsafe_op_in_unsafe_fn` at the crate root.
    #![allow(unsafe_op_in_unsafe_fn)]

    use super::*;
    use crate::swresample::{alloc_set_opts2, channel_layout, init, sample_format};

    // ========================================================================
    // Conversion tests
    // ========================================================================

    #[test]
    fn test_convert_null_context() {
        unsafe {
            let result = convert(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                0,
            );

            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);
        }
    }

    #[test]
    fn test_convert_invalid_counts() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                48000,
                &in_layout,
                sample_format::FLT,
                48000,
            )
            .unwrap();

            init(ctx).unwrap();

            // Negative out_count
            let result = convert(ctx, std::ptr::null_mut(), -1, std::ptr::null(), 0);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);

            // Negative in_count
            let result = convert(ctx, std::ptr::null_mut(), 0, std::ptr::null(), -1);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), crate::error_codes::EINVAL);

            crate::swr_free(&mut ctx);
        }
    }

    #[test]
    fn test_convert_s16_to_float() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLTP,
                48000,
                &in_layout,
                sample_format::S16,
                48000,
            )
            .unwrap();

            init(ctx).unwrap();

            // Create input data (S16 stereo, interleaved)
            let num_samples = 1024;
            let mut in_data: Vec<i16> = vec![0; num_samples * 2]; // 2 channels

            // Generate a simple sine wave
            for i in 0..num_samples {
                let sample = ((i as f32 / 48.0).sin() * 16000.0) as i16;
                in_data[i * 2] = sample; // Left
                in_data[i * 2 + 1] = sample; // Right
            }

            // Create output buffers (FLTP = planar float, separate planes)
            let out_samples = estimate_output_samples(48000, 48000, num_samples as i32);
            let mut out_left: Vec<f32> = vec![0.0; out_samples as usize];
            let mut out_right: Vec<f32> = vec![0.0; out_samples as usize];

            let in_ptr = in_data.as_ptr() as *const u8;
            let in_ptrs: [*const u8; 1] = [in_ptr];

            let out_ptr_left = out_left.as_mut_ptr() as *mut u8;
            let out_ptr_right = out_right.as_mut_ptr() as *mut u8;
            let mut out_ptrs: [*mut u8; 2] = [out_ptr_left, out_ptr_right];

            let result = convert(
                ctx,
                out_ptrs.as_mut_ptr(),
                out_samples,
                in_ptrs.as_ptr(),
                num_samples as c_int,
            );

            assert!(result.is_ok(), "Conversion should succeed");
            let samples_out = result.unwrap();
            assert!(samples_out > 0, "Should produce output samples");
            assert!(samples_out <= out_samples, "Should not exceed buffer size");

            // Verify some output data is non-zero
            let non_zero_left = out_left
                .iter()
                .take(samples_out as usize)
                .any(|&x| x != 0.0);
            let non_zero_right = out_right
                .iter()
                .take(samples_out as usize)
                .any(|&x| x != 0.0);
            assert!(
                non_zero_left || non_zero_right,
                "Output should contain data"
            );

            crate::swr_free(&mut ctx);
        }
    }

    #[test]
    fn test_convert_resample_44100_to_48000() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                48000,
                &in_layout,
                sample_format::FLT,
                44100,
            )
            .unwrap();

            init(ctx).unwrap();

            // Create input data (FLT stereo, interleaved)
            let num_samples = 1024;
            let mut in_data: Vec<f32> = vec![0.0; num_samples * 2];

            // Generate test data
            for i in 0..num_samples {
                let sample = (i as f32 / 100.0).sin();
                in_data[i * 2] = sample;
                in_data[i * 2 + 1] = sample;
            }

            // Calculate expected output size
            let out_samples = estimate_output_samples(48000, 44100, num_samples as i32);
            let mut out_data: Vec<f32> = vec![0.0; out_samples as usize * 2];

            let in_ptr = in_data.as_ptr() as *const u8;
            let in_ptrs: [*const u8; 1] = [in_ptr];

            let out_ptr = out_data.as_mut_ptr() as *mut u8;
            let mut out_ptrs: [*mut u8; 1] = [out_ptr];

            let result = convert(
                ctx,
                out_ptrs.as_mut_ptr(),
                out_samples,
                in_ptrs.as_ptr(),
                num_samples as c_int,
            );

            assert!(result.is_ok(), "Resampling should succeed");
            let samples_out = result.unwrap();

            // 44100 -> 48000 should produce more samples
            // Expected ratio: 48000/44100 ≈ 1.088
            let expected_min = (num_samples as f32 * 1.05) as i32;
            let expected_max = (num_samples as f32 * 1.15) as i32;

            assert!(
                samples_out >= expected_min && samples_out <= expected_max,
                "Output sample count {} should be between {} and {}",
                samples_out,
                expected_min,
                expected_max
            );

            crate::swr_free(&mut ctx);
        }
    }

    // ========================================================================
    // Delay tests
    // ========================================================================

    #[test]
    fn test_get_delay_null() {
        unsafe {
            let delay = get_delay(std::ptr::null_mut(), 48000);
            assert_eq!(delay, 0);
        }
    }

    #[test]
    fn test_get_delay_after_init() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                48000,
                &in_layout,
                sample_format::FLT,
                44100,
            )
            .unwrap();

            init(ctx).unwrap();

            // Initial delay should be 0 (no samples buffered yet)
            let delay = get_delay(ctx, 48000);
            assert_eq!(delay, 0, "Initial delay should be 0");

            crate::swr_free(&mut ctx);
        }
    }

    // ========================================================================
    // Helper function tests
    // ========================================================================

    #[test]
    fn test_estimate_output_samples_same_rate() {
        let out = estimate_output_samples(48000, 48000, 1024);
        // Should be slightly more than input due to headroom
        assert!(out > 1024);
        assert!(out < 2048);
    }

    #[test]
    fn test_estimate_output_samples_upsampling() {
        // 44100 -> 48000
        let out = estimate_output_samples(48000, 44100, 1024);
        // Expected: 1024 * (48000/44100) ≈ 1115 + headroom
        assert!(out > 1100);
        assert!(out < 1500);
    }

    #[test]
    fn test_estimate_output_samples_downsampling() {
        // 48000 -> 44100
        let out = estimate_output_samples(44100, 48000, 1024);
        // Expected: 1024 * (44100/48000) ≈ 940 + headroom
        assert!(out > 900);
        assert!(out < 1300);
    }

    #[test]
    fn test_estimate_output_samples_invalid() {
        assert_eq!(estimate_output_samples(0, 48000, 1024), 0);
        assert_eq!(estimate_output_samples(48000, 0, 1024), 0);
        assert_eq!(estimate_output_samples(48000, 48000, -1), 0);
        assert_eq!(estimate_output_samples(-1, 48000, 1024), 0);
    }

    // ========================================================================
    // Multiple conversion passes tests
    // ========================================================================

    #[test]
    fn test_multiple_conversions() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                48000,
                &in_layout,
                sample_format::FLT,
                48000,
            )
            .unwrap();

            init(ctx).unwrap();

            let chunk_size = 256;
            let num_chunks = 4;
            let in_data: Vec<f32> = vec![0.5; chunk_size * 2];

            let out_samples = estimate_output_samples(48000, 48000, chunk_size as i32);
            let mut out_data: Vec<f32> = vec![0.0; out_samples as usize * 2];

            for _ in 0..num_chunks {
                let in_ptr = in_data.as_ptr() as *const u8;
                let in_ptrs: [*const u8; 1] = [in_ptr];

                let out_ptr = out_data.as_mut_ptr() as *mut u8;
                let mut out_ptrs: [*mut u8; 1] = [out_ptr];

                let result = convert(
                    ctx,
                    out_ptrs.as_mut_ptr(),
                    out_samples,
                    in_ptrs.as_ptr(),
                    chunk_size as c_int,
                );

                assert!(result.is_ok(), "Conversion should succeed on each pass");
            }

            crate::swr_free(&mut ctx);
        }
    }

    // ========================================================================
    // Flush test
    // ========================================================================

    #[test]
    fn test_flush_remaining_samples() {
        unsafe {
            let out_layout = channel_layout::stereo();
            let in_layout = channel_layout::stereo();

            let mut ctx = alloc_set_opts2(
                &out_layout,
                sample_format::FLT,
                48000,
                &in_layout,
                sample_format::FLT,
                44100, // Different rate to ensure buffering
            )
            .unwrap();

            init(ctx).unwrap();

            // Convert some data first
            let num_samples = 1024;
            let in_data: Vec<f32> = vec![0.5; num_samples * 2];
            let out_samples = estimate_output_samples(48000, 44100, num_samples as i32);
            let mut out_data: Vec<f32> = vec![0.0; out_samples as usize * 2];

            let in_ptr = in_data.as_ptr() as *const u8;
            let in_ptrs: [*const u8; 1] = [in_ptr];

            let out_ptr = out_data.as_mut_ptr() as *mut u8;
            let mut out_ptrs: [*mut u8; 1] = [out_ptr];

            convert(
                ctx,
                out_ptrs.as_mut_ptr(),
                out_samples,
                in_ptrs.as_ptr(),
                num_samples as c_int,
            )
            .unwrap();

            // Flush: call with null input
            let result = convert(ctx, out_ptrs.as_mut_ptr(), out_samples, std::ptr::null(), 0);

            // Flush should succeed (return >= 0)
            assert!(result.is_ok(), "Flush should succeed");

            crate::swr_free(&mut ctx);
        }
    }
}
