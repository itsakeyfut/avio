//! RAII owner for an `AVPacket`.
//!
//! [`Packet`] allocates a packet and frees it exactly once on drop, replacing
//! the manual `av_packet_alloc` + `av_packet_free` pair. Ownership is unique;
//! [`try_clone`](Packet::try_clone) makes a ref-counted copy (`av_packet_ref`).
//! Scalar fields ([`stream_index`](Packet::stream_index) / [`pts`](Packet::pts))
//! are read through the typed accessors below.

use std::ptr::NonNull;

use crate::{
    AVPacket, AvError, av_packet_alloc as ffi_av_packet_alloc,
    av_packet_free as ffi_av_packet_free, av_packet_ref as ffi_av_packet_ref,
    av_packet_unref as ffi_av_packet_unref,
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
}
