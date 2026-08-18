//! Unsafe FFmpeg calls for stream-copy trimming.

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::cast_precision_loss)]

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
        ff_sys::avformat::close_input(&mut p);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 3: allocate output context ──────────────────────────────────────
    let Some(output_str) = output.to_str() else {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&mut p);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path is not valid UTF-8".to_string(),
        });
    };
    let Ok(c_output) = std::ffi::CString::new(output_str) else {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&mut p);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path contains null bytes".to_string(),
        });
    };

    let mut out_ctx: *mut ff_sys::AVFormatContext = std::ptr::null_mut();
    // SAFETY: c_output is a valid null-terminated C string.
    let ret = ff_sys::avformat_alloc_output_context2(
        &mut out_ctx,
        std::ptr::null_mut(),
        std::ptr::null(),
        c_output.as_ptr(),
    );
    if ret < 0 || out_ctx.is_null() {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&mut p);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }

    // ── Step 4: copy stream parameters ───────────────────────────────────────
    let nb_streams = (*in_ctx).nb_streams as usize;
    for i in 0..nb_streams {
        // SAFETY: i < nb_streams, streams is a valid array of nb_streams pointers.
        let in_stream = *(*in_ctx).streams.add(i);

        // SAFETY: out_ctx is non-null (avformat_alloc_output_context2 succeeded).
        let out_stream = ff_sys::avformat_new_stream(out_ctx, std::ptr::null());
        if out_stream.is_null() {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&mut p);
            ff_sys::avformat_free_context(out_ctx);
            return Err(RemuxError::Ffmpeg {
                code: 0,
                message: "avformat_new_stream failed".to_string(),
            });
        }

        // SAFETY: both codecpar pointers are non-null (created by FFmpeg).
        let ret = ff_sys::avcodec_parameters_copy((*out_stream).codecpar, (*in_stream).codecpar);
        if ret < 0 {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&mut p);
            ff_sys::avformat_free_context(out_ctx);
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
        ff_sys::avformat::close_input(&mut p);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 6: open output file ──────────────────────────────────────────────
    // SAFETY: output is a valid path; avio_flags::WRITE opens for writing.
    let pb = match ff_sys::avformat::open_output(output, ff_sys::avformat::avio_flags::WRITE) {
        Ok(pb) => pb,
        Err(e) => {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&mut p);
            ff_sys::avformat_free_context(out_ctx);
            return Err(RemuxError::from_ffmpeg_error(e));
        }
    };
    // SAFETY: out_ctx is non-null; pb is a valid AVIOContext.
    (*out_ctx).pb = pb;

    // ── Step 7: write header ──────────────────────────────────────────────────
    // SAFETY: out_ctx is fully configured with streams and pb set.
    let ret = ff_sys::avformat_write_header(out_ctx, std::ptr::null_mut());
    if ret < 0 {
        // SAFETY: (*out_ctx).pb was set above and is non-null.
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&mut p);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }

    log::debug!("stream copy trim header written nb_streams={nb_streams}");

    // ── Step 8: packet copy loop ──────────────────────────────────────────────
    // SAFETY: av_packet_alloc never returns null on OOM (aborts instead).
    let pkt = ff_sys::av_packet_alloc();
    if pkt.is_null() {
        ff_sys::av_write_trailer(out_ctx);
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&mut p);
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
        let out_stream = *(*out_ctx).streams.add(stream_idx);
        let out_tb = (*out_stream).time_base;
        // SAFETY: pkt, in_tb, out_tb are valid plain-data values.
        ff_sys::av_packet_rescale_ts(pkt, in_tb, out_tb);
        (*pkt).stream_index = stream_idx as i32;

        // SAFETY: out_ctx and pkt are valid.
        let ret = ff_sys::av_interleaved_write_frame(out_ctx, pkt);
        ff_sys::av_packet_unref(pkt);
        if ret < 0 {
            loop_err = Some(RemuxError::from_ffmpeg_error(ret));
            break 'read;
        }
    }

    // SAFETY: pkt was allocated by av_packet_alloc above and is still valid.
    let mut pkt_ptr = pkt;
    ff_sys::av_packet_free(&mut pkt_ptr);

    // ── Step 9: write trailer ─────────────────────────────────────────────────
    // SAFETY: out_ctx is valid; write_header was called successfully.
    ff_sys::av_write_trailer(out_ctx);

    // ── Step 10: cleanup ──────────────────────────────────────────────────────
    // SAFETY: (*out_ctx).pb is non-null (opened above; still set if write_header passed).
    ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
    // SAFETY: out_ctx is non-null and was allocated by avformat_alloc_output_context2.
    ff_sys::avformat_free_context(out_ctx);
    // SAFETY: in_ctx is non-null (open_input succeeded).
    let mut in_ctx_ptr = in_ctx;
    ff_sys::avformat::close_input(&mut in_ctx_ptr);

    log::debug!("stream copy trim complete");

    match loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
