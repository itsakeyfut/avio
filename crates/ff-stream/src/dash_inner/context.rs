//! `AVFormatContext` DASH setup/cleanup helpers and the `RenditionState` type.

use std::ptr;

use ff_sys::{AVFormatContext, AVFrame, AVPixelFormat};
use ff_sys::{av_frame_free, avformat_free_context};

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

// ============================================================================
// Cleanup helpers (safe to call with null pointers)
// ============================================================================

pub(super) unsafe fn cleanup_output_ctx(mut out_ctx: *mut AVFormatContext) {
    if !out_ctx.is_null() {
        avformat_free_context(out_ctx);
        out_ctx = ptr::null_mut();
        let _ = out_ctx; // suppress unused warning
    }
}

pub(super) unsafe fn free_frames(
    mut vid_dec: *mut AVFrame,
    mut vid_enc: *mut AVFrame,
    mut aud_dec: *mut AVFrame,
    mut aud_enc: *mut AVFrame,
) {
    if !vid_dec.is_null() {
        av_frame_free(&mut vid_dec as *mut *mut _);
    }
    if !vid_enc.is_null() {
        av_frame_free(&mut vid_enc as *mut *mut _);
    }
    if !aud_dec.is_null() {
        av_frame_free(&mut aud_dec as *mut *mut _);
    }
    if !aud_enc.is_null() {
        av_frame_free(&mut aud_enc as *mut *mut _);
    }
}
