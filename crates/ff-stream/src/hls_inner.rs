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

use ff_sys::{
    AVPictureType_AV_PICTURE_TYPE_I, AVPictureType_AV_PICTURE_TYPE_NONE, AVPixelFormat,
    AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational, ReceiveOutcome, av_rescale_q,
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

    // 1. Open input
    // The owned demux context frees itself (closing the input) on every early
    // return below, so no manual teardown is needed on any path.
    let mut input_ctx = ff_sys::InputFormatContext::open(Path::new(input_path))
        .map_err(|e| ffmpeg_err(e.code()))?;

    input_ctx
        .find_stream_info()
        .map_err(|e| ffmpeg_err(e.code()))?;

    // 2. Locate video and audio streams
    let nb_streams = input_ctx.nb_streams() as usize;
    let mut video_stream_idx: i32 = -1;
    let mut audio_stream_idx: i32 = -1;

    for i in 0..nb_streams {
        let Some(stream) = input_ctx.stream(i) else {
            continue;
        };
        let codec_type = stream.codecpar().codec_type();
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

    // 3. Read video stream properties
    let vid_stream = input_ctx
        .stream(video_stream_idx as usize)
        .ok_or_else(|| ffmpeg_err_msg("video stream not found"))?;
    let vid_par = vid_stream.codecpar();
    let enc_width = if target_width > 0 {
        target_width
    } else {
        vid_par.width()
    };
    let enc_height = if target_height > 0 {
        target_height
    } else {
        vid_par.height()
    };
    let video_fps = detect_fps(
        vid_stream.avg_frame_rate(),
        vid_stream.r_frame_rate(),
        vid_stream.nb_frames(),
        input_ctx.duration(),
    );
    let fps_int = video_fps.round().max(1.0) as i32;

    // 4. Open input video decoder
    let vid_codec_id = vid_par.codec_id();
    let vid_decoder = ff_sys::Codec::find_decoder(vid_codec_id)
        .ok_or_else(|| ffmpeg_err_msg("no video decoder available for input stream"))?;

    let mut vid_dec_ctx =
        ff_sys::CodecContext::new(Some(vid_decoder)).map_err(|e| ffmpeg_err(e.code()))?;

    vid_dec_ctx
        .apply_parameters(&vid_par)
        .map_err(|e| ffmpeg_err(e.code()))?;

    vid_dec_ctx
        .open_codec(vid_decoder)
        .map_err(|e| ffmpeg_err(e.code()))?;

    // 5. Open input audio decoder (optional)
    let mut aud_dec_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_sample_rate: i32 = 44100;
    let mut aud_nb_channels: i32 = 2;

    if audio_stream_idx >= 0
        && let Some(aud_par) = input_ctx
            .stream(audio_stream_idx as usize)
            .map(|s| s.codecpar())
    {
        let aud_codec_id = aud_par.codec_id();

        if let Some(aud_decoder) = ff_sys::Codec::find_decoder(aud_codec_id) {
            if let Ok(mut ctx) = ff_sys::CodecContext::new(Some(aud_decoder)) {
                if ctx.apply_parameters(&aud_par).is_ok() && ctx.open_codec(aud_decoder).is_ok() {
                    aud_sample_rate = ctx.sample_rate();
                    aud_nb_channels = ctx.ch_layout().nb_channels;
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

    // 6. Allocate HLS output context
    // Both `input_ctx` and `out_ctx` are owned: every early return below drops
    // them (closing the input / the output's `pb` and freeing) with no manual
    // teardown on any path.
    let playlist_path = format!("{output_dir}/playlist.m3u8");

    let mut out_ctx = ff_sys::OutputFormatContext::new(Some("hls"), Path::new(&playlist_path))
        .map_err(|e| ffmpeg_err(e.code()))?;

    // 7. Set HLS muxer options
    let seg_time_str = format!("{}", segment_duration_secs as u32);
    let use_fmp4 = segment_format == crate::hls::HlsSegmentFormat::Fmp4;
    let seg_ext = if use_fmp4 { "m4s" } else { "ts" };
    let seg_filename = format!("{output_dir}/segment%03d.{seg_ext}");
    if let (Ok(c_seg_time), Ok(c_seg_file)) = (
        CString::new(seg_time_str.as_str()),
        CString::new(seg_filename.as_str()),
    ) {
        if let Err(e) = out_ctx.set_opt(c"hls_time", &c_seg_time) {
            log::warn!(
                "hls_time option not supported, using default \
                 requested={seg_time_str} error={}",
                ff_sys::av_error_string(e.code())
            );
        }
        if let Err(e) = out_ctx.set_opt(c"hls_segment_filename", &c_seg_file) {
            log::warn!(
                "hls_segment_filename option not supported, using default \
                 requested={seg_filename} error={}",
                ff_sys::av_error_string(e.code())
            );
        }
        if use_fmp4 && let Err(e) = out_ctx.set_opt(c"hls_segment_type", c"fmp4") {
            log::warn!(
                "hls_segment_type fmp4 option not supported error={}",
                ff_sys::av_error_string(e.code())
            );
        }
    }

    // 8. Open H.264 video encoder
    let vid_enc_codec = crate::codec_utils::select_h264_encoder("hls").ok_or_else(|| {
        ffmpeg_err_msg("no H.264 encoder available (tried h264_nvenc, h264_qsv, h264_amf, h264_videotoolbox, libx264, mpeg4)")
    })?;

    let mut vid_enc_ctx =
        ff_sys::CodecContext::new(Some(vid_enc_codec)).map_err(|e| ffmpeg_err(e.code()))?;

    vid_enc_ctx.set_width(enc_width);
    vid_enc_ctx.set_height(enc_height);
    vid_enc_ctx.set_time_base(AVRational {
        num: 1,
        den: fps_int,
    });
    vid_enc_ctx.set_framerate(AVRational {
        num: fps_int,
        den: 1,
    });
    vid_enc_ctx.set_pix_fmt(AVPixelFormat_AV_PIX_FMT_YUV420P);
    vid_enc_ctx.set_bit_rate(if target_bitrate > 0 {
        target_bitrate
    } else {
        2_000_000
    });

    // On error the owned `vid_enc_ctx` / decoders / `input_ctx` / `out_ctx` all
    // drop; no manual teardown is needed.
    vid_enc_ctx
        .open_codec(vid_enc_codec)
        .map_err(|e| ffmpeg_err(e.code()))?;

    // 9. Add video output stream
    let vid_out_stream_idx = out_ctx
        .new_stream(Some(&vid_enc_codec))
        .map_err(|e| ffmpeg_err(e.code()))? as i32;
    out_ctx.set_stream_time_base(vid_out_stream_idx as usize, vid_enc_ctx.time_base());
    out_ctx
        .apply_stream_params_from_context(vid_out_stream_idx as usize, &vid_enc_ctx)
        .map_err(|e| ffmpeg_err(e.code()))?;

    // 10. Open AAC audio encoder and add audio stream (optional)
    let mut aud_enc_ctx: Option<ff_sys::CodecContext> = None;
    let mut aud_out_stream_idx: i32 = -1;
    let mut swr_ctx: Option<ff_sys::ResampleContext> = None;

    if audio_stream_idx >= 0 {
        match crate::codec_utils::open_aac_encoder(aud_sample_rate, aud_nb_channels, 192_000, "hls")
        {
            Ok(ctx) => {
                if let Ok(aud_idx) = out_ctx.new_stream(None) {
                    let aud_idx = aud_idx as i32;
                    aud_out_stream_idx = aud_idx;
                    out_ctx.set_stream_time_base(
                        aud_idx as usize,
                        AVRational {
                            num: 1,
                            den: aud_sample_rate,
                        },
                    );
                    if out_ctx
                        .apply_stream_params_from_context(aud_idx as usize, &ctx)
                        .is_err()
                    {
                        log::warn!("hls audio stream codecpar copy failed");
                    }

                    // Set up resampler: decoded audio → FLTP at aud_sample_rate.
                    if let Some(dec) = aud_dec_ctx.as_ref() {
                        if let Ok(swr) = ff_sys::ResampleContext::new(
                            ctx.ch_layout(),
                            ctx.sample_fmt(),
                            ctx.sample_rate(),
                            dec.ch_layout(),
                            dec.sample_fmt(),
                            dec.sample_rate(),
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
                } else {
                    // `ctx` drops here (frees the codec context).
                    log::warn!("hls cannot create audio output stream, skipping audio");
                    audio_stream_idx = -1;
                }
            }
            Err(e) => {
                log::warn!("hls aac encoder unavailable: {e}, skipping audio");
                audio_stream_idx = -1;
            }
        }
    }

    // 11. Open output file and write header
    out_ctx
        .open_io(Path::new(&playlist_path))
        .map_err(|e| ffmpeg_err(e.code()))?;

    // On header-write failure the owned `out_ctx` (closing `pb`) and `input_ctx`
    // drop at this early return.
    out_ctx.write_header().map_err(|e| ffmpeg_err(e.code()))?;

    // Close pb now so the HLS muxer can rename its .tmp playlist files without
    // hitting a locked-file error on Windows.  The HLS muxer manages its own avio
    // handles for all subsequent playlist writes. `close_io` nulls `pb`, so the
    // later drop only frees the context.
    out_ctx.close_io();

    log::info!(
        "hls output context ready width={enc_width} height={enc_height} fps={video_fps:.1} \
         bit_rate={} audio={}",
        vid_enc_ctx.bit_rate(),
        audio_stream_idx >= 0,
    );

    // 12. Allocate frame and packet buffers
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
        // `out_ctx` and `input_ctx` free on drop at this return.
        return Err(ffmpeg_err_msg("cannot allocate frame or packet"));
    };

    // 13. Decode–encode loop
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
            // Video path
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

                if scaled && vid_enc_ctx.send_frame(Some(&vid_enc_frame)).is_ok() {
                    crate::codec_utils::drain_encoder(
                        &mut vid_enc_ctx,
                        &mut out_ctx,
                        vid_out_stream_idx as usize,
                        "hls",
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
            // Audio path
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
                    if aud_enc.send_frame(Some(&aud_enc_frame)).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc,
                            &mut out_ctx,
                            aud_out_stream_idx as usize,
                            "hls",
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

    // 14. Flush encoders
    let _ = vid_enc_ctx.send_frame(None);
    crate::codec_utils::drain_encoder(
        &mut vid_enc_ctx,
        &mut out_ctx,
        vid_out_stream_idx as usize,
        "hls",
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
                // Flush the resampler with a NULL input; `flush_into_frame`
                // drains the buffered samples into the frame's `nb_samples`
                // (set to `enc_frame_size` above).
                let flushed = if let Some(swr) = swr_ctx.as_mut() {
                    // SAFETY: `aud_enc_frame` was just `get_buffer`'d, so its
                    //         output planes are allocated for the flush.
                    swr.flush_into_frame(&mut aud_enc_frame).ok()
                } else {
                    None
                };
                if let Some(n) = flushed
                    && n > 0
                {
                    aud_enc_frame.set_nb_samples(n);
                    aud_enc_frame.set_pts(audio_sample_count);
                    if aud_enc.send_frame(Some(&aud_enc_frame)).is_ok() {
                        crate::codec_utils::drain_encoder(
                            aud_enc,
                            &mut out_ctx,
                            aud_out_stream_idx as usize,
                            "hls",
                            aud_frame_period,
                        );
                    }
                }
                aud_enc_frame.unref();
            }
        }
        let _ = aud_enc.send_frame(None);
        crate::codec_utils::drain_encoder(
            aud_enc,
            &mut out_ctx,
            aud_out_stream_idx as usize,
            "hls",
            aud_frame_period,
        );
    }

    // 15. Finalize
    let _ = out_ctx.write_trailer();
    // pb was already closed via close_io after the header; skip double-close.

    // Owned frames/packet, encoder / decoder / resample / scale contexts, and the
    // owned `input_ctx` / `out_ctx` all drop on scope exit; no manual teardown.

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
fn detect_fps(avg: AVRational, rfr: AVRational, nb_frames: i64, duration: i64) -> f64 {
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
    if let Some(fps) = try_rational(avg.num, avg.den) {
        return fps;
    }

    // 2. r_frame_rate — constant-framerate indicator
    if let Some(fps) = try_rational(rfr.num, rfr.den) {
        return fps;
    }

    // 3. Derive from nb_frames and total duration (robust for MPEG-4 Part 2)
    // `duration` is in AV_TIME_BASE (1 000 000) microseconds.
    if nb_frames > 0 && duration > 0 {
        let fps = nb_frames as f64 / (duration as f64 / 1_000_000.0);
        if (MIN_FPS..=MAX_FPS).contains(&fps) {
            return fps;
        }
    }

    25.0 // sane default
}

#[cfg(test)]
mod tests {
    use super::detect_fps;
    use ff_sys::AVRational;

    fn r(num: i32, den: i32) -> AVRational {
        AVRational { num, den }
    }

    #[test]
    fn detect_fps_should_prefer_avg_frame_rate() {
        assert_eq!(detect_fps(r(30, 1), r(60, 1), 0, 0), 30.0);
    }

    #[test]
    fn detect_fps_should_fall_back_to_r_frame_rate_when_avg_out_of_range() {
        // avg = 1_250_000/49 ≈ 25510 fps is outside [1, 240] and is rejected.
        assert_eq!(detect_fps(r(1_250_000, 49), r(24, 1), 0, 0), 24.0);
    }

    #[test]
    fn detect_fps_should_derive_from_nb_frames_and_duration() {
        // 300 frames over 10 s (10_000_000 µs) = 30 fps, distinct from the 25.0 default.
        assert_eq!(detect_fps(r(0, 0), r(0, 0), 300, 10_000_000), 30.0);
    }

    #[test]
    fn detect_fps_should_default_to_25_when_all_sources_unknown() {
        assert_eq!(detect_fps(r(0, 0), r(0, 0), 0, 0), 25.0);
    }
}
