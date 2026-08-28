//! Stream-copy trimming via FFmpeg's muxer/demuxer, using ff-sys safe accessors.

// FFmpeg-boundary lints: intentional narrowing/sign casts at the C ABI and
// acronym-heavy FFmpeg doc terms concentrate in this module.
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
/// The bitstream is stream-copied (no decode/encode cycle). All FFmpeg access
/// goes through owned ff-sys types and their safe accessors, so both contexts
/// free themselves on every exit path.
pub(crate) fn run_trim(
    input: &Path,
    output: &Path,
    start_sec: f64,
    end_sec: f64,
) -> Result<(), RemuxError> {
    // Both the input and output contexts are owned; every early return drops
    // them (closing IO / freeing) with no manual teardown on any path.
    let mut in_ctx = ff_sys::InputFormatContext::open(input)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    in_ctx
        .find_stream_info()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    let mut out_ctx = ff_sys::OutputFormatContext::new(None, output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    let nb_streams = in_ctx.nb_streams() as usize;
    for i in 0..nb_streams {
        let Some(in_stream) = in_ctx.stream(i) else {
            return Err(RemuxError::OperationFailed {
                reason: format!("input stream {i} is missing"),
            });
        };
        let out_idx = out_ctx.new_stream(None).map_err(|_| RemuxError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed".to_string(),
        })?;
        // copy_stream_params deep-copies the parameters and clears codec_tag so
        // the muxer assigns the correct value for the container.
        out_ctx
            .copy_stream_params(out_idx, in_stream.codecpar())
            .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
    }

    let start_ts = (start_sec * AV_TIME_BASE as f64) as i64;
    in_ctx
        .seek_file(-1, i64::MIN, start_ts, start_ts, 0)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    out_ctx
        .open_io(output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    out_ctx
        .write_header()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    log::debug!("stream copy trim header written nb_streams={nb_streams}");

    // The owned packet frees itself exactly once on drop at scope end.
    let Ok(mut pkt) = ff_sys::Packet::new() else {
        let _ = out_ctx.write_trailer();
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    };

    let mut loop_err: Option<RemuxError> = None;

    'read: loop {
        match in_ctx.read_frame(&mut pkt) {
            Err(e) if e.is_eof() => break 'read,
            Err(e) => {
                loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                break 'read;
            }
            Ok(()) => {}
        }

        let stream_idx = pkt.stream_index() as usize;
        if stream_idx >= nb_streams {
            pkt.unref();
            continue;
        }

        let Some(in_stream) = in_ctx.stream(stream_idx) else {
            pkt.unref();
            continue;
        };
        let in_tb = in_stream.time_base();

        // Check whether this packet is past the end of the requested range.
        // Prefer PTS; fall back to DTS if PTS is absent.
        let ts = if pkt.pts() != ff_sys::AV_NOPTS_VALUE {
            pkt.pts()
        } else {
            pkt.dts()
        };
        if ts != ff_sys::AV_NOPTS_VALUE && in_tb.den != 0 {
            let ts_sec = ts as f64 * f64::from(in_tb.num) / f64::from(in_tb.den);
            if ts_sec >= end_sec {
                pkt.unref();
                break 'read;
            }
        }

        // Rescale timestamps to the output stream's time base.
        let out_tb = out_ctx.stream_time_base(stream_idx);
        pkt.rescale_ts(in_tb, out_tb);
        pkt.set_stream_index(stream_idx as i32);

        let write_res = out_ctx.write_interleaved(&mut pkt);
        // av_interleaved_write_frame takes the packet's buf reference; unref to clear.
        pkt.unref();
        if let Err(e) = write_res {
            loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
            break 'read;
        }
    }

    let _ = out_ctx.write_trailer();

    // The owned `in_ctx` / `out_ctx` close their IO and free themselves when they
    // drop at the end of this scope; no manual teardown is needed.
    log::debug!("stream copy trim complete");

    match loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
