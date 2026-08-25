//! Per-rendition encoder state for the ABR DASH mux loop.

use ff_sys::AVPixelFormat;

// ============================================================================
// Per-rendition encoder state for ABR
// ============================================================================

/// Per-rendition encoder state for the ABR DASH mux loop.
///
/// The owned `vid_enc_ctx` / `sws_ctx` are freed on drop, so a `Vec<RenditionState>`
/// releases every rendition's contexts automatically when it goes out of scope.
pub(super) struct RenditionState {
    pub(super) vid_enc_ctx: ff_sys::CodecContext,
    pub(super) vid_out_stream_idx: i32,
    pub(super) enc_width: i32,
    pub(super) enc_height: i32,
    pub(super) sws_ctx: Option<ff_sys::ScaleContext>,
    pub(super) last_src_fmt: Option<AVPixelFormat>,
    pub(super) last_src_w: Option<i32>,
    pub(super) last_src_h: Option<i32>,
}
