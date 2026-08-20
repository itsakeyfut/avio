//! Unsafe FFmpeg calls for audio stream operations (replacement, extraction, addition).

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]
// FFmpeg-boundary lints: intentional narrowing/sign casts at the C ABI and
// acronym-heavy FFmpeg doc terms concentrate in this isolated FFI module.
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::doc_markdown)]

use std::path::Path;

use crate::error::RemuxError;

/// Replace the audio stream of `video_input` with the audio from `audio_input`,
/// writing the combined result to `output`.
///
/// # Safety
///
/// All FFmpeg pointer invariants are maintained internally.  The public
/// `AudioReplacement::run` wraps this function safely.
pub(crate) fn run_audio_replacement(
    video_input: &Path,
    audio_input: &Path,
    output: &Path,
) -> Result<(), RemuxError> {
    // SAFETY: All pointers are validated (null-checked) before use; resources
    //         are freed on every exit path.
    unsafe { run_audio_replacement_unsafe(video_input, audio_input, output) }
}

unsafe fn run_audio_replacement_unsafe(
    video_input: &Path,
    audio_input: &Path,
    output: &Path,
) -> Result<(), RemuxError> {
    // ── Step 1: open video input ──────────────────────────────────────────────
    // SAFETY: video_input is a caller-supplied path; open_input returns Err on failure.
    let vid_ctx =
        ff_sys::avformat::open_input(video_input).map_err(RemuxError::from_ffmpeg_error)?;

    // ── Step 2: find stream info for video input ──────────────────────────────
    // SAFETY: vid_ctx is non-null (open_input succeeded).
    if let Err(e) = ff_sys::avformat::find_stream_info(vid_ctx) {
        let mut p = vid_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 3: locate the first video stream ─────────────────────────────────
    // SAFETY: nb_streams is the valid count; streams is a valid array of that length.
    let nb_vid_streams = (*vid_ctx).nb_streams as usize;
    let mut video_stream_idx: Option<usize> = None;
    for i in 0..nb_vid_streams {
        let stream = *(*vid_ctx).streams.add(i);
        if (*(*stream).codecpar).codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO {
            video_stream_idx = Some(i);
            break;
        }
    }
    let Some(video_stream_idx) = video_stream_idx else {
        let mut p = vid_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "no video stream found in video input path={}",
                video_input.display()
            ),
        });
    };

    // ── Step 4: open audio input ──────────────────────────────────────────────
    // SAFETY: audio_input is a caller-supplied path; open_input returns Err on failure.
    let aud_ctx = match ff_sys::avformat::open_input(audio_input) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut p = vid_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::from_ffmpeg_error(e));
        }
    };

    // ── Step 5: find stream info for audio input ──────────────────────────────
    // SAFETY: aud_ctx is non-null (open_input succeeded).
    if let Err(e) = ff_sys::avformat::find_stream_info(aud_ctx) {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 6: locate the first audio stream ─────────────────────────────────
    // SAFETY: nb_streams is the valid count; streams is a valid array of that length.
    let nb_aud_streams = (*aud_ctx).nb_streams as usize;
    let mut audio_stream_idx: Option<usize> = None;
    for i in 0..nb_aud_streams {
        let stream = *(*aud_ctx).streams.add(i);
        if (*(*stream).codecpar).codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO {
            audio_stream_idx = Some(i);
            break;
        }
    }
    let Some(audio_stream_idx) = audio_stream_idx else {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "no audio stream found in audio input path={}",
                audio_input.display()
            ),
        });
    };

    // ── Step 7: allocate output context ──────────────────────────────────────
    let Some(output_str) = output.to_str() else {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path is not valid UTF-8".to_string(),
        });
    };
    let Ok(c_output) = std::ffi::CString::new(output_str) else {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path contains null bytes".to_string(),
        });
    };

    let mut out_ctx: *mut ff_sys::AVFormatContext = std::ptr::null_mut();
    // SAFETY: c_output is a valid null-terminated C string.
    let ret = ff_sys::avformat_alloc_output_context2(
        &raw mut out_ctx,
        std::ptr::null_mut(),
        std::ptr::null(),
        c_output.as_ptr(),
    );
    if ret < 0 || out_ctx.is_null() {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }

    // ── Step 8: copy video stream parameters to output ────────────────────────
    // SAFETY: video_stream_idx < nb_vid_streams; streams is a valid array.
    let vid_in_stream = *(*vid_ctx).streams.add(video_stream_idx);
    // SAFETY: out_ctx is non-null (avformat_alloc_output_context2 succeeded).
    let vid_out_stream = ff_sys::avformat_new_stream(out_ctx, std::ptr::null());
    if vid_out_stream.is_null() {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed for video".to_string(),
        });
    }
    // SAFETY: both codecpar pointers are non-null (created by FFmpeg).
    let ret =
        ff_sys::avcodec_parameters_copy((*vid_out_stream).codecpar, (*vid_in_stream).codecpar);
    if ret < 0 {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }
    // Clear codec_tag so the muxer assigns the correct value for the container.
    (*(*vid_out_stream).codecpar).codec_tag = 0;

    // ── Step 9: copy audio stream parameters to output ────────────────────────
    // SAFETY: audio_stream_idx < nb_aud_streams; streams is a valid array.
    let aud_in_stream = *(*aud_ctx).streams.add(audio_stream_idx);
    // SAFETY: out_ctx is non-null (avformat_alloc_output_context2 succeeded).
    let aud_out_stream = ff_sys::avformat_new_stream(out_ctx, std::ptr::null());
    if aud_out_stream.is_null() {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed for audio".to_string(),
        });
    }
    // SAFETY: both codecpar pointers are non-null (created by FFmpeg).
    let ret =
        ff_sys::avcodec_parameters_copy((*aud_out_stream).codecpar, (*aud_in_stream).codecpar);
    if ret < 0 {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }
    // Clear codec_tag so the muxer assigns the correct value for the container.
    (*(*aud_out_stream).codecpar).codec_tag = 0;

    // ── Step 10: open output file ─────────────────────────────────────────────
    // SAFETY: output is a valid path; WRITE opens the file for writing.
    let pb = match ff_sys::avformat::open_output(output, ff_sys::avformat::avio_flags::WRITE) {
        Ok(pb) => pb,
        Err(e) => {
            let mut pv = vid_ctx;
            ff_sys::avformat::close_input(&raw mut pv);
            let mut pa = aud_ctx;
            ff_sys::avformat::close_input(&raw mut pa);
            ff_sys::avformat_free_context(out_ctx);
            return Err(RemuxError::from_ffmpeg_error(e));
        }
    };
    // SAFETY: out_ctx is non-null; pb is a valid AVIOContext.
    (*out_ctx).pb = pb;

    // ── Step 11: write header ─────────────────────────────────────────────────
    // SAFETY: out_ctx is fully configured with streams and pb set.
    let ret = ff_sys::avformat_write_header(out_ctx, std::ptr::null_mut());
    if ret < 0 {
        // SAFETY: (*out_ctx).pb was set above and is non-null.
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }

    // Read time bases after avformat_write_header — the muxer may adjust them.
    // SAFETY: stream pointers remain valid for the lifetime of their parent contexts.
    let vid_in_tb = (*vid_in_stream).time_base;
    let aud_in_tb = (*aud_in_stream).time_base;
    let vid_out_tb = (*vid_out_stream).time_base;
    let aud_out_tb = (*aud_out_stream).time_base;

    log::debug!(
        "audio replacement header written \
         video_stream_idx={video_stream_idx} audio_stream_idx={audio_stream_idx}"
    );

    // ── Step 12: allocate packet ──────────────────────────────────────────────
    // SAFETY: av_packet_alloc never returns null in practice (aborts on OOM).
    let pkt = ff_sys::av_packet_alloc();
    if pkt.is_null() {
        ff_sys::av_write_trailer(out_ctx);
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    }

    // ── Step 13: interleaved packet copy loop ─────────────────────────────────
    // Alternate between video and audio inputs; use av_interleaved_write_frame
    // so the muxer buffers and flushes packets in the correct timestamp order.
    let mut loop_err: Option<RemuxError> = None;
    let mut vid_eof = false;
    let mut aud_eof = false;

    'copy: loop {
        // Read one packet from the video input, forwarding only the target stream.
        if !vid_eof {
            // SAFETY: vid_ctx and pkt are valid non-null pointers.
            match ff_sys::avformat::read_frame(vid_ctx, pkt) {
                Err(e) if e == ff_sys::error_codes::EOF => {
                    vid_eof = true;
                }
                Err(e) => {
                    loop_err = Some(RemuxError::from_ffmpeg_error(e));
                    break 'copy;
                }
                Ok(()) => {
                    if (*pkt).stream_index as usize == video_stream_idx {
                        // SAFETY: pkt, vid_in_tb, vid_out_tb are valid plain-data values.
                        ff_sys::av_packet_rescale_ts(pkt, vid_in_tb, vid_out_tb);
                        (*pkt).stream_index = 0;
                        // SAFETY: out_ctx and pkt are valid.
                        let ret = ff_sys::av_interleaved_write_frame(out_ctx, pkt);
                        // av_interleaved_write_frame takes the packet's buf reference;
                        // unref to clear any remaining fields.
                        ff_sys::av_packet_unref(pkt);
                        if ret < 0 {
                            loop_err = Some(RemuxError::from_ffmpeg_error(ret));
                            break 'copy;
                        }
                    } else {
                        ff_sys::av_packet_unref(pkt);
                    }
                }
            }
        }

        // Read one packet from the audio input, forwarding only the target stream.
        if !aud_eof {
            // SAFETY: aud_ctx and pkt are valid non-null pointers.
            match ff_sys::avformat::read_frame(aud_ctx, pkt) {
                Err(e) if e == ff_sys::error_codes::EOF => {
                    aud_eof = true;
                }
                Err(e) => {
                    loop_err = Some(RemuxError::from_ffmpeg_error(e));
                    break 'copy;
                }
                Ok(()) => {
                    if (*pkt).stream_index as usize == audio_stream_idx {
                        // SAFETY: pkt, aud_in_tb, aud_out_tb are valid plain-data values.
                        ff_sys::av_packet_rescale_ts(pkt, aud_in_tb, aud_out_tb);
                        (*pkt).stream_index = 1;
                        // SAFETY: out_ctx and pkt are valid.
                        let ret = ff_sys::av_interleaved_write_frame(out_ctx, pkt);
                        ff_sys::av_packet_unref(pkt);
                        if ret < 0 {
                            loop_err = Some(RemuxError::from_ffmpeg_error(ret));
                            break 'copy;
                        }
                    } else {
                        ff_sys::av_packet_unref(pkt);
                    }
                }
            }
        }

        if vid_eof && aud_eof {
            break 'copy;
        }
    }

    // SAFETY: pkt was allocated by av_packet_alloc above and is still valid.
    let mut pkt_ptr = pkt;
    ff_sys::av_packet_free(&raw mut pkt_ptr);

    // ── Step 14: write trailer ────────────────────────────────────────────────
    // SAFETY: out_ctx is valid; write_header was called successfully.
    ff_sys::av_write_trailer(out_ctx);

    // ── Step 15: cleanup ──────────────────────────────────────────────────────
    // SAFETY: (*out_ctx).pb is non-null (opened above; still set after write_header).
    ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
    // SAFETY: out_ctx is non-null and was allocated by avformat_alloc_output_context2.
    ff_sys::avformat_free_context(out_ctx);
    // SAFETY: vid_ctx and aud_ctx are non-null (open_input succeeded).
    let mut pv = vid_ctx;
    ff_sys::avformat::close_input(&raw mut pv);
    let mut pa = aud_ctx;
    ff_sys::avformat::close_input(&raw mut pa);

    log::info!("audio replaced output={}", output.display());

    match loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ── Audio extraction ──────────────────────────────────────────────────────────

/// Demux the audio track at `stream_index` (or the first audio stream when
/// `stream_index` is `None`) from `input` and write it to `output`.
///
/// The audio bitstream is stream-copied (no decode/encode cycle).
///
/// # Safety
///
/// All FFmpeg pointer invariants are maintained internally.  The public
/// `AudioExtractor::run` wraps this function safely.
pub(crate) fn run_audio_extraction(
    input: &Path,
    output: &Path,
    stream_index: Option<usize>,
) -> Result<(), RemuxError> {
    // SAFETY: All pointers are validated (null-checked) before use; resources
    //         are freed on every exit path.
    unsafe { run_audio_extraction_unsafe(input, output, stream_index) }
}

unsafe fn run_audio_extraction_unsafe(
    input: &Path,
    output: &Path,
    requested_idx: Option<usize>,
) -> Result<(), RemuxError> {
    // ── Step 1: open input ────────────────────────────────────────────────────
    // SAFETY: input is a caller-supplied path; open_input returns Err on failure.
    let in_ctx = ff_sys::avformat::open_input(input).map_err(RemuxError::from_ffmpeg_error)?;

    // ── Step 2: find stream info ──────────────────────────────────────────────
    // SAFETY: in_ctx is non-null (open_input succeeded).
    if let Err(e) = ff_sys::avformat::find_stream_info(in_ctx) {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 3: locate the audio stream ──────────────────────────────────────
    // SAFETY: nb_streams is the valid count; streams is a valid array of that length.
    let nb_streams = (*in_ctx).nb_streams as usize;
    let audio_stream_idx = if let Some(idx) = requested_idx {
        // Validate that the requested index is actually an audio stream.
        if idx >= nb_streams {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::OperationFailed {
                reason: format!("stream index {idx} out of range (input has {nb_streams} streams)"),
            });
        }
        let stream = *(*in_ctx).streams.add(idx);
        if (*(*stream).codecpar).codec_type != ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::OperationFailed {
                reason: format!("stream index {idx} is not an audio stream"),
            });
        }
        idx
    } else {
        // Find the first audio stream.
        let mut found: Option<usize> = None;
        for i in 0..nb_streams {
            let stream = *(*in_ctx).streams.add(i);
            if (*(*stream).codecpar).codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO {
                found = Some(i);
                break;
            }
        }
        if let Some(idx) = found {
            idx
        } else {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::OperationFailed {
                reason: format!("no audio stream found in input path={}", input.display()),
            });
        }
    };

    // ── Step 4: allocate output context ──────────────────────────────────────
    let Some(output_str) = output.to_str() else {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path is not valid UTF-8".to_string(),
        });
    };
    let Ok(c_output) = std::ffi::CString::new(output_str) else {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path contains null bytes".to_string(),
        });
    };

    let mut out_ctx: *mut ff_sys::AVFormatContext = std::ptr::null_mut();
    // SAFETY: c_output is a valid null-terminated C string; format is auto-detected from ext.
    let ret = ff_sys::avformat_alloc_output_context2(
        &raw mut out_ctx,
        std::ptr::null_mut(),
        std::ptr::null(),
        c_output.as_ptr(),
    );
    if ret < 0 || out_ctx.is_null() {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }

    // ── Step 5: copy audio stream parameters to output ────────────────────────
    // SAFETY: audio_stream_idx < nb_streams; streams is a valid array.
    let in_stream = *(*in_ctx).streams.add(audio_stream_idx);
    // SAFETY: out_ctx is non-null (avformat_alloc_output_context2 succeeded).
    let out_stream = ff_sys::avformat_new_stream(out_ctx, std::ptr::null());
    if out_stream.is_null() {
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
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
        ff_sys::avformat::close_input(&raw mut p);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }
    // Clear codec_tag so the muxer assigns the correct value for the container.
    (*(*out_stream).codecpar).codec_tag = 0;

    // ── Step 6: open output file ──────────────────────────────────────────────
    // SAFETY: output is a valid path; WRITE opens the file for writing.
    let pb = match ff_sys::avformat::open_output(output, ff_sys::avformat::avio_flags::WRITE) {
        Ok(pb) => pb,
        Err(e) => {
            let mut p = in_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            ff_sys::avformat_free_context(out_ctx);
            return Err(RemuxError::from_ffmpeg_error(e));
        }
    };
    // SAFETY: out_ctx is non-null; pb is a valid AVIOContext.
    (*out_ctx).pb = pb;

    // ── Step 7: write header ──────────────────────────────────────────────────
    // A non-zero return here usually means the codec is incompatible with the
    // chosen output container.  Wrap it as MediaOperationFailed with a clear
    // message so callers know what went wrong.
    // SAFETY: out_ctx is fully configured with the stream and pb set.
    let ret = ff_sys::avformat_write_header(out_ctx, std::ptr::null_mut());
    if ret < 0 {
        // SAFETY: (*out_ctx).pb was set above and is non-null.
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "codec incompatible with output container: {}",
                ff_sys::av_error_string(ret)
            ),
        });
    }

    // Read time bases after avformat_write_header — the muxer may adjust them.
    // SAFETY: stream pointers remain valid for the lifetime of their parent contexts.
    let in_tb = (*in_stream).time_base;
    let out_tb = (*out_stream).time_base;

    log::debug!(
        "audio extraction header written audio_stream_idx={audio_stream_idx} \
         output={}",
        output.display()
    );

    // ── Step 8: allocate packet ───────────────────────────────────────────────
    // SAFETY: av_packet_alloc never returns null in practice (aborts on OOM).
    let pkt = ff_sys::av_packet_alloc();
    if pkt.is_null() {
        ff_sys::av_write_trailer(out_ctx);
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut p = in_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    }

    // ── Step 9: packet copy loop (audio stream only) ──────────────────────────
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

        if (*pkt).stream_index as usize != audio_stream_idx {
            // Skip non-audio packets.
            ff_sys::av_packet_unref(pkt);
            continue 'read;
        }

        // Rescale timestamps to the output stream's time base and remap index.
        // SAFETY: pkt, in_tb, out_tb are valid plain-data values.
        ff_sys::av_packet_rescale_ts(pkt, in_tb, out_tb);
        (*pkt).stream_index = 0;

        // SAFETY: out_ctx and pkt are valid.
        let ret = ff_sys::av_interleaved_write_frame(out_ctx, pkt);
        // av_interleaved_write_frame takes the packet's buf reference; unref to clear.
        ff_sys::av_packet_unref(pkt);
        if ret < 0 {
            loop_err = Some(RemuxError::from_ffmpeg_error(ret));
            break 'read;
        }
    }

    // SAFETY: pkt was allocated by av_packet_alloc above and is still valid.
    let mut pkt_ptr = pkt;
    ff_sys::av_packet_free(&raw mut pkt_ptr);

    // ── Step 10: write trailer ────────────────────────────────────────────────
    // SAFETY: out_ctx is valid; write_header was called successfully.
    ff_sys::av_write_trailer(out_ctx);

    // ── Step 11: cleanup ──────────────────────────────────────────────────────
    // SAFETY: (*out_ctx).pb is non-null (opened above; still set after write_header).
    ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
    // SAFETY: out_ctx is non-null and was allocated by avformat_alloc_output_context2.
    ff_sys::avformat_free_context(out_ctx);
    // SAFETY: in_ctx is non-null (open_input succeeded).
    let mut p = in_ctx;
    ff_sys::avformat::close_input(&raw mut p);

    log::info!(
        "audio extracted output={} stream_index={audio_stream_idx}",
        output.display()
    );

    match loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ── Audio addition ────────────────────────────────────────────────────────────

/// Mux `audio_input` into `video_input`, writing both streams to `output`.
///
/// The video bitstream is stream-copied (no decode/encode cycle).  When
/// `loop_audio` is true and the audio is shorter than the video, the audio
/// track is looped by re-seeking to the start and advancing the PTS offset.
///
/// # Safety
///
/// All FFmpeg pointer invariants are maintained internally.  The public
/// `AudioAdder::run` wraps this function safely.
pub(crate) fn run_audio_addition(
    video_input: &Path,
    audio_input: &Path,
    output: &Path,
    loop_audio: bool,
) -> Result<(), RemuxError> {
    // SAFETY: All pointers are validated (null-checked) before use; resources
    //         are freed on every exit path.
    unsafe { run_audio_addition_unsafe(video_input, audio_input, output, loop_audio) }
}

unsafe fn run_audio_addition_unsafe(
    video_input: &Path,
    audio_input: &Path,
    output: &Path,
    loop_audio: bool,
) -> Result<(), RemuxError> {
    // ── Step 1: open video input ──────────────────────────────────────────────
    // SAFETY: video_input is a caller-supplied path; open_input returns Err on failure.
    let vid_ctx =
        ff_sys::avformat::open_input(video_input).map_err(RemuxError::from_ffmpeg_error)?;

    // ── Step 2: find stream info for video input ──────────────────────────────
    // SAFETY: vid_ctx is non-null (open_input succeeded).
    if let Err(e) = ff_sys::avformat::find_stream_info(vid_ctx) {
        let mut p = vid_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 3: locate the first video stream ─────────────────────────────────
    // SAFETY: nb_streams is the valid count; streams is a valid array of that length.
    let nb_vid_streams = (*vid_ctx).nb_streams as usize;
    let mut video_stream_idx: Option<usize> = None;
    for i in 0..nb_vid_streams {
        let stream = *(*vid_ctx).streams.add(i);
        if (*(*stream).codecpar).codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO {
            video_stream_idx = Some(i);
            break;
        }
    }
    let Some(video_stream_idx) = video_stream_idx else {
        let mut p = vid_ctx;
        ff_sys::avformat::close_input(&raw mut p);
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "no video stream found in video input path={}",
                video_input.display()
            ),
        });
    };

    // ── Step 4: open audio input ──────────────────────────────────────────────
    // SAFETY: audio_input is a caller-supplied path; open_input returns Err on failure.
    let aud_ctx = match ff_sys::avformat::open_input(audio_input) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut p = vid_ctx;
            ff_sys::avformat::close_input(&raw mut p);
            return Err(RemuxError::from_ffmpeg_error(e));
        }
    };

    // ── Step 5: find stream info for audio input ──────────────────────────────
    // SAFETY: aud_ctx is non-null (open_input succeeded).
    if let Err(e) = ff_sys::avformat::find_stream_info(aud_ctx) {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::from_ffmpeg_error(e));
    }

    // ── Step 6: locate the first audio stream ─────────────────────────────────
    // SAFETY: nb_streams is the valid count; streams is a valid array of that length.
    let nb_aud_streams = (*aud_ctx).nb_streams as usize;
    let mut audio_stream_idx: Option<usize> = None;
    for i in 0..nb_aud_streams {
        let stream = *(*aud_ctx).streams.add(i);
        if (*(*stream).codecpar).codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO {
            audio_stream_idx = Some(i);
            break;
        }
    }
    let Some(audio_stream_idx) = audio_stream_idx else {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "no audio stream found in audio input path={}",
                audio_input.display()
            ),
        });
    };

    // ── Step 7: decide whether to loop the audio ──────────────────────────────
    // Loop only when requested AND audio duration < video duration.
    // Durations are in AV_TIME_BASE (microseconds); a value ≤ 0 means unknown.
    let vid_duration_us = (*vid_ctx).duration;
    let aud_duration_us = (*aud_ctx).duration;
    let should_loop = loop_audio
        && vid_duration_us > 0
        && aud_duration_us > 0
        && aud_duration_us < vid_duration_us;

    // ── Step 8: allocate output context ──────────────────────────────────────
    let Some(output_str) = output.to_str() else {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path is not valid UTF-8".to_string(),
        });
    };
    let Ok(c_output) = std::ffi::CString::new(output_str) else {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "output path contains null bytes".to_string(),
        });
    };

    let mut out_ctx: *mut ff_sys::AVFormatContext = std::ptr::null_mut();
    // SAFETY: c_output is a valid null-terminated C string.
    let ret = ff_sys::avformat_alloc_output_context2(
        &raw mut out_ctx,
        std::ptr::null_mut(),
        std::ptr::null(),
        c_output.as_ptr(),
    );
    if ret < 0 || out_ctx.is_null() {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }

    // ── Step 9: copy video stream parameters ─────────────────────────────────
    // SAFETY: video_stream_idx < nb_vid_streams; streams is a valid array.
    let vid_in_stream = *(*vid_ctx).streams.add(video_stream_idx);
    // SAFETY: out_ctx is non-null (avformat_alloc_output_context2 succeeded).
    let vid_out_stream = ff_sys::avformat_new_stream(out_ctx, std::ptr::null());
    if vid_out_stream.is_null() {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed for video".to_string(),
        });
    }
    // SAFETY: both codecpar pointers are non-null (created by FFmpeg).
    let ret =
        ff_sys::avcodec_parameters_copy((*vid_out_stream).codecpar, (*vid_in_stream).codecpar);
    if ret < 0 {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }
    // Clear codec_tag so the muxer assigns the correct value for the container.
    (*(*vid_out_stream).codecpar).codec_tag = 0;

    // ── Step 10: copy audio stream parameters ────────────────────────────────
    // SAFETY: audio_stream_idx < nb_aud_streams; streams is a valid array.
    let aud_in_stream = *(*aud_ctx).streams.add(audio_stream_idx);
    // SAFETY: out_ctx is non-null (avformat_alloc_output_context2 succeeded).
    let aud_out_stream = ff_sys::avformat_new_stream(out_ctx, std::ptr::null());
    if aud_out_stream.is_null() {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed for audio".to_string(),
        });
    }
    // SAFETY: both codecpar pointers are non-null (created by FFmpeg).
    let ret =
        ff_sys::avcodec_parameters_copy((*aud_out_stream).codecpar, (*aud_in_stream).codecpar);
    if ret < 0 {
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        ff_sys::avformat_free_context(out_ctx);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }
    // Clear codec_tag so the muxer assigns the correct value for the container.
    (*(*aud_out_stream).codecpar).codec_tag = 0;

    // ── Step 11: open output file ─────────────────────────────────────────────
    // SAFETY: output is a valid path; WRITE opens the file for writing.
    let pb = match ff_sys::avformat::open_output(output, ff_sys::avformat::avio_flags::WRITE) {
        Ok(pb) => pb,
        Err(e) => {
            let mut pv = vid_ctx;
            ff_sys::avformat::close_input(&raw mut pv);
            let mut pa = aud_ctx;
            ff_sys::avformat::close_input(&raw mut pa);
            ff_sys::avformat_free_context(out_ctx);
            return Err(RemuxError::from_ffmpeg_error(e));
        }
    };
    // SAFETY: out_ctx is non-null; pb is a valid AVIOContext.
    (*out_ctx).pb = pb;

    // ── Step 12: write header ─────────────────────────────────────────────────
    // SAFETY: out_ctx is fully configured with streams and pb set.
    let ret = ff_sys::avformat_write_header(out_ctx, std::ptr::null_mut());
    if ret < 0 {
        // SAFETY: (*out_ctx).pb was set above and is non-null.
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::from_ffmpeg_error(ret));
    }

    // Read time bases after avformat_write_header — the muxer may adjust them.
    // SAFETY: stream pointers remain valid for the lifetime of their parent contexts.
    let vid_in_tb = (*vid_in_stream).time_base;
    let aud_in_tb = (*aud_in_stream).time_base;
    let vid_out_tb = (*vid_out_stream).time_base;
    let aud_out_tb = (*aud_out_stream).time_base;

    // Duration of the audio stream in its INPUT timebase — used to compute the
    // PTS offset when the audio is looped.  Fall back to 0 when unknown.
    let aud_loop_duration_in_tb: i64 = if (*aud_in_stream).duration > 0 {
        (*aud_in_stream).duration
    } else {
        0
    };

    log::debug!(
        "audio addition header written should_loop={should_loop} \
         video_stream_idx={video_stream_idx} audio_stream_idx={audio_stream_idx}"
    );

    // ── Step 13: allocate packet ──────────────────────────────────────────────
    // SAFETY: av_packet_alloc never returns null in practice (aborts on OOM).
    let pkt = ff_sys::av_packet_alloc();
    if pkt.is_null() {
        ff_sys::av_write_trailer(out_ctx);
        ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
        ff_sys::avformat_free_context(out_ctx);
        let mut pv = vid_ctx;
        ff_sys::avformat::close_input(&raw mut pv);
        let mut pa = aud_ctx;
        ff_sys::avformat::close_input(&raw mut pa);
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    }

    // ── Step 14: interleaved packet copy loop ─────────────────────────────────
    // Terminate when video is exhausted.  Audio terminates naturally (non-loop)
    // or is re-seeked with an advancing PTS offset (loop).
    let mut add_loop_err: Option<RemuxError> = None;
    let mut vid_eof = false;
    let mut aud_eof = false;
    // Cumulative PTS offset applied to looped audio packets (in audio IN timebase).
    let mut aud_pts_offset_in_tb: i64 = 0;

    'copy: loop {
        // ── video packet ──────────────────────────────────────────────────
        if !vid_eof {
            // SAFETY: vid_ctx and pkt are valid non-null pointers.
            match ff_sys::avformat::read_frame(vid_ctx, pkt) {
                Err(e) if e == ff_sys::error_codes::EOF => {
                    vid_eof = true;
                }
                Err(e) => {
                    add_loop_err = Some(RemuxError::from_ffmpeg_error(e));
                    break 'copy;
                }
                Ok(()) => {
                    if (*pkt).stream_index as usize == video_stream_idx {
                        // SAFETY: pkt, vid_in_tb, vid_out_tb are valid plain-data values.
                        ff_sys::av_packet_rescale_ts(pkt, vid_in_tb, vid_out_tb);
                        (*pkt).stream_index = 0;
                        // SAFETY: out_ctx and pkt are valid.
                        let ret = ff_sys::av_interleaved_write_frame(out_ctx, pkt);
                        ff_sys::av_packet_unref(pkt);
                        if ret < 0 {
                            add_loop_err = Some(RemuxError::from_ffmpeg_error(ret));
                            break 'copy;
                        }
                    } else {
                        ff_sys::av_packet_unref(pkt);
                    }
                }
            }
        }

        // Stop as soon as video is done — no point reading more audio.
        if vid_eof {
            break 'copy;
        }

        // ── audio packet ──────────────────────────────────────────────────
        if !aud_eof {
            // SAFETY: aud_ctx and pkt are valid non-null pointers.
            match ff_sys::avformat::read_frame(aud_ctx, pkt) {
                Err(e) if e == ff_sys::error_codes::EOF => {
                    if should_loop {
                        // Re-seek audio to the start and advance the PTS offset
                        // so that looped packets continue from where the last
                        // packet ended.
                        // SAFETY: aud_ctx is non-null; seeking to timestamp 0.
                        let _ = ff_sys::avformat::seek_frame(
                            aud_ctx,
                            audio_stream_idx as i32,
                            0,
                            ff_sys::avformat::seek_flags::BACKWARD,
                        );
                        aud_pts_offset_in_tb += aud_loop_duration_in_tb;
                        // pkt was not filled on EOF; nothing to unref.
                    } else {
                        aud_eof = true;
                    }
                }
                Err(e) => {
                    add_loop_err = Some(RemuxError::from_ffmpeg_error(e));
                    break 'copy;
                }
                Ok(()) => {
                    if (*pkt).stream_index as usize == audio_stream_idx {
                        // Apply the cumulative loop offset before rescaling so
                        // that PTS values are monotonically increasing across loops.
                        if (*pkt).pts != ff_sys::AV_NOPTS_VALUE {
                            (*pkt).pts += aud_pts_offset_in_tb;
                        }
                        if (*pkt).dts != ff_sys::AV_NOPTS_VALUE {
                            (*pkt).dts += aud_pts_offset_in_tb;
                        }
                        // SAFETY: pkt, aud_in_tb, aud_out_tb are valid plain-data values.
                        ff_sys::av_packet_rescale_ts(pkt, aud_in_tb, aud_out_tb);
                        (*pkt).stream_index = 1;
                        // SAFETY: out_ctx and pkt are valid.
                        let ret = ff_sys::av_interleaved_write_frame(out_ctx, pkt);
                        ff_sys::av_packet_unref(pkt);
                        if ret < 0 {
                            add_loop_err = Some(RemuxError::from_ffmpeg_error(ret));
                            break 'copy;
                        }
                    } else {
                        ff_sys::av_packet_unref(pkt);
                    }
                }
            }
        }
    }

    // SAFETY: pkt was allocated by av_packet_alloc above and is still valid.
    let mut pkt_ptr = pkt;
    ff_sys::av_packet_free(&raw mut pkt_ptr);

    // ── Step 15: write trailer ────────────────────────────────────────────────
    // SAFETY: out_ctx is valid; write_header was called successfully.
    ff_sys::av_write_trailer(out_ctx);

    // ── Step 16: cleanup ──────────────────────────────────────────────────────
    // SAFETY: (*out_ctx).pb is non-null (opened above; still set after write_header).
    ff_sys::avformat::close_output(std::ptr::addr_of_mut!((*out_ctx).pb));
    // SAFETY: out_ctx is non-null and was allocated by avformat_alloc_output_context2.
    ff_sys::avformat_free_context(out_ctx);
    // SAFETY: vid_ctx and aud_ctx are non-null (open_input succeeded).
    let mut pv = vid_ctx;
    ff_sys::avformat::close_input(&raw mut pv);
    let mut pa = aud_ctx;
    ff_sys::avformat::close_input(&raw mut pa);

    log::info!(
        "audio added output={} loop_audio={loop_audio}",
        output.display()
    );

    match add_loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
