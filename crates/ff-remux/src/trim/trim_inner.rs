//! Unsafe FFmpeg calls for stream-copy trimming.

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]
// FFmpeg-boundary lints: intentional narrowing/sign casts at the C ABI and
// acronym-heavy FFmpeg doc terms concentrate in this isolated FFI module.
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::doc_markdown)]

use std::path::Path;

use crate::error::RemuxError;

/// Microseconds per second — the `AV_TIME_BASE` unit used by `avformat_seek_file`.
const AV_TIME_BASE: i64 = 1_000_000;

/// Execute stream-copy trim via FFmpeg's muxer/demuxer.
///
/// # Safety
///
/// All FFmpeg pointer invariants are maintained internally.  The function is
/// safe to call from safe Rust — the public `StreamCopyTrimmer::run` wraps it.
pub(crate) fn run_trim(
    input: &Path,
    output: &Path,
    start_sec: f64,
    end_sec: f64,
) -> Result<(), RemuxError> {
    // SAFETY: All pointers are validated (null-checked) before use; resources
    //         are freed on every exit path.
    unsafe { run_trim_unsafe(input, output, start_sec, end_sec) }
}

unsafe fn run_trim_unsafe(
    input: &Path,
    output: &Path,
    start_sec: f64,
    end_sec: f64,
) -> Result<(), RemuxError> {
    // ── Step 1: open input ────────────────────────────────────────────────────
    // SAFETY: input path is provided by the caller; open_input returns a null
    //         on failure and the wrapper converts that to Err.
    let in_ctx = ff_sys::avformat::open_input(input).map_err(RemuxError::from_ffmpeg_error)?;

    // ── Step 2: find stream info ──────────────────────────────────────────────
    // SAFETY: in_ctx is non-null (open_input succeeded).
    if let Err(e) = ff_sys::avformat::find_stream_info(in_ctx) {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 3: allocate output context (owned) ──────────────────────────────
    // The owned context frees itself and closes its IO on drop, so the error
    // paths below only need to close the raw input context. NB: because `in_ctx`
    // is still a raw pointer, out_ctx operations use explicit `match`/`if let`
    // (not `?`) so the manual `close_input` is never skipped.
    let mut out_ctx = match ff_sys::OutputFormatContext::new(None, output) {
        Ok(c) => c,
        Err(e) => {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::from_ffmpeg_error(e.code()));
        }
    };

    // ── Step 4: copy stream parameters ───────────────────────────────────────
    let nb_streams = (*in_ctx).nb_streams as usize;
    for i in 0..nb_streams {
        // SAFETY: i < nb_streams, streams is a valid array of nb_streams pointers.
        let in_stream = *(*in_ctx).streams.add(i);

        // SAFETY: out_ctx is a valid owned mux context.
        let out_stream = ff_sys::avformat_new_stream(out_ctx.as_mut_ptr(), std::ptr::null());
        if out_stream.is_null() {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::Ffmpeg {
                code: 0,
                message: "avformat_new_stream failed".to_string(),
            });
        }

        // SAFETY: both codecpar pointers are non-null (created by FFmpeg).
        let ret = ff_sys::avcodec_parameters_copy((*out_stream).codecpar, (*in_stream).codecpar);
        if ret < 0 {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::from_ffmpeg_error(ret));
        }
        // Clear the codec_tag so the muxer can assign the correct value.
        (*(*out_stream).codecpar).codec_tag = 0;
    }

    // ── Step 5: seek to start ─────────────────────────────────────────────────
    let start_ts = (start_sec * AV_TIME_BASE as f64) as i64;
    // SAFETY: in_ctx is valid; seeking to AV_TIME_BASE-scaled timestamp.
    if let Err(e) = ff_sys::avformat::seek_file(in_ctx, -1, i64::MIN, start_ts, start_ts, 0) {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 6: open output file ──────────────────────────────────────────────
    // SAFETY: output is a valid path; avio_flags::WRITE opens for writing.
    if let Err(e) = out_ctx.open_io(output) {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(e.code()));
    }

    // ── Step 7: write header ──────────────────────────────────────────────────
    // SAFETY: out_ctx is fully configured with streams and pb set.
    if let Err(e) = out_ctx.write_header() {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(e.code()));
    }

    log::debug!("stream copy trim header written nb_streams={nb_streams}");

    // ── Step 8: packet copy loop ──────────────────────────────────────────────
    // SAFETY: av_packet_alloc never returns null on OOM (aborts instead).
    let pkt = ff_sys::av_packet_alloc();
    if pkt.is_null() {
        let _ = out_ctx.write_trailer();
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    }

    let mut loop_err: Option<RemuxError> = None;

    'read: loop {
        // SAFETY: in_ctx and pkt are valid non-null pointers.
        match ff_sys::avformat::read_frame(in_ctx, pkt) {
            Err(e) if e == ff_sys::error_codes::EOF => break 'read,
            Err(e) => {
                loop_err = Some(RemuxError::from_ffmpeg_error(e));
                break 'read;
            }
            Ok(()) => {}
        }

        let stream_idx = (*pkt).stream_index as usize;
        if stream_idx >= nb_streams {
            ff_sys::av_packet_unref(pkt);
            continue;
        }

        // SAFETY: stream_idx < nb_streams; streams arrays are valid.
        let in_stream = *(*in_ctx).streams.add(stream_idx);
        let in_tb = (*in_stream).time_base;

        // Check whether this packet is past the end of the requested range.
        // Prefer PTS; fall back to DTS if PTS is absent.
        let ts = if (*pkt).pts != ff_sys::AV_NOPTS_VALUE {
            (*pkt).pts
        } else {
            (*pkt).dts
        };
        if ts != ff_sys::AV_NOPTS_VALUE && in_tb.den != 0 {
            let ts_sec = ts as f64 * f64::from(in_tb.num) / f64::from(in_tb.den);
            if ts_sec >= end_sec {
                ff_sys::av_packet_unref(pkt);
                break 'read;
            }
        }

        // Rescale timestamps to the output stream's time base.
        // SAFETY: stream_idx < nb_streams; out_ctx is valid.
        let out_stream = *(*out_ctx.as_mut_ptr()).streams.add(stream_idx);
        let out_tb = (*out_stream).time_base;
        // SAFETY: pkt, in_tb, out_tb are valid plain-data values.
        ff_sys::av_packet_rescale_ts(pkt, in_tb, out_tb);
        (*pkt).stream_index = stream_idx as i32;

        // SAFETY: out_ctx and pkt are valid.
        let ret = ff_sys::av_interleaved_write_frame(out_ctx.as_mut_ptr(), pkt);
        ff_sys::av_packet_unref(pkt);
        if ret < 0 {
            loop_err = Some(RemuxError::from_ffmpeg_error(ret));
            break 'read;
        }
    }

    // SAFETY: pkt was allocated by av_packet_alloc above and is still valid.
    let mut pkt_ptr = pkt;
    ff_sys::av_packet_free(&raw mut pkt_ptr);

    // ── Step 9: write trailer ─────────────────────────────────────────────────
    // SAFETY: out_ctx is valid; write_header was called successfully.
    let _ = out_ctx.write_trailer();

    // ── Step 10: cleanup ──────────────────────────────────────────────────────
    // The owned `out_ctx` closes its IO and frees itself when it drops at the end
    // of this scope; only the raw input context needs an explicit close here.
    // SAFETY: in_ctx is non-null (open_input succeeded).
    let mut in_ctx_ptr = in_ctx;
    ff_sys::avformat::close_input(&raw mut in_ctx_ptr);

    log::debug!("stream copy trim complete");

    match loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
