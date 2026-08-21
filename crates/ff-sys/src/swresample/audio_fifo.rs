//! Safe thin wrappers around `AVAudioFifo` (from `libavutil/audio_fifo.h`).
//!
//! `AVAudioFifo` is FFmpeg's format-aware circular sample buffer. It handles
//! both planar and packed sample layouts internally and is used to adapt
//! variable-size decoded frames to the fixed frame size required by some
//! encoders (e.g. AAC requires exactly 1024 samples per frame).

use std::ffi::c_void;
use std::os::raw::c_int;

use crate::{AVAudioFifo, AVSampleFormat};

/// Allocate an `AVAudioFifo` for the given sample format, channel count,
/// and initial capacity (in samples).
///
/// Returns `Ok(fifo)` on success, `Err(-1)` on allocation failure.
///
/// # Safety
///
/// `sample_fmt` must be a valid format; `channels` and `nb_samples` must
/// be positive.
pub unsafe fn alloc(
    sample_fmt: AVSampleFormat,
    channels: c_int,
    nb_samples: c_int,
) -> Result<*mut AVAudioFifo, c_int> {
    // SAFETY: caller guarantees parameters are valid
    let fifo = unsafe { crate::av_audio_fifo_alloc(sample_fmt, channels, nb_samples) };
    if fifo.is_null() { Err(-1) } else { Ok(fifo) }
}

/// Free an `AVAudioFifo` created by [`alloc`].
///
/// # Safety
///
/// `fifo` must be a valid non-null pointer returned by [`alloc`].
pub unsafe fn free(fifo: *mut AVAudioFifo) {
    // SAFETY: caller guarantees fifo is valid
    unsafe { crate::av_audio_fifo_free(fifo) };
}

/// Write `nb_samples` samples from `data` into the FIFO.
///
/// `data` is a const pointer to an array of channel buffer pointers (one
/// per channel for planar formats, one for packed formats). The pointer
/// array itself is not modified; the data the pointers reference is read.
/// Returns the number of samples written.
///
/// # Safety
///
/// `fifo` must be valid; each channel buffer in `data` must contain at
/// least `nb_samples` samples worth of bytes.
pub unsafe fn write(
    fifo: *mut AVAudioFifo,
    data: *const *mut c_void,
    nb_samples: c_int,
) -> Result<c_int, c_int> {
    // SAFETY: caller guarantees all pointers are valid
    let ret = unsafe { crate::av_audio_fifo_write(fifo, data, nb_samples) };
    if ret < 0 { Err(ret) } else { Ok(ret) }
}

/// Read up to `nb_samples` samples from the FIFO into pre-allocated
/// channel buffers.
///
/// `data` is a const pointer to an array of writable channel buffer
/// pointers. The pointer array itself is not modified; the data the
/// pointers reference is written.
/// Returns the number of samples actually read (may be less than
/// `nb_samples` if the FIFO contains fewer samples).
///
/// # Safety
///
/// `fifo` must be valid; each channel buffer in `data` must have room for
/// at least `nb_samples` samples.
pub unsafe fn read(
    fifo: *mut AVAudioFifo,
    data: *const *mut c_void,
    nb_samples: c_int,
) -> Result<c_int, c_int> {
    // SAFETY: caller guarantees all pointers are valid
    let ret = unsafe { crate::av_audio_fifo_read(fifo, data, nb_samples) };
    if ret < 0 { Err(ret) } else { Ok(ret) }
}

/// Return the number of samples currently stored in the FIFO.
///
/// # Safety
///
/// `fifo` must be a valid non-null pointer.
pub unsafe fn size(fifo: *mut AVAudioFifo) -> c_int {
    // SAFETY: caller guarantees fifo is valid
    unsafe { crate::av_audio_fifo_size(fifo) }
}
