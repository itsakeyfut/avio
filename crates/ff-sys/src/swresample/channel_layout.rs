//! Channel layout helpers for audio configuration.

use crate::{
    AVChannelLayout, AVChannelOrder_AV_CHANNEL_ORDER_NATIVE, av_channel_layout_compare,
    av_channel_layout_copy, av_channel_layout_default, av_channel_layout_uninit,
};

/// Initialize a channel layout with a default layout for the given number of channels.
///
/// # Arguments
///
/// * `ch_layout` - Pointer to the channel layout to initialize
/// * `nb_channels` - Number of channels
///
/// # Safety
///
/// - The channel layout pointer must be valid and uninitialized (or zeroed).
/// - If `ch_layout` is null, this function is a no-op (silently ignored).
pub unsafe fn set_default(ch_layout: *mut AVChannelLayout, nb_channels: i32) {
    if !ch_layout.is_null() {
        // SAFETY: `ch_layout` is non-null (checked above) and points to storage
        // the caller guarantees is valid to initialize.
        unsafe { av_channel_layout_default(ch_layout, nb_channels) };
    }
}

/// Uninitialize a channel layout and reset it to a zeroed state.
///
/// # Arguments
///
/// * `ch_layout` - Pointer to the channel layout to uninitialize
///
/// # Safety
///
/// - The channel layout pointer must be valid.
/// - If `ch_layout` is null, this function is a no-op (silently ignored).
pub unsafe fn uninit(ch_layout: *mut AVChannelLayout) {
    if !ch_layout.is_null() {
        // SAFETY: `ch_layout` is non-null (checked above) and points to a valid
        // channel layout to uninitialize.
        unsafe { av_channel_layout_uninit(ch_layout) };
    }
}

/// Copy a channel layout.
///
/// # Arguments
///
/// * `dst` - Destination channel layout
/// * `src` - Source channel layout
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error code on failure.
///
/// # Safety
///
/// Both pointers must be valid.
pub unsafe fn copy(dst: *mut AVChannelLayout, src: *const AVChannelLayout) -> Result<(), i32> {
    if dst.is_null() || src.is_null() {
        return Err(crate::error_codes::EINVAL);
    }

    // SAFETY: `dst` and `src` are both non-null (checked above) and point to
    // valid channel layouts.
    let ret = unsafe { av_channel_layout_copy(dst, src) };
    if ret < 0 { Err(ret) } else { Ok(()) }
}

/// Compare two channel layouts.
///
/// # Arguments
///
/// * `chl` - First channel layout
/// * `chl1` - Second channel layout
///
/// # Returns
///
/// Returns `true` if the layouts are identical, `false` otherwise.
///
/// # Safety
///
/// Both pointers must be valid.
pub unsafe fn is_equal(chl: *const AVChannelLayout, chl1: *const AVChannelLayout) -> bool {
    if chl.is_null() || chl1.is_null() {
        return false;
    }

    // SAFETY: `chl` and `chl1` are both non-null (checked above) and point to
    // valid channel layouts.
    unsafe { av_channel_layout_compare(chl, chl1) == 0 }
}

/// Create a mono channel layout.
///
/// # Returns
///
/// Returns a mono (1 channel) layout.
pub fn mono() -> AVChannelLayout {
    let mut layout = AVChannelLayout::default();
    // SAFETY: `&mut layout` points to a valid, stack-allocated channel layout.
    unsafe {
        av_channel_layout_default(&mut layout, 1);
    }
    layout
}

/// Create a stereo channel layout.
///
/// # Returns
///
/// Returns a stereo (2 channel) layout.
pub fn stereo() -> AVChannelLayout {
    let mut layout = AVChannelLayout::default();
    // SAFETY: `&mut layout` points to a valid, stack-allocated channel layout.
    unsafe {
        av_channel_layout_default(&mut layout, 2);
    }
    layout
}

/// Create a channel layout with the specified number of channels.
///
/// Uses the default layout for that channel count (e.g., 2 = stereo, 6 = 5.1).
///
/// # Arguments
///
/// * `nb_channels` - Number of channels
///
/// # Returns
///
/// Returns a channel layout with the default configuration for `nb_channels`.
pub fn with_channels(nb_channels: i32) -> AVChannelLayout {
    let mut layout = AVChannelLayout::default();
    // SAFETY: `&mut layout` points to a valid, stack-allocated channel layout.
    unsafe {
        av_channel_layout_default(&mut layout, nb_channels);
    }
    layout
}

/// Check if a channel layout is valid.
///
/// # Arguments
///
/// * `ch_layout` - The channel layout to check
///
/// # Returns
///
/// Returns `true` if the layout has at least one channel, `false` otherwise.
pub fn is_valid(ch_layout: &AVChannelLayout) -> bool {
    ch_layout.nb_channels > 0
}

/// Get the number of channels in a layout.
///
/// # Arguments
///
/// * `ch_layout` - The channel layout
///
/// # Returns
///
/// Returns the number of channels.
pub fn nb_channels(ch_layout: &AVChannelLayout) -> i32 {
    ch_layout.nb_channels
}

/// Check if a channel layout uses native order.
///
/// # Arguments
///
/// * `ch_layout` - The channel layout to check
///
/// # Returns
///
/// Returns `true` if the layout uses native channel order.
pub fn is_native_order(ch_layout: &AVChannelLayout) -> bool {
    ch_layout.order == AVChannelOrder_AV_CHANNEL_ORDER_NATIVE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_layout_mono() {
        let layout = mono();
        assert_eq!(nb_channels(&layout), 1);
        assert!(is_valid(&layout));
    }

    #[test]
    fn test_channel_layout_stereo() {
        let layout = stereo();
        assert_eq!(nb_channels(&layout), 2);
        assert!(is_valid(&layout));
    }

    #[test]
    fn test_channel_layout_with_channels() {
        for n in 1..=8 {
            let layout = with_channels(n);
            assert_eq!(nb_channels(&layout), n);
            assert!(is_valid(&layout));
        }
    }

    #[test]
    fn test_channel_layout_copy() {
        let src = stereo();
        let mut dst = AVChannelLayout::default();

        unsafe {
            let result = copy(&mut dst, &src);
            assert!(result.is_ok());
            assert_eq!(nb_channels(&dst), 2);
            assert!(is_equal(&src, &dst));

            uninit(&mut dst);
        }
    }

    #[test]
    fn test_channel_layout_copy_null() {
        unsafe {
            let result = copy(std::ptr::null_mut(), std::ptr::null());
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_channel_layout_is_equal() {
        let layout1 = stereo();
        let layout2 = stereo();
        let layout3 = mono();

        unsafe {
            assert!(is_equal(&layout1, &layout2));
            assert!(!is_equal(&layout1, &layout3));
        }
    }

    #[test]
    fn test_channel_layout_is_equal_null() {
        let layout = stereo();
        unsafe {
            assert!(!is_equal(std::ptr::null(), &layout));
            assert!(!is_equal(&layout, std::ptr::null()));
        }
    }
}
