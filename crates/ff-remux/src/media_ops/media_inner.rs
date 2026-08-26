//! Audio stream operations (replacement, extraction, addition) via ff-sys safe accessors.

// FFmpeg-boundary lints: intentional narrowing/sign casts at the C ABI and
// acronym-heavy FFmpeg doc terms concentrate in this module.
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::doc_markdown)]

use std::path::Path;

use crate::error::RemuxError;

/// Replace the audio stream of `video_input` with the audio from `audio_input`,
/// writing the combined result to `output`.
///
/// The bitstream is stream-copied (no decode/encode cycle); all FFmpeg access
/// goes through owned ff-sys types, so every context frees itself on drop.
pub(crate) fn run_audio_replacement(
    video_input: &Path,
    audio_input: &Path,
    output: &Path,
) -> Result<(), RemuxError> {
    // ── Step 1: open video input (owned) ──────────────────────────────────────
    // All contexts are owned; every early return drops them (closing IO /
    // freeing) with no manual teardown on any path.
    let mut vid_ctx = ff_sys::InputFormatContext::open(video_input)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 2: find stream info for video input ──────────────────────────────
    vid_ctx
        .find_stream_info()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 3: locate the first video stream ─────────────────────────────────
    let Some(video_stream_idx) = vid_ctx
        .streams()
        .find(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO)
        .map(|s| s.index() as usize)
    else {
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "no video stream found in video input path={}",
                video_input.display()
            ),
        });
    };

    // ── Step 4: open audio input (owned) ──────────────────────────────────────
    let mut aud_ctx = ff_sys::InputFormatContext::open(audio_input)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 5: find stream info for audio input ──────────────────────────────
    aud_ctx
        .find_stream_info()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 6: locate the first audio stream ─────────────────────────────────
    let Some(audio_stream_idx) = aud_ctx
        .streams()
        .find(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO)
        .map(|s| s.index() as usize)
    else {
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "no audio stream found in audio input path={}",
                audio_input.display()
            ),
        });
    };

    // ── Step 7: allocate output context (owned) ──────────────────────────────
    let mut out_ctx = ff_sys::OutputFormatContext::new(None, output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 8: copy video stream parameters to output ────────────────────────
    // copy_stream_params deep-copies the parameters and clears codec_tag so the
    // muxer assigns the correct value for the container. The input time base is
    // stable across write_header, so capture it here.
    let vid_out_idx = out_ctx.new_stream(None).map_err(|_| RemuxError::Ffmpeg {
        code: 0,
        message: "avformat_new_stream failed for video".to_string(),
    })?;
    let vid_in_tb = {
        let vid_in =
            vid_ctx
                .stream(video_stream_idx)
                .ok_or_else(|| RemuxError::OperationFailed {
                    reason: "video input stream missing".to_string(),
                })?;
        out_ctx
            .copy_stream_params(vid_out_idx, vid_in.codecpar())
            .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
        vid_in.time_base()
    };

    // ── Step 9: copy audio stream parameters to output ────────────────────────
    let aud_out_idx = out_ctx.new_stream(None).map_err(|_| RemuxError::Ffmpeg {
        code: 0,
        message: "avformat_new_stream failed for audio".to_string(),
    })?;
    let aud_in_tb = {
        let aud_in =
            aud_ctx
                .stream(audio_stream_idx)
                .ok_or_else(|| RemuxError::OperationFailed {
                    reason: "audio input stream missing".to_string(),
                })?;
        out_ctx
            .copy_stream_params(aud_out_idx, aud_in.codecpar())
            .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
        aud_in.time_base()
    };

    // ── Step 10: open output file ─────────────────────────────────────────────
    out_ctx
        .open_io(output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 11: write header ─────────────────────────────────────────────────
    out_ctx
        .write_header()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // Read output time bases after avformat_write_header — the muxer may adjust
    // them; the input time bases were captured before the header write.
    let vid_out_tb = out_ctx.stream_time_base(vid_out_idx);
    let aud_out_tb = out_ctx.stream_time_base(aud_out_idx);

    log::debug!(
        "audio replacement header written \
         video_stream_idx={video_stream_idx} audio_stream_idx={audio_stream_idx}"
    );

    // ── Step 12: allocate packet ──────────────────────────────────────────────
    // The owned packet frees itself exactly once on drop at scope end.
    let Ok(mut pkt) = ff_sys::Packet::new() else {
        let _ = out_ctx.write_trailer();
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    };

    // ── Step 13: interleaved packet copy loop ─────────────────────────────────
    // Alternate between video and audio inputs; use av_interleaved_write_frame
    // so the muxer buffers and flushes packets in the correct timestamp order.
    let mut loop_err: Option<RemuxError> = None;
    let mut vid_eof = false;
    let mut aud_eof = false;

    'copy: loop {
        // Read one packet from the video input, forwarding only the target stream.
        if !vid_eof {
            match vid_ctx.read_frame(&mut pkt) {
                Err(e) if e.is_eof() => {
                    vid_eof = true;
                }
                Err(e) => {
                    loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                    break 'copy;
                }
                Ok(()) => {
                    if pkt.stream_index() as usize == video_stream_idx {
                        pkt.rescale_ts(vid_in_tb, vid_out_tb);
                        pkt.set_stream_index(0);
                        let write_res = out_ctx.write_interleaved(&mut pkt);
                        // av_interleaved_write_frame takes the packet's buf reference;
                        // unref to clear any remaining fields.
                        pkt.unref();
                        if let Err(e) = write_res {
                            loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                            break 'copy;
                        }
                    } else {
                        pkt.unref();
                    }
                }
            }
        }

        // Read one packet from the audio input, forwarding only the target stream.
        if !aud_eof {
            match aud_ctx.read_frame(&mut pkt) {
                Err(e) if e.is_eof() => {
                    aud_eof = true;
                }
                Err(e) => {
                    loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                    break 'copy;
                }
                Ok(()) => {
                    if pkt.stream_index() as usize == audio_stream_idx {
                        pkt.rescale_ts(aud_in_tb, aud_out_tb);
                        pkt.set_stream_index(1);
                        let write_res = out_ctx.write_interleaved(&mut pkt);
                        pkt.unref();
                        if let Err(e) = write_res {
                            loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                            break 'copy;
                        }
                    } else {
                        pkt.unref();
                    }
                }
            }
        }

        if vid_eof && aud_eof {
            break 'copy;
        }
    }

    // ── Step 14: write trailer ────────────────────────────────────────────────
    let _ = out_ctx.write_trailer();

    // The owned `vid_ctx` / `aud_ctx` / `out_ctx` close their IO and free
    // themselves when they drop at scope end; no manual teardown is needed.
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
pub(crate) fn run_audio_extraction(
    input: &Path,
    output: &Path,
    requested_idx: Option<usize>,
) -> Result<(), RemuxError> {
    // ── Step 1: open input (owned) ────────────────────────────────────────────
    // The input and output contexts are owned; every early return drops them.
    let mut in_ctx = ff_sys::InputFormatContext::open(input)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 2: find stream info ──────────────────────────────────────────────
    in_ctx
        .find_stream_info()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 3: locate the audio stream ──────────────────────────────────────
    let nb_streams = in_ctx.nb_streams() as usize;
    let audio_stream_idx = if let Some(idx) = requested_idx {
        // Validate that the requested index is actually an audio stream.
        if idx >= nb_streams {
            return Err(RemuxError::OperationFailed {
                reason: format!("stream index {idx} out of range (input has {nb_streams} streams)"),
            });
        }
        let is_audio = in_ctx
            .stream(idx)
            .is_some_and(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO);
        if !is_audio {
            return Err(RemuxError::OperationFailed {
                reason: format!("stream index {idx} is not an audio stream"),
            });
        }
        idx
    } else {
        // Find the first audio stream.
        let Some(idx) = in_ctx
            .streams()
            .find(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO)
            .map(|s| s.index() as usize)
        else {
            return Err(RemuxError::OperationFailed {
                reason: format!("no audio stream found in input path={}", input.display()),
            });
        };
        idx
    };

    // ── Step 4: allocate output context (owned) ──────────────────────────────
    let mut out_ctx = ff_sys::OutputFormatContext::new(None, output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 5: create output stream and copy parameters ─────────────────────
    // copy_stream_params deep-copies the parameters and clears codec_tag so the
    // muxer assigns the correct value for the container. The input time base is
    // stable across write_header, so capture it here.
    let out_idx = out_ctx.new_stream(None).map_err(|_| RemuxError::Ffmpeg {
        code: 0,
        message: "avformat_new_stream failed".to_string(),
    })?;
    let in_tb = {
        let in_stream =
            in_ctx
                .stream(audio_stream_idx)
                .ok_or_else(|| RemuxError::OperationFailed {
                    reason: "audio input stream missing".to_string(),
                })?;
        out_ctx
            .copy_stream_params(out_idx, in_stream.codecpar())
            .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
        in_stream.time_base()
    };

    // ── Step 6: open output file ──────────────────────────────────────────────
    out_ctx
        .open_io(output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 7: write header ──────────────────────────────────────────────────
    // A non-zero return here usually means the codec is incompatible with the
    // chosen output container. Wrap it with a clear message so callers know what
    // went wrong.
    out_ctx
        .write_header()
        .map_err(|e| RemuxError::OperationFailed {
            reason: format!(
                "codec incompatible with output container: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

    // Read the output time base after avformat_write_header — the muxer may
    // adjust it; the input time base was captured before the header write.
    let out_tb = out_ctx.stream_time_base(out_idx);

    log::debug!(
        "audio extraction header written audio_stream_idx={audio_stream_idx} \
         output={}",
        output.display()
    );

    // ── Step 8: allocate packet ───────────────────────────────────────────────
    // The owned packet frees itself exactly once on drop at scope end.
    let Ok(mut pkt) = ff_sys::Packet::new() else {
        let _ = out_ctx.write_trailer();
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    };

    // ── Step 9: packet copy loop (audio stream only) ──────────────────────────
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

        if pkt.stream_index() as usize != audio_stream_idx {
            // Skip non-audio packets.
            pkt.unref();
            continue 'read;
        }

        // Rescale timestamps to the output stream's time base and remap index.
        pkt.rescale_ts(in_tb, out_tb);
        pkt.set_stream_index(0);

        let write_res = out_ctx.write_interleaved(&mut pkt);
        // av_interleaved_write_frame takes the packet's buf reference; unref to clear.
        pkt.unref();
        if let Err(e) = write_res {
            loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
            break 'read;
        }
    }

    // ── Step 10: write trailer ────────────────────────────────────────────────
    let _ = out_ctx.write_trailer();

    // The owned `in_ctx` / `out_ctx` close their IO and free themselves when they
    // drop at scope end; no manual teardown is needed.
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
pub(crate) fn run_audio_addition(
    video_input: &Path,
    audio_input: &Path,
    output: &Path,
    loop_audio: bool,
) -> Result<(), RemuxError> {
    // ── Step 1: open video input (owned) ──────────────────────────────────────
    // All contexts are owned; every early return drops them.
    let mut vid_ctx = ff_sys::InputFormatContext::open(video_input)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 2: find stream info for video input ──────────────────────────────
    vid_ctx
        .find_stream_info()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 3: locate the first video stream ─────────────────────────────────
    let Some(video_stream_idx) = vid_ctx
        .streams()
        .find(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO)
        .map(|s| s.index() as usize)
    else {
        return Err(RemuxError::OperationFailed {
            reason: format!(
                "no video stream found in video input path={}",
                video_input.display()
            ),
        });
    };

    // ── Step 4: open audio input (owned) ──────────────────────────────────────
    let mut aud_ctx = ff_sys::InputFormatContext::open(audio_input)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 5: find stream info for audio input ──────────────────────────────
    aud_ctx
        .find_stream_info()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 6: locate the first audio stream ─────────────────────────────────
    let Some(audio_stream_idx) = aud_ctx
        .streams()
        .find(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO)
        .map(|s| s.index() as usize)
    else {
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
    let vid_duration_us = vid_ctx.duration();
    let aud_duration_us = aud_ctx.duration();
    let should_loop = loop_audio
        && vid_duration_us > 0
        && aud_duration_us > 0
        && aud_duration_us < vid_duration_us;

    // ── Step 8: allocate output context (owned) ──────────────────────────────
    let mut out_ctx = ff_sys::OutputFormatContext::new(None, output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 9: copy video stream parameters ─────────────────────────────────
    // copy_stream_params deep-copies the parameters and clears codec_tag so the
    // muxer assigns the correct value for the container. The input time base is
    // stable across write_header, so capture it here.
    let vid_out_idx = out_ctx.new_stream(None).map_err(|_| RemuxError::Ffmpeg {
        code: 0,
        message: "avformat_new_stream failed for video".to_string(),
    })?;
    let vid_in_tb = {
        let vid_in =
            vid_ctx
                .stream(video_stream_idx)
                .ok_or_else(|| RemuxError::OperationFailed {
                    reason: "video input stream missing".to_string(),
                })?;
        out_ctx
            .copy_stream_params(vid_out_idx, vid_in.codecpar())
            .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
        vid_in.time_base()
    };

    // ── Step 10: copy audio stream parameters ────────────────────────────────
    let aud_out_idx = out_ctx.new_stream(None).map_err(|_| RemuxError::Ffmpeg {
        code: 0,
        message: "avformat_new_stream failed for audio".to_string(),
    })?;
    let aud_in_tb = {
        let aud_in =
            aud_ctx
                .stream(audio_stream_idx)
                .ok_or_else(|| RemuxError::OperationFailed {
                    reason: "audio input stream missing".to_string(),
                })?;
        out_ctx
            .copy_stream_params(aud_out_idx, aud_in.codecpar())
            .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;
        aud_in.time_base()
    };

    // ── Step 11: open output file ─────────────────────────────────────────────
    out_ctx
        .open_io(output)
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // ── Step 12: write header ─────────────────────────────────────────────────
    out_ctx
        .write_header()
        .map_err(|e| RemuxError::from_ffmpeg_error(e.code()))?;

    // Read output time bases after avformat_write_header — the muxer may adjust
    // them; the input time bases were captured before the header write.
    let vid_out_tb = out_ctx.stream_time_base(vid_out_idx);
    let aud_out_tb = out_ctx.stream_time_base(aud_out_idx);

    // Duration of the audio stream in its INPUT timebase — used to compute the
    // PTS offset when the audio is looped.  Fall back to 0 when unknown.
    let aud_loop_duration_in_tb: i64 = aud_ctx
        .stream(audio_stream_idx)
        .map_or(0, |s| s.duration().max(0));

    log::debug!(
        "audio addition header written should_loop={should_loop} \
         video_stream_idx={video_stream_idx} audio_stream_idx={audio_stream_idx}"
    );

    // ── Step 13: allocate packet ──────────────────────────────────────────────
    // The owned packet frees itself exactly once on drop at scope end.
    let Ok(mut pkt) = ff_sys::Packet::new() else {
        let _ = out_ctx.write_trailer();
        return Err(RemuxError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    };

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
            match vid_ctx.read_frame(&mut pkt) {
                Err(e) if e.is_eof() => {
                    vid_eof = true;
                }
                Err(e) => {
                    add_loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                    break 'copy;
                }
                Ok(()) => {
                    if pkt.stream_index() as usize == video_stream_idx {
                        pkt.rescale_ts(vid_in_tb, vid_out_tb);
                        pkt.set_stream_index(0);
                        let write_res = out_ctx.write_interleaved(&mut pkt);
                        pkt.unref();
                        if let Err(e) = write_res {
                            add_loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                            break 'copy;
                        }
                    } else {
                        pkt.unref();
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
            match aud_ctx.read_frame(&mut pkt) {
                Err(e) if e.is_eof() => {
                    if should_loop {
                        // Re-seek audio to the start and advance the PTS offset
                        // so that looped packets continue from where the last
                        // packet ended.
                        let _ = aud_ctx.seek_frame(
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
                    add_loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                    break 'copy;
                }
                Ok(()) => {
                    if pkt.stream_index() as usize == audio_stream_idx {
                        // Apply the cumulative loop offset before rescaling so
                        // that PTS values are monotonically increasing across loops.
                        if pkt.pts() != ff_sys::AV_NOPTS_VALUE {
                            pkt.set_pts(pkt.pts() + aud_pts_offset_in_tb);
                        }
                        if pkt.dts() != ff_sys::AV_NOPTS_VALUE {
                            pkt.set_dts(pkt.dts() + aud_pts_offset_in_tb);
                        }
                        pkt.rescale_ts(aud_in_tb, aud_out_tb);
                        pkt.set_stream_index(1);
                        let write_res = out_ctx.write_interleaved(&mut pkt);
                        pkt.unref();
                        if let Err(e) = write_res {
                            add_loop_err = Some(RemuxError::from_ffmpeg_error(e.code()));
                            break 'copy;
                        }
                    } else {
                        pkt.unref();
                    }
                }
            }
        }
    }

    // ── Step 15: write trailer ────────────────────────────────────────────────
    let _ = out_ctx.write_trailer();

    // The owned `vid_ctx` / `aud_ctx` / `out_ctx` close their IO and free
    // themselves when they drop at scope end; no manual teardown is needed.
    log::info!(
        "audio added output={} loop_audio={loop_audio}",
        output.display()
    );

    match add_loop_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
