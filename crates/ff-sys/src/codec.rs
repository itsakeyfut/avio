//! Borrowed handle to a static FFmpeg `AVCodec`.
//!
//! [`Codec`] wraps a `*const AVCodec` returned by a finder such as
//! [`find_decoder`](Codec::find_decoder). FFmpeg codecs are static singletons
//! owned by the library, so this is a `Copy` borrowed handle with no `Drop`; it
//! replaces the raw `*const AVCodec` in the safe API.

use std::ptr::NonNull;

use crate::{AVCodec, AVCodecID};

/// A borrowed handle to a static FFmpeg codec.
#[derive(Clone, Copy, Debug)]
pub struct Codec {
    ptr: NonNull<AVCodec>,
}

impl Codec {
    /// Finds a decoder by codec id, returning `None` when none is registered.
    ///
    /// This is a pure table lookup with no precondition, so it is a safe fn.
    #[must_use]
    pub fn find_decoder(codec_id: AVCodecID) -> Option<Self> {
        // SAFETY: `find_decoder` is a pure table lookup that dereferences no
        //         caller pointer; it returns a valid static codec pointer or `None`.
        let ptr = unsafe { crate::avcodec::find_decoder(codec_id) }?;
        NonNull::new(ptr.cast_mut()).map(|ptr| Self { ptr })
    }

    /// Returns the underlying codec pointer for FFI calls.
    #[must_use]
    pub const fn as_ptr(&self) -> *const AVCodec {
        self.ptr.as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_decoder_should_return_none_for_codec_none() {
        // `AV_CODEC_ID_NONE` is the sentinel with no registered decoder, so the
        // lookup is deterministically empty regardless of the linked FFmpeg build.
        let result = Codec::find_decoder(crate::AVCodecID_AV_CODEC_ID_NONE);
        assert!(result.is_none());
    }
}
