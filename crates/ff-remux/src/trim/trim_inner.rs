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

/// The bitstream filter chains to apply, per media type.
///
/// Empty by default: `FFmpeg` already inserts the filter a container requires (a
/// muxer's `check_bitstream` callback runs from every `av_*write_frame` path under
/// `AVFMT_FLAG_AUTO_BSF`), so nothing here is needed for a correct container change.
/// These carry the filters `FFmpeg` never applies by itself. See ADR-0011.
#[derive(Debug, Default, Clone)]
pub(crate) struct BsfSpec {
    pub(crate) video: Option<String>,
    pub(crate) audio: Option<String>,
}

impl BsfSpec {
    /// Returns the chain to apply to a stream of `codec_type`, if any.
    fn for_media_type(&self, codec_type: ff_sys::AVMediaType) -> Option<&str> {
        if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO {
            self.video.as_deref()
        } else if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO {
            self.audio.as_deref()
        } else {
            None
        }
    }
}

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
    bsf: &BsfSpec,
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
    // One slot per input stream; `None` where no filter was requested. Each filter is
    // built *before* its output stream is configured, because `av_bsf_init` may rewrite
    // the stream description (h264_mp4toannexb replaces the avcC extradata with Annex B
    // parameter sets), and the muxer must be told the filtered shape, not the input's.
    let mut filters: Vec<Option<ff_sys::BsfContext>> = Vec::with_capacity(nb_streams);
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
        let par = in_stream.codecpar();
        // copy_stream_params deep-copies the parameters and clears codec_tag so
        // the muxer assigns the correct value for the container.
        if let Some(spec) = bsf.for_media_type(par.codec_type()) {
            // The only thing that can be wrong here is the caller's spec, so report it
            // as a configuration error rather than a raw FFmpeg code.
            let filter =
                ff_sys::BsfContext::open(spec, Some(par), in_stream.time_base()).map_err(|e| {
                    RemuxError::InvalidConfig {
                        reason: format!(
                            "bitstream filter {spec:?} for stream {i}: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    }
                })?;
            out_ctx
                .copy_stream_params(out_idx, filter.output_params())
                .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
            filters.push(Some(filter));
        } else {
            out_ctx
                .copy_stream_params(out_idx, par)
                .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
            filters.push(None);
        }
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

        let out_tb = out_ctx.stream_time_base(stream_idx);

        let Some(filter) = filters[stream_idx].as_mut() else {
            // Unfiltered: rescale timestamps to the output stream's time base.
            pkt.rescale_ts(in_tb, out_tb);
            pkt.set_stream_index(stream_idx as i32);

            let write_res = out_ctx.write_interleaved(&mut pkt);
            // av_interleaved_write_frame takes the packet's buf reference; unref to clear.
            pkt.unref();
            if let Err(e) = write_res {
                loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                break 'read;
            }
            continue;
        };

        // Filtered. `send_packet` moves the payload into the filter and blanks `pkt`,
        // so there is nothing to unref here and `pkt` is free to receive the output.
        if let Err(e) = filter.send_packet(&mut pkt) {
            loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
            break 'read;
        }
        let bsf_tb = filter.output_time_base();
        loop {
            match filter.receive_packet(&mut pkt) {
                // One input packet may yield several outputs, or none.
                Ok(ff_sys::ReceiveOutcome::Frame) => {}
                Ok(_) => break,
                Err(e) => {
                    loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                    break 'read;
                }
            }
            // The filter emits in its own output time base, which need not be the
            // input's.
            pkt.rescale_ts(bsf_tb, out_tb);
            pkt.set_stream_index(stream_idx as i32);
            let write_res = out_ctx.write_interleaved(&mut pkt);
            pkt.unref();
            if let Err(e) = write_res {
                loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                break 'read;
            }
        }
    }

    // Flush every filter before the trailer: a filter may hold packets back, and they
    // belong in the output. Skipped when the read loop already failed, since the output
    // is being abandoned anyway.
    if loop_err.is_none() {
        for (idx, slot) in filters.iter_mut().enumerate() {
            let Some(filter) = slot.as_mut() else {
                continue;
            };
            if let Err(e) = filter.send_eof() {
                loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                break;
            }
            let bsf_tb = filter.output_time_base();
            let out_tb = out_ctx.stream_time_base(idx);
            loop {
                match filter.receive_packet(&mut pkt) {
                    Ok(ff_sys::ReceiveOutcome::Frame) => {}
                    Ok(_) => break,
                    Err(e) => {
                        loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                        break;
                    }
                }
                pkt.rescale_ts(bsf_tb, out_tb);
                pkt.set_stream_index(idx as i32);
                let write_res = out_ctx.write_interleaved(&mut pkt);
                pkt.unref();
                if let Err(e) = write_res {
                    loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                    break;
                }
            }
            if loop_err.is_some() {
                break;
            }
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
