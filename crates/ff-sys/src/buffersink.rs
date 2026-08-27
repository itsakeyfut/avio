//! Safe drain wrapper over `av_buffersink_get_frame`.
//!
//! [`buffersink_get_frame`] pulls one filtered frame out of a buffersink filter
//! into an owned [`Frame`], mapping FFmpeg's `EAGAIN` / `EOF` drain states to the
//! named [`BufferSinkOutcome`] variants so callers never branch on raw return
//! codes. It mirrors [`CodecContext::receive_packet`](crate::CodecContext::receive_packet)
//! and its [`ReceiveOutcome`](crate::ReceiveOutcome).

use std::os::raw::c_int;

use crate::{AVFilterContext, AvError, Frame};

/// The outcome of a [`buffersink_get_frame`] call.
///
/// Encodes FFmpeg's `EAGAIN` / `EOF` drain states as named variants so callers
/// never branch on raw return codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferSinkOutcome {
    /// A filtered frame was written into `dst`.
    Frame,
    /// The sink needs more input (`EAGAIN`): push more frames into the graph
    /// before pulling again.
    NeedMore,
    /// The graph is fully drained (`EOF`): no more frames will be produced.
    Drained,
}

/// Pulls one filtered frame from a buffersink into `dst`, returning a typed
/// [`BufferSinkOutcome`].
///
/// `EAGAIN` (need input) and `EOF` (drained) are returned as
/// [`BufferSinkOutcome::NeedMore`] / [`BufferSinkOutcome::Drained`] rather than
/// errors; only other negative codes are `Err`.
///
/// # Safety
///
/// `sink` must be a valid `*mut AVFilterContext` for a buffersink filter.
// seal-allow-raw: the filter graph (AVFilterContext / buffersrc / buffersink) is
// not yet a sealed owned type, so this drain wrapper still takes a raw sink
// pointer. Tracked with the broader filter-graph RAII work, separate from #1506.
pub unsafe fn buffersink_get_frame(
    sink: *mut AVFilterContext,
    dst: &mut Frame,
) -> Result<BufferSinkOutcome, AvError> {
    // SAFETY: the caller upholds `sink`; `dst` owns a valid `AVFrame`, so
    //         `dst.as_mut_ptr()` is a valid destination for the filtered frame.
    let ret = unsafe { crate::av_buffersink_get_frame(sink, dst.as_mut_ptr()) };
    classify_get_frame(ret)
}

/// Maps a raw `av_buffersink_get_frame` result to a [`BufferSinkOutcome`].
///
/// `EAGAIN` and `EOF` are expected drain states, not errors; any other negative
/// code is a real error.
fn classify_get_frame(ret: c_int) -> Result<BufferSinkOutcome, AvError> {
    match ret {
        r if r >= 0 => Ok(BufferSinkOutcome::Frame),
        r if r == crate::error_codes::EAGAIN => Ok(BufferSinkOutcome::NeedMore),
        r if r == crate::error_codes::EOF => Ok(BufferSinkOutcome::Drained),
        r => Err(AvError::new(r)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_get_frame_should_map_drain_states() {
        assert!(matches!(
            classify_get_frame(0),
            Ok(BufferSinkOutcome::Frame)
        ));
        assert!(matches!(
            classify_get_frame(1),
            Ok(BufferSinkOutcome::Frame)
        ));
        assert!(matches!(
            classify_get_frame(crate::error_codes::EAGAIN),
            Ok(BufferSinkOutcome::NeedMore)
        ));
        assert!(matches!(
            classify_get_frame(crate::error_codes::EOF),
            Ok(BufferSinkOutcome::Drained)
        ));
    }

    #[test]
    fn classify_get_frame_should_return_err_on_other_negative_code() {
        // EINVAL (-22) is a real error, not a drain state; the original code is
        // preserved in the returned AvError (mirrors classify_receive's test).
        assert_eq!(classify_get_frame(-22), Err(AvError::new(-22)));
    }
}
