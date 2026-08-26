//! RAII owner for an `AVPacket`.
//!
//! [`Packet`] allocates a packet and frees it exactly once on drop, replacing
//! the manual `av_packet_alloc` + `av_packet_free` pair. Ownership is unique;
//! [`try_clone`](Packet::try_clone) makes a ref-counted copy (`av_packet_ref`).
//! Scalar fields ([`stream_index`](Packet::stream_index) / [`pts`](Packet::pts))
//! are read through the typed accessors below.

use std::ptr::NonNull;

use crate::{
    AVPacket, AVRational, AvError, av_packet_alloc as ffi_av_packet_alloc,
    av_packet_free as ffi_av_packet_free, av_packet_ref as ffi_av_packet_ref,
    av_packet_rescale_ts as ffi_av_packet_rescale_ts, av_packet_unref as ffi_av_packet_unref,
};

/// An owned `AVPacket`.
///
/// The packet is freed exactly once on drop. This is guaranteed by construction:
/// the value owns a [`NonNull`] and is neither `Copy` nor `Clone`, so it drops
/// exactly once and cannot be duplicated (a ref-counted copy is made explicitly
/// via [`try_clone`](Self::try_clone)).
#[derive(Debug)]
pub struct Packet {
    ptr: NonNull<AVPacket>,
}

impl Packet {
    /// Allocates a new, empty packet.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if allocation fails.
    pub fn new() -> Result<Self, AvError> {
        // SAFETY: `av_packet_alloc` takes no arguments and returns a fresh packet or null.
        let ptr = unsafe { ffi_av_packet_alloc() };
        NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr })
    }

    /// Returns the packet pointer for read-only use.
    #[must_use]
    pub const fn as_ptr(&self) -> *const AVPacket {
        self.ptr.as_ptr()
    }

    /// Returns the packet pointer for mutation and FFI calls.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVPacket {
        self.ptr.as_ptr()
    }

    /// Returns the index of the stream this packet belongs to.
    #[must_use]
    pub fn stream_index(&self) -> std::os::raw::c_int {
        // SAFETY: `self.ptr` is a valid owned packet; `stream_index` is a plain field.
        unsafe { (*self.ptr.as_ptr()).stream_index }
    }

    /// Returns the presentation timestamp (in the stream's time base).
    #[must_use]
    pub fn pts(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned packet; `pts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pts }
    }

    /// Returns the decompression timestamp (in the stream's time base).
    #[must_use]
    pub fn dts(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned packet; `dts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).dts }
    }

    /// Returns the packet's duration (in the stream's time base).
    #[must_use]
    pub fn duration(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned packet; `duration` is a plain field.
        unsafe { (*self.ptr.as_ptr()).duration }
    }

    /// Returns the packet's payload size in bytes.
    #[must_use]
    pub fn size(&self) -> std::os::raw::c_int {
        // SAFETY: `self.ptr` is a valid owned packet; `size` is a plain field.
        unsafe { (*self.ptr.as_ptr()).size }
    }

    /// Returns the packet's flags (a bitmask of `AV_PKT_FLAG_*`).
    #[must_use]
    pub fn flags(&self) -> std::os::raw::c_int {
        // SAFETY: `self.ptr` is a valid owned packet; `flags` is a plain field.
        unsafe { (*self.ptr.as_ptr()).flags }
    }

    /// Sets the index of the stream this packet belongs to.
    pub fn set_stream_index(&mut self, stream_index: std::os::raw::c_int) {
        // SAFETY: `self.ptr` is a valid owned packet; `stream_index` is a plain field.
        unsafe { (*self.ptr.as_ptr()).stream_index = stream_index };
    }

    /// Sets the presentation timestamp (in the stream's time base).
    pub fn set_pts(&mut self, pts: i64) {
        // SAFETY: `self.ptr` is a valid owned packet; `pts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pts = pts };
    }

    /// Sets the decompression timestamp (in the stream's time base).
    pub fn set_dts(&mut self, dts: i64) {
        // SAFETY: `self.ptr` is a valid owned packet; `dts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).dts = dts };
    }

    /// Sets the packet's duration (in the stream's time base).
    pub fn set_duration(&mut self, duration: i64) {
        // SAFETY: `self.ptr` is a valid owned packet; `duration` is a plain field.
        unsafe { (*self.ptr.as_ptr()).duration = duration };
    }

    /// Rescales the packet's `pts` / `dts` / `duration` from `src_tb` to `dst_tb`.
    pub fn rescale_ts(&mut self, src_tb: AVRational, dst_tb: AVRational) {
        // SAFETY: `self.ptr` is a valid owned packet; the time bases are plain
        //         POD values.
        unsafe { ffi_av_packet_rescale_ts(self.ptr.as_ptr(), src_tb, dst_tb) };
    }

    /// Unreferences the packet's buffer, returning it to a blank state.
    pub fn unref(&mut self) {
        // SAFETY: `self.ptr` is a valid owned packet.
        unsafe { ffi_av_packet_unref(self.ptr.as_ptr()) };
    }

    /// Makes a ref-counted copy of this packet (`av_packet_ref`), sharing the
    /// underlying buffer rather than deep-copying.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the copy cannot be allocated / referenced.
    pub fn try_clone(&self) -> Result<Self, AvError> {
        let dst = Self::new()?;
        // SAFETY: `dst` is a fresh blank packet and `self` is a valid packet;
        //         `av_packet_ref` ref-counts `self`'s buffer into `dst`.
        let ret = unsafe { ffi_av_packet_ref(dst.ptr.as_ptr(), self.ptr.as_ptr()) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(dst)
        }
    }
}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the packet (NonNull, not Copy/Clone), so this runs
        //         exactly once. `av_packet_free` frees it and writes null into our
        //         local copy of the pointer, which is then discarded.
        unsafe {
            let mut raw = self.ptr.as_ptr();
            ffi_av_packet_free(&mut raw);
        }
    }
}

// SAFETY: an `AVPacket` is not safe for concurrent access, but moving ownership
//         between threads is sound because Rust's ownership model guarantees
//         exclusive access.
unsafe impl Send for Packet {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_should_allocate_and_drop_cleanly() {
        let packet = Packet::new().expect("packet allocation should succeed");
        assert!(!packet.as_ptr().is_null());
        // Dropping `packet` frees it exactly once (no panic / double free).
    }

    #[test]
    fn try_clone_should_produce_an_independent_owner() {
        let packet = Packet::new().expect("packet allocation should succeed");
        let clone = packet.try_clone().expect("ref-count clone should succeed");
        assert!(!clone.as_ptr().is_null());
        // Both `packet` and `clone` drop independently (ref-counted), no double free.
    }

    #[test]
    fn scalar_setters_should_round_trip() {
        let mut packet = Packet::new().expect("packet allocation should succeed");
        packet.set_stream_index(3);
        packet.set_pts(1_000);
        packet.set_dts(900);
        packet.set_duration(512);
        assert_eq!(packet.stream_index(), 3);
        assert_eq!(packet.pts(), 1_000);
        assert_eq!(packet.dts(), 900);
        assert_eq!(packet.duration(), 512);
    }

    #[test]
    fn size_and_flags_should_read_defaults() {
        let packet = Packet::new().expect("packet allocation should succeed");
        // A fresh packet carries no payload and no flags; the accessors read
        // those plain fields (there is no public setter for either).
        assert_eq!(packet.size(), 0);
        assert_eq!(packet.flags(), 0);
    }

    #[test]
    fn rescale_ts_should_scale_pts_and_dts() {
        let mut packet = Packet::new().expect("packet allocation should succeed");
        packet.set_pts(100);
        packet.set_dts(100);
        // 1/1000 -> 1/2000 doubles the timestamps.
        packet.rescale_ts(
            AVRational { num: 1, den: 1000 },
            AVRational { num: 1, den: 2000 },
        );
        assert_eq!(packet.pts(), 200);
        assert_eq!(packet.dts(), 200);
    }
}
