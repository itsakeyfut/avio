//! Internal HLS muxing implementation using `FFmpeg` directly.
//!
//! This module implements the decode → encode → HLS-mux loop that powers
//! [`HlsOutput::write`](crate::hls::HlsOutput::write).  All `unsafe` code is
//! isolated here; `hls.rs` is purely safe Rust.

// This module is intentionally unsafe — it drives the FFmpeg C API directly.
#![allow(unsafe_code)]
// Rust 2024: Allow unsafe operations in unsafe functions for FFmpeg C API
#![allow(unsafe_op_in_unsafe_fn)]
// FFmpeg C API frequently requires raw pointer casting and borrows-as-ptr
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::too_many_lines)]
// `&mut ptr` to get `*mut *mut T` is the standard FFmpeg double-pointer pattern
#![allow(clippy::borrow_as_ptr)]
// `&mut foo as *mut *mut _` is the standard way to pass double-pointers in FFmpeg
#![allow(clippy::ref_as_ptr)]

use std::ffi::CString;
use std::path::Path;
use std::ptr;

use ff_sys::{
    AVFormatContext, AVFrame, AVPictureType_AV_PICTURE_TYPE_I, AVPictureType_AV_PICTURE_TYPE_NONE,
    AVPixelFormat, AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational, av_frame_alloc, av_frame_free,
    av_frame_get_buffer, av_frame_unref, av_opt_set, av_packet_alloc, av_packet_free,
    av_packet_unref, av_rescale_q, av_write_trailer, avformat_alloc_output_context2,
    avformat_free_context, avformat_new_stream, avformat_write_header,
};

use crate::codec_utils::{ffmpeg_err, ffmpeg_err_msg};
use crate::error::StreamError;

// ============================================================================
// Public entry point (safe wrapper)
// ============================================================================

/// Write an HLS segmented stream for the given input file.
///
/// Creates `output_dir/playlist.m3u8` and numbered segment files
/// (`segment000.ts`, `segment001.ts`, …).
///
/// # Errors
///
/// Returns [`StreamError::Ffmpeg`] when any `FFmpeg` operation fails, or
/// [`StreamError::Io`] when directory creation fails.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_hls(
    input_path: &str,
    output_dir: &str,
    segment_duration_secs: f64,
    keyframe_interval: u32,
    target_bitrate: i64,
    target_width: i32,
    target_height: i32,
    segment_format: crate::hls::HlsSegmentFormat,
) -> Result<(), StreamError> {
    std::fs::create_dir_all(output_dir)?;
    // SAFETY: All FFmpeg resources are allocated and freed within this call.
    unsafe {
        write_hls_unsafe(
            input_path,
            output_dir,
            segment_duration_secs,
            keyframe_interval,
            target_bitrate,
            target_width,
            target_height,
            segment_format,
        )
    }
}

// ============================================================================
// Unsafe implementation
// ============================================================================

#[allow(clippy::too_many_arguments)]
unsafe fn write_hls_unsafe(
    input_path: &str,
    output_dir: &str,
    segment_duration_secs: f64,
    keyframe_interval: u32,
    target_bitrate: i64,
    target_width: i32,
    target_height: i32,
    segment_format: crate::hls::HlsSegmentFormat,
) -> Result<(), StreamError> {
    ff_sys::ensure_initialized();

    // ── 1. Open input ─────────────────────────────────────────────────────────
    let mut input_ctx = ff_sys::avformat::open_input(Path::new(input_path)).map_err(ffmpeg_err)?;

    ff_sys::avformat::find_stream_info(input_ctx).map_err(|e| {
        ff_sys::avformat::close_input(&mut input_ctx);
        ffmpeg_err(e)
    })?;

    // ── 2. Locate video and audio streams ─────────────────────────────────────
    let nb_streams = (*input_ctx).nb_streams as usize;
    let mut video_stream_idx: i32 = -1;
    let mut audio_stream_idx: i32 = -1;

    for i in 0..nb_streams {
        let stream = *(*input_ctx).streams.add(i);
        let codec_type = (*(*stream).codecpar).codec_type;
        if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO && video_stream_idx < 0 {
            video_stream_idx = i as i32;
        } else if codec_type == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO && audio_stream_idx < 0 {
            audio_stream_idx = i as i32;
        }
    }

    if video_stream_idx < 0 {
        ff_sys::avformat::close_input(&mut input_ctx);
        return Err(StreamError::InvalidConfig {
            reason: "input file contains no video stream".into(),
        });
    }

    // ── 3. Read video stream properties ──────────────────────────────────────
    let video_stream = *(*input_ctx).streams.add(video_stream_idx as usize);
    let video_codecpar = (*video_stream).codecpar;
    let enc_width = if target_width > 0 {
        target_width
    } else {
        (*video_codecpar).width
    };
    let enc_height = if target_height > 0 {
        target_height
    } else {
        (*video_codecpar).height
    };
    let video_fps = detect_fps(video_stream, input_ctx);
    let fps_int = video_fps.round().max(1.0) as i32;

    // ── 4. Open input video decoder ────────────────────────────────────────────
    let vid_codec_id = (*video_codecpar).codec_id;
    let vid_decoder = ff_sys::Codec::find_decoder(vid_codec_id)
        .ok_or_else(|| ffmpeg_err_msg("no video decoder available for input stream"))?;

    let mut vid_dec_ctx =
        ff_sys::CodecContext::new(Some(vid_decoder)).map_err(|e| ffmpeg_err(e.code()))?;

    vid_dec_ctx
        .parameters_to_context(video_codecpar)
        .map_err(|e| {
            ff_sys::avformat::close_input(&mut input_ctx);
            ffmpeg_err(e.code())
        })?;

    vid_dec_ctx
        .open(vid_decoder, ptr::null_mut())
        .map_err(|e| {
            ff_sys::avformat::close_input(&mut input_ctx);
            ffmpeg_err(e.code())
        })?;

    // ── 5. Open input audio decoder (optional) ────────────────────────────────
    let mut aud_dec_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_sample_rate: i32 = 44100;
    let mut aud_nb_channels: i32 = 2;

    if audio_stream_idx >= 0 {
        let audio_stream = *(*input_ctx).streams.add(audio_stream_idx as usize);
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
                        "hls audio decoder opened sample_rate={aud_sample_rate} \
                         channels={aud_nb_channels}"
                    );
                } else {
                    // `ctx` drops here (frees the decoder context).
                    audio_stream_idx = -1;
                    log::warn!("hls audio decoder open failed, skipping audio");
                }
            } else {
                audio_stream_idx = -1;
                log::warn!("hls audio decoder alloc failed, skipping audio");
            }
        } else {
            audio_stream_idx = -1;
            log::warn!("hls no audio decoder found, skipping audio");
        }
    }

    // ── 6. Allocate HLS output context ────────────────────────────────────────
    let playlist_path = format!("{output_dir}/playlist.m3u8");
    let c_playlist = CString::new(playlist_path.as_str())
        .map_err(|_| ffmpeg_err_msg("playlist path contains null byte"))?;
    let c_hls = c"hls";

    let mut out_ctx: *mut AVFormatContext = ptr::null_mut();
    let ret = avformat_alloc_output_context2(
        &mut out_ctx,
        ptr::null_mut(),
        c_hls.as_ptr(),
        c_playlist.as_ptr(),
    );
    if ret < 0 || out_ctx.is_null() {
        ff_sys::avformat::close_input(&mut input_ctx);
        return Err(ffmpeg_err(ret));
    }

    // ── 7. Set HLS muxer options ──────────────────────────────────────────────
    let seg_time_str = format!("{}", segment_duration_secs as u32);
    let use_fmp4 = segment_format == crate::hls::HlsSegmentFormat::Fmp4;
    let seg_ext = if use_fmp4 { "m4s" } else { "ts" };
    let seg_filename = format!("{output_dir}/segment%03d.{seg_ext}");
    if let (Ok(c_seg_time), Ok(c_seg_file)) = (
        CString::new(seg_time_str.as_str()),
        CString::new(seg_filename.as_str()),
    ) {
        let ret = av_opt_set(
            (*out_ctx).priv_data,
            c"hls_time".as_ptr(),
            c_seg_time.as_ptr(),
            0,
        );
        if ret < 0 {
            log::warn!(
                "hls_time option not supported, using default \
                 requested={seg_time_str} error={}",
                ff_sys::av_error_string(ret)
            );
        }
        let ret = av_opt_set(
            (*out_ctx).priv_data,
            c"hls_segment_filename".as_ptr(),
            c_seg_file.as_ptr(),
            0,
        );
        if ret < 0 {
            log::warn!(
                "hls_segment_filename option not supported, using default \
                 requested={seg_filename} error={}",
                ff_sys::av_error_string(ret)
            );
        }
        if use_fmp4 {
            let ret = av_opt_set(
                (*out_ctx).priv_data,
                c"hls_segment_type".as_ptr(),
                c"fmp4".as_ptr(),
                0,
            );
            if ret < 0 {
                log::warn!(
                    "hls_segment_type fmp4 option not supported error={}",
                    ff_sys::av_error_string(ret)
                );
            }
        }
    }

    // ── 8. Open H.264 video encoder ───────────────────────────────────────────
    let vid_enc_codec = crate::codec_utils::select_h264_encoder("hls").ok_or_else(|| {
        cleanup_output_ctx(out_ctx);
        ff_sys::avformat::close_input(&mut input_ctx);
        ffmpeg_err_msg("no H.264 encoder available (tried h264_nvenc, h264_qsv, h264_amf, h264_videotoolbox, libx264, mpeg4)")
    })?;

    let mut vid_enc_ctx = ff_sys::CodecContext::new(Some(vid_enc_codec)).map_err(|e| {
        cleanup_output_ctx(out_ctx);
        ff_sys::avformat::close_input(&mut input_ctx);
        ffmpeg_err(e.code())
    })?;
    let venc = vid_enc_ctx.as_mut_ptr();

    (*venc).width = enc_width;
    (*venc).height = enc_height;
    (*venc).time_base.num = 1;
    (*venc).time_base.den = fps_int;
    (*venc).framerate.num = fps_int;
    (*venc).framerate.den = 1;
    (*venc).pix_fmt = AVPixelFormat_AV_PIX_FMT_YUV420P;
    (*venc).bit_rate = if target_bitrate > 0 {
        target_bitrate
    } else {
        2_000_000
    };

    // On error the owned `vid_enc_ctx` / decoders drop; only the raw format
    // contexts need explicit teardown.
    vid_enc_ctx
        .open(vid_enc_codec, ptr::null_mut())
        .map_err(|e| {
            cleanup_output_ctx(out_ctx);
            ff_sys::avformat::close_input(&mut input_ctx);
            ffmpeg_err(e.code())
        })?;

    // ── 9. Add video output stream ────────────────────────────────────────────
    let vid_out_stream = avformat_new_stream(out_ctx, vid_enc_codec.as_ptr());
    if vid_out_stream.is_null() {
        cleanup_output_ctx(out_ctx);
        ff_sys::avformat::close_input(&mut input_ctx);
        return Err(ffmpeg_err_msg("cannot create video output stream"));
    }
    (*vid_out_stream).time_base = (*vid_enc_ctx.as_ptr()).time_base;
    let vid_out_stream_idx = ((*out_ctx).nb_streams - 1) as i32;

    // SAFETY: vid_out_stream and vid_enc_ctx are valid; avcodec_open2 has been called.
    vid_enc_ctx
        .parameters_from_context((*vid_out_stream).codecpar)
        .map_err(|e| {
            cleanup_output_ctx(out_ctx);
            ff_sys::avformat::close_input(&mut input_ctx);
            ffmpeg_err(e.code())
        })?;

    // ── 10. Open AAC audio encoder and add audio stream (optional) ────────────
    let mut aud_enc_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_out_stream_idx: i32 = -1;
    let mut swr_ctx: Option<ff_sys::ResampleContext> = None;

    if audio_stream_idx >= 0 {
        match crate::codec_utils::open_aac_encoder(aud_sample_rate, aud_nb_channels, 192_000, "hls")
        {
            Ok(ctx) => {
                let aud_out_stream = avformat_new_stream(out_ctx, ptr::null());
                if aud_out_stream.is_null() {
                    // `ctx` drops here (frees the codec context).
                    log::warn!("hls cannot create audio output stream, skipping audio");
                    audio_stream_idx = -1;
                } else {
                    (*aud_out_stream).time_base.num = 1;
                    (*aud_out_stream).time_base.den = aud_sample_rate;
                    aud_out_stream_idx = ((*out_ctx).nb_streams - 1) as i32;

                    // SAFETY: aud_out_stream and ctx are valid; avcodec_open2 called.
                    if ctx
                        .parameters_from_context((*aud_out_stream).codecpar)
                        .is_err()
                    {
                        log::warn!("hls audio stream codecpar copy failed");
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
                            log::warn!("hls swr alloc failed, skipping audio");
                            audio_stream_idx = -1;
                        }
                    } else {
                        log::warn!("hls audio decoder missing, skipping audio");
                        audio_stream_idx = -1;
                    }
                }
            }
            Err(e) => {
                log::warn!("hls aac encoder unavailable: {e}, skipping audio");
                audio_stream_idx = -1;
            }
        }
    }

    // ── 11. Open output file and write header ─────────────────────────────────
    let pb = ff_sys::avformat::open_output(
        Path::new(&playlist_path),
        ff_sys::avformat::avio_flags::WRITE,
    )
    .map_err(|e| {
        cleanup_output_ctx(out_ctx);
        ff_sys::avformat::close_input(&mut input_ctx);
        ffmpeg_err(e)
    })?;
    (*out_ctx).pb = pb;

    let ret = avformat_write_header(out_ctx, ptr::null_mut());
    if ret < 0 {
        ff_sys::avformat::close_output(&mut (*out_ctx).pb);
        cleanup_output_ctx(out_ctx);
        ff_sys::avformat::close_input(&mut input_ctx);
        return Err(ffmpeg_err(ret));
    }

    // Close pb now so the HLS muxer can rename its .tmp playlist files
    // without hitting a locked-file error on Windows.  The HLS muxer
    // manages its own avio handles for all subsequent playlist writes.
    ff_sys::avformat::close_output(&mut (*out_ctx).pb);

    log::info!(
        "hls output context ready width={enc_width} height={enc_height} fps={video_fps:.1} \
         bit_rate={} audio={}",
        (*vid_enc_ctx.as_ptr()).bit_rate,
        audio_stream_idx >= 0,
    );

    // ── 12. Allocate frame and packet buffers ──────────────────────────────────
    let mut pkt = av_packet_alloc();
    if pkt.is_null() {
        av_write_trailer(out_ctx);
        ff_sys::avformat::close_output(&mut (*out_ctx).pb);
        cleanup_output_ctx(out_ctx);
        ff_sys::avformat::close_input(&mut input_ctx);
        return Err(ffmpeg_err_msg("cannot allocate packet"));
    }

    let vid_dec_frame = av_frame_alloc();
    let vid_enc_frame = av_frame_alloc();
    let aud_dec_frame = av_frame_alloc();
    let aud_enc_frame = av_frame_alloc();

    if vid_dec_frame.is_null()
        || vid_enc_frame.is_null()
        || aud_dec_frame.is_null()
        || aud_enc_frame.is_null()
    {
        free_frames(vid_dec_frame, vid_enc_frame, aud_dec_frame, aud_enc_frame);
        av_packet_free(&mut pkt);
        av_write_trailer(out_ctx);
        ff_sys::avformat::close_output(&mut (*out_ctx).pb);
        cleanup_output_ctx(out_ctx);
        ff_sys::avformat::close_input(&mut input_ctx);
        return Err(ffmpeg_err_msg("cannot allocate frame"));
    }

    // ── 13. Decode–encode loop ─────────────────────────────────────────────────
    let mut video_frame_count: u64 = 0;
    let mut audio_sample_count: i64 = 0;
    let mut sws_ctx: Option<ff_sys::ScaleContext> = None;
    let mut last_src_fmt: Option<AVPixelFormat> = None;
    let mut last_src_w: Option<i32> = None;
    let mut last_src_h: Option<i32> = None;

    // Frame period rationals passed to drain_encoder so it can compute the correct
    // per-frame duration in enc_tb units at drain time (immune to lazy time_base
    // mutations by the encoder on first send_frame).
    let vid_frame_period = AVRational {
        num: 1,
        den: fps_int,
    };
    // SAFETY: aud_enc_ctx (if present) is valid after avcodec_open2.
    let aud_frame_period = if let Some(aud) = aud_enc_ctx.as_ref() {
        AVRational {
            num: (*aud.as_ptr()).frame_size,
            den: (*aud.as_ptr()).sample_rate,
        }
    } else {
        AVRational { num: 1, den: 48000 } // unused; aud_enc_ctx is None
    };

    loop {
        match ff_sys::avformat::read_frame(input_ctx, pkt) {
            Err(e) if e == ff_sys::error_codes::EOF => break,
            Err(_e) => {
                // Non-EOF read errors: continue to try next packet
                av_packet_unref(pkt);
                continue;
            }
            Ok(()) => {}
        }

        let stream_idx = (*pkt).stream_index;

        if stream_idx == video_stream_idx {
            // ── Video path ────────────────────────────────────────────────────
            if ff_sys::avcodec::send_packet(vid_dec_ctx.as_mut_ptr(), pkt).is_err() {
                av_packet_unref(pkt);
                continue;
            }
            av_packet_unref(pkt);

            loop {
                match ff_sys::avcodec::receive_frame(vid_dec_ctx.as_mut_ptr(), vid_dec_frame) {
                    Err(e) if e == ff_sys::error_codes::EAGAIN || e == ff_sys::error_codes::EOF => {
                        break;
                    }
                    Err(_) => break,
                    Ok(()) => {}
                }

                // Force keyframe at intervals
                (*vid_dec_frame).pict_type =
                    if video_frame_count.is_multiple_of(u64::from(keyframe_interval)) {
                        AVPictureType_AV_PICTURE_TYPE_I
                    } else {
                        AVPictureType_AV_PICTURE_TYPE_NONE
                    };

                // Convert decoded frame to YUV420P at encoder dimensions
                let src_fmt = (*vid_dec_frame).format;
                let src_w = (*vid_dec_frame).width;
                let src_h = (*vid_dec_frame).height;

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
                        av_frame_unref(vid_dec_frame);
                        continue;
                    }
                }

                // Prepare encoder frame
                (*vid_enc_frame).format = AVPixelFormat_AV_PIX_FMT_YUV420P;
                (*vid_enc_frame).width = enc_width;
                (*vid_enc_frame).height = enc_height;
                // SAFETY: av_rescale_q is safe for valid AVRational values.
                (*vid_enc_frame).pts = av_rescale_q(
                    video_frame_count as i64,
                    AVRational {
                        num: 1,
                        den: fps_int,
                    },
                    (*vid_enc_ctx.as_ptr()).time_base,
                );

                let buf_ret = av_frame_get_buffer(vid_enc_frame, 0);
                if buf_ret < 0 {
                    av_frame_unref(vid_dec_frame);
                    continue;
                }

                // Scale decoded frame into encoder frame
                let scaled = if let Some(sws) = sws_ctx.as_mut() {
                    sws.scale(
                        (*vid_dec_frame).data.as_ptr() as *const *const u8,
                        (*vid_dec_frame).linesize.as_ptr(),
                        0,
                        src_h,
                        (*vid_enc_frame).data.as_mut_ptr().cast_const(),
                        (*vid_enc_frame).linesize.as_mut_ptr(),
                    )
                    .is_ok()
                } else {
                    false
                };

                if scaled
                    && ff_sys::avcodec::send_frame(vid_enc_ctx.as_mut_ptr(), vid_enc_frame).is_ok()
                {
                    crate::codec_utils::drain_encoder(
                        vid_enc_ctx.as_mut_ptr(),
                        out_ctx,
                        vid_out_stream_idx,
                        "hls",
                        vid_frame_period,
                    );
                }

                av_frame_unref(vid_enc_frame);
                av_frame_unref(vid_dec_frame);
                video_frame_count += 1;
            }
        } else if stream_idx == audio_stream_idx
            && let Some(aud_dec_ptr) = aud_dec_ctx.as_mut().map(ff_sys::CodecContext::as_mut_ptr)
            && let Some(aud_enc_ptr) = aud_enc_ctx.as_mut().map(ff_sys::CodecContext::as_mut_ptr)
        {
            // ── Audio path ────────────────────────────────────────────────────
            if ff_sys::avcodec::send_packet(aud_dec_ptr, pkt).is_err() {
                av_packet_unref(pkt);
                continue;
            }
            av_packet_unref(pkt);

            loop {
                match ff_sys::avcodec::receive_frame(aud_dec_ptr, aud_dec_frame) {
                    Err(e) if e == ff_sys::error_codes::EAGAIN || e == ff_sys::error_codes::EOF => {
                        break;
                    }
                    Err(_) => break,
                    Ok(()) => {}
                }

                let enc_frame_size = if (*aud_enc_ptr).frame_size > 0 {
                    (*aud_enc_ptr).frame_size
                } else {
                    (*aud_dec_frame).nb_samples
                };

                (*aud_enc_frame).format = (*aud_enc_ptr).sample_fmt;
                (*aud_enc_frame).sample_rate = (*aud_enc_ptr).sample_rate;
                (*aud_enc_frame).nb_samples = enc_frame_size;
                let _ = ff_sys::swresample::channel_layout::copy(
                    &mut (*aud_enc_frame).ch_layout,
                    &(*aud_enc_ptr).ch_layout,
                );

                let buf_ret = av_frame_get_buffer(aud_enc_frame, 0);
                if buf_ret < 0 {
                    av_frame_unref(aud_dec_frame);
                    continue;
                }

                let in_data = (*aud_dec_frame).data.as_ptr() as *const *const u8;
                let in_samples = (*aud_dec_frame).nb_samples;

                let samples_out = if let Some(swr) = swr_ctx.as_mut() {
                    swr.convert(
                        (*aud_enc_frame).data.as_mut_ptr(),
                        enc_frame_size,
                        in_data,
                        in_samples,
                    )
                    .ok()
                } else {
                    None
                };

                if let Some(n) = samples_out
                    && n > 0
                {
                    (*aud_enc_frame).nb_samples = n;
                    (*aud_enc_frame).pts = audio_sample_count;
                    if ff_sys::avcodec::send_frame(aud_enc_ptr, aud_enc_frame).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc_ptr,
                            out_ctx,
                            aud_out_stream_idx,
                            "hls",
                            aud_frame_period,
                        );
                    }
                    audio_sample_count += i64::from(n);
                }

                av_frame_unref(aud_enc_frame);
                av_frame_unref(aud_dec_frame);
            }
        } else {
            av_packet_unref(pkt);
        }
    }

    // ── 14. Flush encoders ────────────────────────────────────────────────────
    let _ = vid_enc_ctx.send_frame(ptr::null());
    crate::codec_utils::drain_encoder(
        vid_enc_ctx.as_mut_ptr(),
        out_ctx,
        vid_out_stream_idx,
        "hls",
        vid_frame_period,
    );

    if let Some(aud_enc_ptr) = aud_enc_ctx.as_mut().map(ff_sys::CodecContext::as_mut_ptr) {
        // Flush resampler
        if swr_ctx.is_some() {
            let enc_frame_size = if (*aud_enc_ptr).frame_size > 0 {
                (*aud_enc_ptr).frame_size
            } else {
                1024
            };
            (*aud_enc_frame).format = (*aud_enc_ptr).sample_fmt;
            (*aud_enc_frame).sample_rate = (*aud_enc_ptr).sample_rate;
            (*aud_enc_frame).nb_samples = enc_frame_size;
            let _ = ff_sys::swresample::channel_layout::copy(
                &mut (*aud_enc_frame).ch_layout,
                &(*aud_enc_ptr).ch_layout,
            );
            if av_frame_get_buffer(aud_enc_frame, 0) == 0 {
                let flushed = if let Some(swr) = swr_ctx.as_mut() {
                    swr.convert(
                        (*aud_enc_frame).data.as_mut_ptr(),
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
                    (*aud_enc_frame).nb_samples = n;
                    (*aud_enc_frame).pts = audio_sample_count;
                    if ff_sys::avcodec::send_frame(aud_enc_ptr, aud_enc_frame).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc_ptr,
                            out_ctx,
                            aud_out_stream_idx,
                            "hls",
                            aud_frame_period,
                        );
                    }
                }
                av_frame_unref(aud_enc_frame);
            }
        }
        let _ = ff_sys::avcodec::send_frame(aud_enc_ptr, ptr::null());
        crate::codec_utils::drain_encoder(
            aud_enc_ptr,
            out_ctx,
            aud_out_stream_idx,
            "hls",
            aud_frame_period,
        );
    }

    // ── 15. Finalize ──────────────────────────────────────────────────────────
    av_write_trailer(out_ctx);
    // pb was already closed after avformat_write_header; skip double-close.

    // ── Cleanup ───────────────────────────────────────────────────────────────
    // Owned encoder / decoder / resample / scale contexts drop on scope exit;
    // only the raw frames and format contexts need explicit teardown.
    free_frames(vid_dec_frame, vid_enc_frame, aud_dec_frame, aud_enc_frame);
    av_packet_free(&mut pkt);

    cleanup_output_ctx(out_ctx);
    ff_sys::avformat::close_input(&mut input_ctx);

    log::info!(
        "hls write complete video_frames={video_frame_count} \
         audio_samples={audio_sample_count}"
    );

    Ok(())
}

// ============================================================================
// Helper: detect video frame rate
// ============================================================================

/// Return the best estimate of the video frame rate for `stream`.
///
/// Some containers (notably MPEG-4 Part 2) store pathological values in
/// `avg_frame_rate` (e.g. 1250000/49 ≈ 25510 fps) or `r_frame_rate`
/// (e.g. `time_increment_resolution/1` = 25000 fps).  Tries each candidate
/// in order and rejects values outside the sane range [1, 240] fps.
/// Falls back to `nb_frames/duration` and finally to 25 fps.
#[allow(clippy::cast_precision_loss)]
unsafe fn detect_fps(stream: *mut ff_sys::AVStream, fmt_ctx: *mut AVFormatContext) -> f64 {
    const MIN_FPS: f64 = 1.0;
    const MAX_FPS: f64 = 240.0;

    let try_rational = |num: i32, den: i32| -> Option<f64> {
        if den <= 0 || num <= 0 {
            return None;
        }
        let fps = num as f64 / den as f64;
        if (MIN_FPS..=MAX_FPS).contains(&fps) {
            Some(fps)
        } else {
            None
        }
    };

    // 1. avg_frame_rate — reliable for most containers
    let avg = (*stream).avg_frame_rate;
    if let Some(fps) = try_rational(avg.num, avg.den) {
        return fps;
    }

    // 2. r_frame_rate — constant-framerate indicator
    let rfr = (*stream).r_frame_rate;
    if let Some(fps) = try_rational(rfr.num, rfr.den) {
        return fps;
    }

    // 3. Derive from nb_frames and total duration (robust for MPEG-4 Part 2)
    let nb = (*stream).nb_frames;
    let dur = (*fmt_ctx).duration; // in AV_TIME_BASE (1 000 000) microseconds
    if nb > 0 && dur > 0 {
        let fps = nb as f64 / (dur as f64 / 1_000_000.0);
        if (MIN_FPS..=MAX_FPS).contains(&fps) {
            return fps;
        }
    }

    25.0 // sane default
}

// ============================================================================
// Cleanup helpers (safe to call with null pointers)
// ============================================================================

unsafe fn cleanup_output_ctx(mut out_ctx: *mut AVFormatContext) {
    if !out_ctx.is_null() {
        avformat_free_context(out_ctx);
        out_ctx = ptr::null_mut();
        let _ = out_ctx; // suppress unused warning
    }
}

unsafe fn free_frames(
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
