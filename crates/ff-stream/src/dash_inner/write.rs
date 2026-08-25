//! Packet writing loop and segment management for single-rendition and ABR DASH output.

use std::ffi::CString;
use std::ptr;

use ff_sys::{
    AVPictureType_AV_PICTURE_TYPE_I, AVPictureType_AV_PICTURE_TYPE_NONE,
    AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational, ReceiveOutcome, av_opt_set, av_rescale_q,
    avformat_new_stream,
};

use crate::error::StreamError;

use super::context::RenditionState;
use super::streams::{detect_fps, open_aac_encoder, select_h264_encoder};
use super::{ffmpeg_err, ffmpeg_err_msg};

// ============================================================================
// Single-rendition DASH write loop
// ============================================================================

pub(super) unsafe fn write_dash_unsafe(
    input_path: &str,
    output_dir: &str,
    segment_duration_secs: f64,
) -> Result<(), StreamError> {
    ff_sys::ensure_initialized();

    // ── 1. Open input ─────────────────────────────────────────────────────────
    // The owned demux context frees itself (closing the input) on every early
    // return below, so no manual teardown is needed on any path.
    let mut input_ctx = ff_sys::InputFormatContext::open(std::path::Path::new(input_path))
        .map_err(|e| ffmpeg_err(e.code()))?;

    input_ctx
        .find_stream_info()
        .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 2. Locate video and audio streams ─────────────────────────────────────
    let nb_streams = input_ctx.nb_streams() as usize;
    let mut video_stream_idx: i32 = -1;
    let mut audio_stream_idx: i32 = -1;

    for i in 0..nb_streams {
        let stream = *(*input_ctx.as_ptr()).streams.add(i);
        let codec_type = (*(*stream).codecpar).codec_type;
        if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO && video_stream_idx < 0 {
            video_stream_idx = i as i32;
        } else if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO && audio_stream_idx < 0 {
            audio_stream_idx = i as i32;
        }
    }

    if video_stream_idx < 0 {
        return Err(StreamError::InvalidConfig {
            reason: "input file contains no video stream".into(),
        });
    }

    // ── 3. Read video stream properties ──────────────────────────────────────
    let video_stream = *(*input_ctx.as_ptr()).streams.add(video_stream_idx as usize);
    let video_codecpar = (*video_stream).codecpar;
    let enc_width = (*video_codecpar).width;
    let enc_height = (*video_codecpar).height;
    let video_fps = detect_fps(video_stream, input_ctx.as_mut_ptr());
    let fps_int = video_fps.round().max(1.0) as i32;

    // Compute keyframe interval from segment duration and fps
    let keyframe_interval = (segment_duration_secs * fps_int as f64).round().max(1.0) as u32;

    // ── 4. Open input video decoder ────────────────────────────────────────────
    let vid_codec_id = (*video_codecpar).codec_id;
    let vid_decoder = ff_sys::Codec::find_decoder(vid_codec_id)
        .ok_or_else(|| ffmpeg_err_msg("no video decoder available for input stream"))?;

    let mut vid_dec_ctx =
        ff_sys::CodecContext::new(Some(vid_decoder)).map_err(|e| ffmpeg_err(e.code()))?;

    vid_dec_ctx
        .parameters_to_context(video_codecpar)
        .map_err(|e| ffmpeg_err(e.code()))?;

    vid_dec_ctx
        .open(vid_decoder, ptr::null_mut())
        .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 5. Open input audio decoder (optional) ────────────────────────────────
    let mut aud_dec_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_sample_rate: i32 = 44100;
    let mut aud_nb_channels: i32 = 2;

    if audio_stream_idx >= 0 {
        let audio_stream = *(*input_ctx.as_ptr()).streams.add(audio_stream_idx as usize);
        let audio_codecpar = (*audio_stream).codecpar;
        let aud_codec_id = (*audio_codecpar).codec_id;

        if let Some(aud_decoder) = ff_sys::Codec::find_decoder(aud_codec_id) {
            if let Ok(mut ctx) = ff_sys::CodecContext::new(Some(aud_decoder)) {
                if ctx.parameters_to_context(audio_codecpar).is_ok()
                    && ctx.open(aud_decoder, ptr::null_mut()).is_ok()
                {
                    aud_sample_rate = (*ctx.as_ptr()).sample_rate;
                    aud_nb_channels = (*ctx.as_ptr()).ch_layout.nb_channels;
                    aud_dec_ctx = Some(ctx);
                    log::info!(
                        "dash audio decoder opened sample_rate={aud_sample_rate} \
                         channels={aud_nb_channels}"
                    );
                } else {
                    // `ctx` drops here (frees the decoder context).
                    audio_stream_idx = -1;
                    log::warn!("dash audio decoder open failed, skipping audio");
                }
            } else {
                audio_stream_idx = -1;
                log::warn!("dash audio decoder alloc failed, skipping audio");
            }
        } else {
            audio_stream_idx = -1;
            log::warn!("dash no audio decoder found, skipping audio");
        }
    }

    // ── 6. Allocate DASH output context ───────────────────────────────────────
    // Both `input_ctx` and `out_ctx` are owned: every early return below drops
    // them (closing the input / the output's `pb` and freeing) with no manual
    // teardown on any path.
    let manifest_path = format!("{output_dir}/manifest.mpd");

    let mut out_ctx =
        ff_sys::OutputFormatContext::new(Some("dash"), std::path::Path::new(&manifest_path))
            .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 7. Set DASH muxer options ──────────────────────────────────────────────
    let seg_duration_str = format!("{}", segment_duration_secs as u32);
    if let Ok(c_seg_dur) = CString::new(seg_duration_str.as_str()) {
        let ret = av_opt_set(
            (*out_ctx.as_mut_ptr()).priv_data,
            c"seg_duration".as_ptr(),
            c_seg_dur.as_ptr(),
            0,
        );
        if ret < 0 {
            log::warn!(
                "dash seg_duration option not supported, using default \
                 requested={seg_duration_str} error={}",
                ff_sys::av_error_string(ret)
            );
        }
    }

    // ── 8. Open H.264 video encoder ───────────────────────────────────────────
    let vid_enc_codec = select_h264_encoder().ok_or_else(|| {
        ffmpeg_err_msg("no H.264 encoder available (tried h264_nvenc, h264_qsv, h264_amf, h264_videotoolbox, libx264, mpeg4)")
    })?;

    let mut vid_enc_ctx =
        ff_sys::CodecContext::new(Some(vid_enc_codec)).map_err(|e| ffmpeg_err(e.code()))?;
    let venc = vid_enc_ctx.as_mut_ptr();

    (*venc).width = enc_width;
    (*venc).height = enc_height;
    (*venc).time_base.num = 1;
    (*venc).time_base.den = fps_int;
    (*venc).framerate.num = fps_int;
    (*venc).framerate.den = 1;
    (*venc).pix_fmt = AVPixelFormat_AV_PIX_FMT_YUV420P;
    (*venc).bit_rate = 2_000_000;

    // On error the owned `vid_enc_ctx` / decoders / `input_ctx` / `out_ctx` all
    // drop; no manual teardown is needed.
    vid_enc_ctx
        .open(vid_enc_codec, ptr::null_mut())
        .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 9. Add video output stream ────────────────────────────────────────────
    let vid_out_stream = avformat_new_stream(out_ctx.as_mut_ptr(), vid_enc_codec.as_ptr());
    if vid_out_stream.is_null() {
        return Err(ffmpeg_err_msg("cannot create video output stream"));
    }
    (*vid_out_stream).time_base = (*vid_enc_ctx.as_ptr()).time_base;
    let vid_out_stream_idx = (out_ctx.nb_streams() - 1) as i32;

    // SAFETY: vid_out_stream and vid_enc_ctx are valid; avcodec_open2 has been called.
    vid_enc_ctx
        .parameters_from_context((*vid_out_stream).codecpar)
        .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 10. Open AAC audio encoder and add audio stream (optional) ────────────
    let mut aud_enc_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_out_stream_idx: i32 = -1;
    let mut swr_ctx: Option<ff_sys::ResampleContext> = None;

    if audio_stream_idx >= 0 {
        match open_aac_encoder(aud_sample_rate, aud_nb_channels) {
            Ok(ctx) => {
                let aud_out_stream = avformat_new_stream(out_ctx.as_mut_ptr(), ptr::null());
                if aud_out_stream.is_null() {
                    // `ctx` drops here (frees the codec context).
                    log::warn!("dash cannot create audio output stream, skipping audio");
                    audio_stream_idx = -1;
                } else {
                    (*aud_out_stream).time_base.num = 1;
                    (*aud_out_stream).time_base.den = aud_sample_rate;
                    aud_out_stream_idx = (out_ctx.nb_streams() - 1) as i32;

                    // SAFETY: aud_out_stream and ctx are valid; avcodec_open2 called.
                    if ctx
                        .parameters_from_context((*aud_out_stream).codecpar)
                        .is_err()
                    {
                        log::warn!("dash audio stream codecpar copy failed");
                    }

                    // Set up resampler: decoded audio → FLTP at aud_sample_rate.
                    if let Some(dec) = aud_dec_ctx.as_ref() {
                        let enc_ptr = ctx.as_ptr();
                        let dec_ptr = dec.as_ptr();
                        if let Ok(swr) = ff_sys::ResampleContext::new(
                            &(*enc_ptr).ch_layout,
                            (*enc_ptr).sample_fmt,
                            (*enc_ptr).sample_rate,
                            &(*dec_ptr).ch_layout,
                            (*dec_ptr).sample_fmt,
                            (*dec_ptr).sample_rate,
                        ) {
                            swr_ctx = Some(swr);
                            aud_enc_ctx = Some(ctx);
                        } else {
                            // `ctx` drops here.
                            log::warn!("dash swr alloc failed, skipping audio");
                            audio_stream_idx = -1;
                        }
                    } else {
                        log::warn!("dash audio decoder missing, skipping audio");
                        audio_stream_idx = -1;
                    }
                }
            }
            Err(e) => {
                log::warn!("dash aac encoder unavailable: {e}, skipping audio");
                audio_stream_idx = -1;
            }
        }
    }

    // ── 11. Open output file and write header ─────────────────────────────────
    if let Err(e) = out_ctx.open_io(std::path::Path::new(&manifest_path)) {
        return Err(ffmpeg_err(e.code()));
    }

    if let Err(e) = out_ctx.write_header() {
        // The owned `out_ctx` closes `pb` and frees on drop at this return.
        return Err(ffmpeg_err(e.code()));
    }

    // Close pb now so the DASH muxer can manage its own avio handles for segment
    // files without hitting a locked-file error on Windows. `close_io` nulls `pb`,
    // so the later drop only frees the context.
    out_ctx.close_io();

    log::info!(
        "dash output context ready width={enc_width} height={enc_height} fps={video_fps:.1} \
         audio={}",
        audio_stream_idx >= 0
    );

    // ── 12. Allocate frame and packet buffers ──────────────────────────────────
    // Owned frames/packet: each frees exactly once on drop. On the error arm any
    // successfully-allocated locals drop at the end of this statement.
    let (
        Ok(mut pkt),
        Ok(mut vid_dec_frame),
        Ok(mut vid_enc_frame),
        Ok(mut aud_dec_frame),
        Ok(mut aud_enc_frame),
    ) = (
        ff_sys::Packet::new(),
        ff_sys::Frame::new(),
        ff_sys::Frame::new(),
        ff_sys::Frame::new(),
        ff_sys::Frame::new(),
    )
    else {
        let _ = out_ctx.write_trailer();
        // `out_ctx` frees on drop at this return (pb already closed via close_io).
        return Err(ffmpeg_err_msg("cannot allocate frame or packet"));
    };

    // ── 13. Decode–encode loop ─────────────────────────────────────────────────
    let mut video_frame_count: u64 = 0;
    let mut audio_sample_count: i64 = 0;
    let mut sws_ctx: Option<ff_sys::ScaleContext> = None;
    let mut last_src_fmt: Option<ff_sys::AVPixelFormat> = None;
    let mut last_src_w: Option<i32> = None;
    let mut last_src_h: Option<i32> = None;

    // Frame period rationals passed to drain_encoder (immune to lazy enc time_base mutations).
    let vid_frame_period = AVRational {
        num: 1,
        den: fps_int,
    };
    let aud_frame_period = if let Some(aud) = aud_enc_ctx.as_ref() {
        AVRational {
            num: aud.frame_size(),
            den: aud.sample_rate(),
        }
    } else {
        AVRational { num: 1, den: 48000 } // unused; aud_enc_ctx is None
    };

    loop {
        match input_ctx.read_frame(&mut pkt) {
            Err(e) if e.is_eof() => break,
            Err(_e) => {
                // Non-EOF read errors: continue to try next packet
                pkt.unref();
                continue;
            }
            Ok(()) => {}
        }

        let stream_idx = pkt.stream_index();

        if stream_idx == video_stream_idx {
            // ── Video path ────────────────────────────────────────────────────
            if vid_dec_ctx.send_packet(&pkt).is_err() {
                pkt.unref();
                continue;
            }
            pkt.unref();

            while let Ok(ReceiveOutcome::Frame) = vid_dec_ctx.receive_frame(&mut vid_dec_frame) {
                // Force keyframe at intervals
                vid_dec_frame.set_pict_type(
                    if video_frame_count.is_multiple_of(u64::from(keyframe_interval)) {
                        AVPictureType_AV_PICTURE_TYPE_I
                    } else {
                        AVPictureType_AV_PICTURE_TYPE_NONE
                    },
                );

                // Convert decoded frame to YUV420P at encoder dimensions
                let src_fmt = vid_dec_frame.format();
                let src_w = vid_dec_frame.width();
                let src_h = vid_dec_frame.height();

                // Recreate SwsContext when source properties change
                if last_src_fmt != Some(src_fmt)
                    || last_src_w != Some(src_w)
                    || last_src_h != Some(src_h)
                {
                    // Move-assign the new context; the old one (if any) drops here.
                    if let Ok(ctx) = ff_sys::ScaleContext::new(
                        src_w,
                        src_h,
                        src_fmt,
                        enc_width,
                        enc_height,
                        AVPixelFormat_AV_PIX_FMT_YUV420P,
                        ff_sys::swscale::scale_flags::BILINEAR,
                    ) {
                        sws_ctx = Some(ctx);
                        last_src_fmt = Some(src_fmt);
                        last_src_w = Some(src_w);
                        last_src_h = Some(src_h);
                    } else {
                        vid_dec_frame.unref();
                        continue;
                    }
                }

                // Prepare encoder frame
                vid_enc_frame.set_format(AVPixelFormat_AV_PIX_FMT_YUV420P);
                vid_enc_frame.set_width(enc_width);
                vid_enc_frame.set_height(enc_height);
                // SAFETY: av_rescale_q is safe for valid AVRational values.
                vid_enc_frame.set_pts(av_rescale_q(
                    video_frame_count as i64,
                    AVRational {
                        num: 1,
                        den: fps_int,
                    },
                    vid_enc_ctx.time_base(),
                ));

                if vid_enc_frame.get_buffer(0).is_err() {
                    vid_dec_frame.unref();
                    continue;
                }

                // Scale decoded frame into encoder frame
                let scaled = if let Some(sws) = sws_ctx.as_mut() {
                    sws.scale_frames(&vid_dec_frame, &mut vid_enc_frame).is_ok()
                } else {
                    false
                };

                if scaled && vid_enc_ctx.send_frame(vid_enc_frame.as_ptr()).is_ok() {
                    crate::codec_utils::drain_encoder(
                        vid_enc_ctx.as_mut_ptr(),
                        out_ctx.as_mut_ptr(),
                        vid_out_stream_idx,
                        "dash",
                        vid_frame_period,
                    );
                }

                vid_enc_frame.unref();
                vid_dec_frame.unref();
                video_frame_count += 1;
            }
        } else if stream_idx == audio_stream_idx
            && let Some(aud_dec) = aud_dec_ctx.as_mut()
            && let Some(aud_enc) = aud_enc_ctx.as_mut()
        {
            // ── Audio path ────────────────────────────────────────────────────
            if aud_dec.send_packet(&pkt).is_err() {
                pkt.unref();
                continue;
            }
            pkt.unref();

            while let Ok(ReceiveOutcome::Frame) = aud_dec.receive_frame(&mut aud_dec_frame) {
                let enc_frame_size = if aud_enc.frame_size() > 0 {
                    aud_enc.frame_size()
                } else {
                    aud_dec_frame.nb_samples()
                };

                aud_enc_frame.set_format(aud_enc.sample_fmt());
                aud_enc_frame.set_sample_rate(aud_enc.sample_rate());
                aud_enc_frame.set_nb_samples(enc_frame_size);
                let _ = aud_enc_frame.set_ch_layout(aud_enc.ch_layout());

                if aud_enc_frame.get_buffer(0).is_err() {
                    aud_dec_frame.unref();
                    continue;
                }

                // Resample the decoded planes into the encoder frame.
                let in_planes: Vec<&[u8]> =
                    (0..).map_while(|i| aud_dec_frame.audio_plane(i)).collect();
                let in_count = aud_dec_frame.nb_samples();
                let samples_out = if let Some(swr) = swr_ctx.as_mut() {
                    swr.convert_into_frame(&mut aud_enc_frame, &in_planes, in_count)
                        .ok()
                } else {
                    None
                };

                if let Some(n) = samples_out
                    && n > 0
                {
                    aud_enc_frame.set_nb_samples(n);
                    aud_enc_frame.set_pts(audio_sample_count);
                    if aud_enc.send_frame(aud_enc_frame.as_ptr()).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc.as_mut_ptr(),
                            out_ctx.as_mut_ptr(),
                            aud_out_stream_idx,
                            "dash",
                            aud_frame_period,
                        );
                    }
                    audio_sample_count += i64::from(n);
                }

                aud_enc_frame.unref();
                aud_dec_frame.unref();
            }
        } else {
            pkt.unref();
        }
    }

    // ── 14. Flush encoders ────────────────────────────────────────────────────
    let _ = vid_enc_ctx.send_frame(ptr::null());
    crate::codec_utils::drain_encoder(
        vid_enc_ctx.as_mut_ptr(),
        out_ctx.as_mut_ptr(),
        vid_out_stream_idx,
        "dash",
        vid_frame_period,
    );

    if let Some(aud_enc) = aud_enc_ctx.as_mut() {
        // Flush resampler
        if swr_ctx.is_some() {
            let enc_frame_size = if aud_enc.frame_size() > 0 {
                aud_enc.frame_size()
            } else {
                1024
            };
            aud_enc_frame.set_format(aud_enc.sample_fmt());
            aud_enc_frame.set_sample_rate(aud_enc.sample_rate());
            aud_enc_frame.set_nb_samples(enc_frame_size);
            let _ = aud_enc_frame.set_ch_layout(aud_enc.ch_layout());
            if aud_enc_frame.get_buffer(0).is_ok() {
                // Flush the resampler with a NULL input. `convert_into_frame`
                // models the normal-input case; the flush needs a NULL input
                // pointer, so the raw `convert` is used here.
                let flushed = if let Some(swr) = swr_ctx.as_mut() {
                    swr.convert(
                        (*aud_enc_frame.as_mut_ptr()).data.as_mut_ptr(),
                        enc_frame_size,
                        ptr::null(),
                        0,
                    )
                    .ok()
                } else {
                    None
                };
                if let Some(n) = flushed
                    && n > 0
                {
                    aud_enc_frame.set_nb_samples(n);
                    aud_enc_frame.set_pts(audio_sample_count);
                    if aud_enc.send_frame(aud_enc_frame.as_ptr()).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc.as_mut_ptr(),
                            out_ctx.as_mut_ptr(),
                            aud_out_stream_idx,
                            "dash",
                            aud_frame_period,
                        );
                    }
                }
                aud_enc_frame.unref();
            }
        }
        let _ = aud_enc.send_frame(ptr::null());
        crate::codec_utils::drain_encoder(
            aud_enc.as_mut_ptr(),
            out_ctx.as_mut_ptr(),
            aud_out_stream_idx,
            "dash",
            aud_frame_period,
        );
    }

    // ── 15. Finalize ──────────────────────────────────────────────────────────
    let _ = out_ctx.write_trailer();
    // pb was already closed via close_io after the header; skip double-close.

    // Owned frames/packet, encoder / decoder / resample / scale contexts, and the
    // owned `input_ctx` / `out_ctx` all drop on scope exit; no manual teardown.

    log::info!(
        "dash write complete video_frames={video_frame_count} \
         audio_samples={audio_sample_count}"
    );

    Ok(())
}

// ============================================================================
// ABR multi-representation DASH write loop
// ============================================================================

pub(super) unsafe fn write_dash_abr_unsafe(
    input_path: &str,
    output_dir: &str,
    segment_duration_secs: f64,
    renditions: &[(i64, i32, i32)],
) -> Result<(), StreamError> {
    ff_sys::ensure_initialized();

    // ── 1. Open input ─────────────────────────────────────────────────────────
    // The owned demux context frees itself (closing the input) on every early
    // return below, so no manual teardown is needed on any path.
    let mut input_ctx = ff_sys::InputFormatContext::open(std::path::Path::new(input_path))
        .map_err(|e| ffmpeg_err(e.code()))?;

    input_ctx
        .find_stream_info()
        .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 2. Locate video and audio streams ─────────────────────────────────────
    let nb_streams = input_ctx.nb_streams() as usize;
    let mut video_stream_idx: i32 = -1;
    let mut audio_stream_idx: i32 = -1;

    for i in 0..nb_streams {
        let stream = *(*input_ctx.as_ptr()).streams.add(i);
        let codec_type = (*(*stream).codecpar).codec_type;
        if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO && video_stream_idx < 0 {
            video_stream_idx = i as i32;
        } else if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO && audio_stream_idx < 0 {
            audio_stream_idx = i as i32;
        }
    }

    if video_stream_idx < 0 {
        return Err(StreamError::InvalidConfig {
            reason: "input file contains no video stream".into(),
        });
    }

    // ── 3. Read video stream properties ──────────────────────────────────────
    let video_stream = *(*input_ctx.as_ptr()).streams.add(video_stream_idx as usize);
    let video_codecpar = (*video_stream).codecpar;
    let video_fps = detect_fps(video_stream, input_ctx.as_mut_ptr());
    let fps_int = video_fps.round().max(1.0) as i32;
    let keyframe_interval = (segment_duration_secs * fps_int as f64).round().max(1.0) as u32;

    // ── 4. Open input video decoder ────────────────────────────────────────────
    let vid_codec_id = (*video_codecpar).codec_id;
    let vid_decoder = ff_sys::Codec::find_decoder(vid_codec_id)
        .ok_or_else(|| ffmpeg_err_msg("no video decoder available for input stream"))?;

    let mut vid_dec_ctx =
        ff_sys::CodecContext::new(Some(vid_decoder)).map_err(|e| ffmpeg_err(e.code()))?;

    vid_dec_ctx
        .parameters_to_context(video_codecpar)
        .map_err(|e| ffmpeg_err(e.code()))?;

    vid_dec_ctx
        .open(vid_decoder, ptr::null_mut())
        .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 5. Open input audio decoder (optional) ────────────────────────────────
    let mut aud_dec_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_sample_rate: i32 = 44100;
    let mut aud_nb_channels: i32 = 2;

    if audio_stream_idx >= 0 {
        let audio_stream = *(*input_ctx.as_ptr()).streams.add(audio_stream_idx as usize);
        let audio_codecpar = (*audio_stream).codecpar;
        let aud_codec_id = (*audio_codecpar).codec_id;

        if let Some(aud_decoder) = ff_sys::Codec::find_decoder(aud_codec_id) {
            if let Ok(mut ctx) = ff_sys::CodecContext::new(Some(aud_decoder)) {
                if ctx.parameters_to_context(audio_codecpar).is_ok()
                    && ctx.open(aud_decoder, ptr::null_mut()).is_ok()
                {
                    aud_sample_rate = (*ctx.as_ptr()).sample_rate;
                    aud_nb_channels = (*ctx.as_ptr()).ch_layout.nb_channels;
                    aud_dec_ctx = Some(ctx);
                    log::info!(
                        "dash abr audio decoder opened sample_rate={aud_sample_rate} \
                         channels={aud_nb_channels}"
                    );
                } else {
                    // `ctx` drops here (frees the decoder context).
                    audio_stream_idx = -1;
                    log::warn!("dash abr audio decoder open failed, skipping audio");
                }
            } else {
                audio_stream_idx = -1;
                log::warn!("dash abr audio decoder alloc failed, skipping audio");
            }
        } else {
            audio_stream_idx = -1;
            log::warn!("dash abr no audio decoder found, skipping audio");
        }
    }

    // ── 6. Allocate DASH output context ───────────────────────────────────────
    // Both `input_ctx` and `out_ctx` are owned: every early return below drops
    // them (closing the input / the output's `pb` and freeing) with no manual
    // teardown on any path.
    let manifest_path = format!("{output_dir}/manifest.mpd");

    let mut out_ctx =
        ff_sys::OutputFormatContext::new(Some("dash"), std::path::Path::new(&manifest_path))
            .map_err(|e| ffmpeg_err(e.code()))?;

    // ── 7. Set DASH muxer options ──────────────────────────────────────────────
    let seg_duration_str = format!("{}", segment_duration_secs as u32);
    if let Ok(c_seg_dur) = CString::new(seg_duration_str.as_str()) {
        let ret = av_opt_set(
            (*out_ctx.as_mut_ptr()).priv_data,
            c"seg_duration".as_ptr(),
            c_seg_dur.as_ptr(),
            0,
        );
        if ret < 0 {
            log::warn!(
                "dash abr seg_duration option not supported, using default \
                 requested={seg_duration_str} error={}",
                ff_sys::av_error_string(ret)
            );
        }
    }

    // ── 8. Select H.264 encoder (shared across all renditions) ────────────────
    let Some(vid_enc_codec) = select_h264_encoder() else {
        return Err(ffmpeg_err_msg(
            "no H.264 encoder available (tried h264_nvenc, h264_qsv, h264_amf, \
             h264_videotoolbox, libx264, mpeg4)",
        ));
    };

    // ── 9. Create one encoder + output stream per rendition ───────────────────
    // On any error the `rendition_states` Vec drops all prior renditions'
    // owned encoder / scale contexts; only the raw format contexts need
    // explicit teardown.
    let mut rendition_states: Vec<RenditionState> = Vec::with_capacity(renditions.len());

    for &(target_bitrate, target_width, target_height) in renditions {
        let mut enc_ctx = match ff_sys::CodecContext::new(Some(vid_enc_codec)) {
            Ok(ctx) => ctx,
            Err(e) => {
                return Err(ffmpeg_err(e.code()));
            }
        };
        let ecptr = enc_ctx.as_mut_ptr();

        (*ecptr).width = target_width;
        (*ecptr).height = target_height;
        (*ecptr).time_base.num = 1;
        (*ecptr).time_base.den = fps_int;
        (*ecptr).framerate.num = fps_int;
        (*ecptr).framerate.den = 1;
        (*ecptr).pix_fmt = AVPixelFormat_AV_PIX_FMT_YUV420P;
        (*ecptr).bit_rate = target_bitrate;

        if let Err(e) = enc_ctx.open(vid_enc_codec, ptr::null_mut()) {
            // `enc_ctx` and the prior renditions drop on return.
            return Err(ffmpeg_err(e.code()));
        }

        let out_stream = avformat_new_stream(out_ctx.as_mut_ptr(), vid_enc_codec.as_ptr());
        if out_stream.is_null() {
            return Err(ffmpeg_err_msg(
                "cannot create video output stream for rendition",
            ));
        }
        (*out_stream).time_base = (*enc_ctx.as_ptr()).time_base;
        let stream_idx = (out_ctx.nb_streams() - 1) as i32;

        // SAFETY: out_stream and enc_ctx are valid; avcodec_open2 has been called.
        if let Err(e) = enc_ctx.parameters_from_context((*out_stream).codecpar) {
            return Err(ffmpeg_err(e.code()));
        }

        log::info!(
            "dash abr rendition added width={target_width} height={target_height} \
             bit_rate={target_bitrate} stream_idx={stream_idx}"
        );

        rendition_states.push(RenditionState {
            vid_enc_ctx: enc_ctx,
            vid_out_stream_idx: stream_idx,
            enc_width: target_width,
            enc_height: target_height,
            sws_ctx: None,
            last_src_fmt: None,
            last_src_w: None,
            last_src_h: None,
        });
    }

    // ── 10. Open AAC audio encoder and add audio stream (optional) ────────────
    let mut aud_enc_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_out_stream_idx: i32 = -1;
    let mut swr_ctx: Option<ff_sys::ResampleContext> = None;

    if audio_stream_idx >= 0 {
        match open_aac_encoder(aud_sample_rate, aud_nb_channels) {
            Ok(ctx) => {
                let aud_out_stream = avformat_new_stream(out_ctx.as_mut_ptr(), ptr::null());
                if aud_out_stream.is_null() {
                    // `ctx` drops here (frees the codec context).
                    log::warn!("dash abr cannot create audio output stream, skipping audio");
                    audio_stream_idx = -1;
                } else {
                    (*aud_out_stream).time_base.num = 1;
                    (*aud_out_stream).time_base.den = aud_sample_rate;
                    aud_out_stream_idx = (out_ctx.nb_streams() - 1) as i32;

                    let enc_ptr = ctx.as_ptr();
                    if !(*aud_out_stream).codecpar.is_null() {
                        (*(*aud_out_stream).codecpar).codec_id = (*enc_ptr).codec_id;
                        (*(*aud_out_stream).codecpar).codec_type =
                            ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO;
                        (*(*aud_out_stream).codecpar).sample_rate = (*enc_ptr).sample_rate;
                        (*(*aud_out_stream).codecpar).format = (*enc_ptr).sample_fmt;
                        let _ = ff_sys::swresample::channel_layout::copy(
                            &mut (*(*aud_out_stream).codecpar).ch_layout,
                            &(*enc_ptr).ch_layout,
                        );
                    }

                    if let Some(dec) = aud_dec_ctx.as_ref() {
                        let dec_ptr = dec.as_ptr();
                        if let Ok(swr) = ff_sys::ResampleContext::new(
                            &(*enc_ptr).ch_layout,
                            (*enc_ptr).sample_fmt,
                            (*enc_ptr).sample_rate,
                            &(*dec_ptr).ch_layout,
                            (*dec_ptr).sample_fmt,
                            (*dec_ptr).sample_rate,
                        ) {
                            swr_ctx = Some(swr);
                            aud_enc_ctx = Some(ctx);
                        } else {
                            // `ctx` drops here.
                            log::warn!("dash abr swr alloc failed, skipping audio");
                            audio_stream_idx = -1;
                        }
                    } else {
                        log::warn!("dash abr audio decoder missing, skipping audio");
                        audio_stream_idx = -1;
                    }
                }
            }
            Err(e) => {
                log::warn!("dash abr aac encoder unavailable: {e}, skipping audio");
                audio_stream_idx = -1;
            }
        }
    }

    // ── 11. Open output file and write header ─────────────────────────────────
    if let Err(e) = out_ctx.open_io(std::path::Path::new(&manifest_path)) {
        return Err(ffmpeg_err(e.code()));
    }

    if let Err(e) = out_ctx.write_header() {
        // The owned `out_ctx` closes `pb` and frees on drop at this return.
        return Err(ffmpeg_err(e.code()));
    }

    // Close pb so the DASH muxer can manage its own avio handles for segment
    // files without hitting a locked-file error on Windows. `close_io` nulls `pb`,
    // so the later drop only frees the context.
    out_ctx.close_io();

    log::info!(
        "dash abr output ready renditions={} fps={video_fps:.1} audio={}",
        rendition_states.len(),
        audio_stream_idx >= 0
    );

    // ── 12. Allocate frame and packet buffers ─────────────────────────────────
    // Owned frames/packet: each frees exactly once on drop. On the error arm any
    // successfully-allocated locals drop at the end of this statement.
    let (
        Ok(mut pkt),
        Ok(mut vid_dec_frame),
        Ok(mut vid_enc_frame),
        Ok(mut aud_dec_frame),
        Ok(mut aud_enc_frame),
    ) = (
        ff_sys::Packet::new(),
        ff_sys::Frame::new(),
        ff_sys::Frame::new(),
        ff_sys::Frame::new(),
        ff_sys::Frame::new(),
    )
    else {
        let _ = out_ctx.write_trailer();
        // `out_ctx` frees on drop at this return (pb already closed via close_io).
        return Err(ffmpeg_err_msg("cannot allocate frame or packet"));
    };

    // ── 13. Decode–encode loop ────────────────────────────────────────────────
    let mut video_frame_count: u64 = 0;
    let mut audio_sample_count: i64 = 0;

    // Frame period rationals passed to drain_encoder (immune to lazy enc time_base mutations).
    let vid_frame_period = AVRational {
        num: 1,
        den: fps_int,
    };
    let aud_frame_period = if let Some(aud) = aud_enc_ctx.as_ref() {
        AVRational {
            num: aud.frame_size(),
            den: aud.sample_rate(),
        }
    } else {
        AVRational { num: 1, den: 48000 } // unused; aud_enc_ctx is None
    };

    loop {
        match input_ctx.read_frame(&mut pkt) {
            Err(e) if e.is_eof() => break,
            Err(_) => {
                pkt.unref();
                continue;
            }
            Ok(()) => {}
        }

        let stream_idx = pkt.stream_index();

        if stream_idx == video_stream_idx {
            // ── Video path ────────────────────────────────────────────────────
            if vid_dec_ctx.send_packet(&pkt).is_err() {
                pkt.unref();
                continue;
            }
            pkt.unref();

            while let Ok(ReceiveOutcome::Frame) = vid_dec_ctx.receive_frame(&mut vid_dec_frame) {
                vid_dec_frame.set_pict_type(
                    if video_frame_count.is_multiple_of(u64::from(keyframe_interval)) {
                        AVPictureType_AV_PICTURE_TYPE_I
                    } else {
                        AVPictureType_AV_PICTURE_TYPE_NONE
                    },
                );

                let src_fmt = vid_dec_frame.format();
                let src_w = vid_dec_frame.width();
                let src_h = vid_dec_frame.height();

                for state in &mut rendition_states {
                    // Recreate SwsContext when source properties change
                    if state.last_src_fmt != Some(src_fmt)
                        || state.last_src_w != Some(src_w)
                        || state.last_src_h != Some(src_h)
                    {
                        // Move-assign the new context; the old one (if any) drops here.
                        if let Ok(ctx) = ff_sys::ScaleContext::new(
                            src_w,
                            src_h,
                            src_fmt,
                            state.enc_width,
                            state.enc_height,
                            AVPixelFormat_AV_PIX_FMT_YUV420P,
                            ff_sys::swscale::scale_flags::BILINEAR,
                        ) {
                            state.sws_ctx = Some(ctx);
                            state.last_src_fmt = Some(src_fmt);
                            state.last_src_w = Some(src_w);
                            state.last_src_h = Some(src_h);
                        } else {
                            continue;
                        }
                    }

                    vid_enc_frame.set_format(AVPixelFormat_AV_PIX_FMT_YUV420P);
                    vid_enc_frame.set_width(state.enc_width);
                    vid_enc_frame.set_height(state.enc_height);
                    // SAFETY: av_rescale_q is safe for valid AVRational values.
                    vid_enc_frame.set_pts(av_rescale_q(
                        video_frame_count as i64,
                        AVRational {
                            num: 1,
                            den: fps_int,
                        },
                        state.vid_enc_ctx.time_base(),
                    ));

                    if vid_enc_frame.get_buffer(0).is_err() {
                        continue;
                    }

                    let scaled = if let Some(sws) = state.sws_ctx.as_mut() {
                        sws.scale_frames(&vid_dec_frame, &mut vid_enc_frame).is_ok()
                    } else {
                        false
                    };

                    if scaled && state.vid_enc_ctx.send_frame(vid_enc_frame.as_ptr()).is_ok() {
                        crate::codec_utils::drain_encoder(
                            state.vid_enc_ctx.as_mut_ptr(),
                            out_ctx.as_mut_ptr(),
                            state.vid_out_stream_idx,
                            "dash",
                            vid_frame_period,
                        );
                    }

                    vid_enc_frame.unref();
                }

                vid_dec_frame.unref();
                video_frame_count += 1;
            }
        } else if stream_idx == audio_stream_idx
            && let Some(aud_dec) = aud_dec_ctx.as_mut()
            && let Some(aud_enc) = aud_enc_ctx.as_mut()
        {
            // ── Audio path ────────────────────────────────────────────────────
            if aud_dec.send_packet(&pkt).is_err() {
                pkt.unref();
                continue;
            }
            pkt.unref();

            while let Ok(ReceiveOutcome::Frame) = aud_dec.receive_frame(&mut aud_dec_frame) {
                let enc_frame_size = if aud_enc.frame_size() > 0 {
                    aud_enc.frame_size()
                } else {
                    aud_dec_frame.nb_samples()
                };

                aud_enc_frame.set_format(aud_enc.sample_fmt());
                aud_enc_frame.set_sample_rate(aud_enc.sample_rate());
                aud_enc_frame.set_nb_samples(enc_frame_size);
                let _ = aud_enc_frame.set_ch_layout(aud_enc.ch_layout());

                if aud_enc_frame.get_buffer(0).is_err() {
                    aud_dec_frame.unref();
                    continue;
                }

                // Resample the decoded planes into the encoder frame.
                let in_planes: Vec<&[u8]> =
                    (0..).map_while(|i| aud_dec_frame.audio_plane(i)).collect();
                let in_count = aud_dec_frame.nb_samples();
                let samples_out = if let Some(swr) = swr_ctx.as_mut() {
                    swr.convert_into_frame(&mut aud_enc_frame, &in_planes, in_count)
                        .ok()
                } else {
                    None
                };

                if let Some(n) = samples_out
                    && n > 0
                {
                    aud_enc_frame.set_nb_samples(n);
                    aud_enc_frame.set_pts(audio_sample_count);
                    if aud_enc.send_frame(aud_enc_frame.as_ptr()).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc.as_mut_ptr(),
                            out_ctx.as_mut_ptr(),
                            aud_out_stream_idx,
                            "dash",
                            aud_frame_period,
                        );
                    }
                    audio_sample_count += i64::from(n);
                }

                aud_enc_frame.unref();
                aud_dec_frame.unref();
            }
        } else {
            pkt.unref();
        }
    }

    // ── 14. Flush encoders ────────────────────────────────────────────────────
    for state in &mut rendition_states {
        let _ = state.vid_enc_ctx.send_frame(ptr::null());
        crate::codec_utils::drain_encoder(
            state.vid_enc_ctx.as_mut_ptr(),
            out_ctx.as_mut_ptr(),
            state.vid_out_stream_idx,
            "dash",
            vid_frame_period,
        );
    }

    if let Some(aud_enc) = aud_enc_ctx.as_mut() {
        if swr_ctx.is_some() {
            let enc_frame_size = if aud_enc.frame_size() > 0 {
                aud_enc.frame_size()
            } else {
                1024
            };
            aud_enc_frame.set_format(aud_enc.sample_fmt());
            aud_enc_frame.set_sample_rate(aud_enc.sample_rate());
            aud_enc_frame.set_nb_samples(enc_frame_size);
            let _ = aud_enc_frame.set_ch_layout(aud_enc.ch_layout());
            if aud_enc_frame.get_buffer(0).is_ok() {
                // Flush the resampler with a NULL input. `convert_into_frame`
                // models the normal-input case; the flush needs a NULL input
                // pointer, so the raw `convert` is used here.
                let flushed = if let Some(swr) = swr_ctx.as_mut() {
                    swr.convert(
                        (*aud_enc_frame.as_mut_ptr()).data.as_mut_ptr(),
                        enc_frame_size,
                        ptr::null(),
                        0,
                    )
                    .ok()
                } else {
                    None
                };
                if let Some(n) = flushed
                    && n > 0
                {
                    aud_enc_frame.set_nb_samples(n);
                    aud_enc_frame.set_pts(audio_sample_count);
                    if aud_enc.send_frame(aud_enc_frame.as_ptr()).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc.as_mut_ptr(),
                            out_ctx.as_mut_ptr(),
                            aud_out_stream_idx,
                            "dash",
                            aud_frame_period,
                        );
                    }
                }
                aud_enc_frame.unref();
            }
        }
        let _ = aud_enc.send_frame(ptr::null());
        crate::codec_utils::drain_encoder(
            aud_enc.as_mut_ptr(),
            out_ctx.as_mut_ptr(),
            aud_out_stream_idx,
            "dash",
            aud_frame_period,
        );
    }

    // ── 15. Finalize ──────────────────────────────────────────────────────────
    let _ = out_ctx.write_trailer();

    // Owned frames/packet, encoder / decoder / resample / scale contexts (including
    // every rendition in the Vec), and the owned `input_ctx` / `out_ctx` all drop on
    // scope exit; no manual teardown.

    log::info!(
        "dash abr write complete video_frames={video_frame_count} \
         audio_samples={audio_sample_count}"
    );

    Ok(())
}
