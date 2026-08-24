//! Codec option application and encoder selection helpers.
#![allow(unsafe_op_in_unsafe_fn)]
// FFmpeg-boundary lints: casts at the C ABI, pointer idioms, C-string
// literals, and FFI-wrapper ergonomics concentrate in this unsafe module.
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::manual_c_str_literals)]
#![allow(clippy::unused_self)]

use super::{
    AVCodecID, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_AC3, AVCodecID_AV_CODEC_ID_ALAC,
    AVCodecID_AV_CODEC_ID_AV1, AVCodecID_AV_CODEC_ID_DNXHD, AVCodecID_AV_CODEC_ID_DTS,
    AVCodecID_AV_CODEC_ID_EAC3, AVCodecID_AV_CODEC_ID_FFV1, AVCodecID_AV_CODEC_ID_FLAC,
    AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC, AVCodecID_AV_CODEC_ID_MJPEG,
    AVCodecID_AV_CODEC_ID_MP3, AVCodecID_AV_CODEC_ID_MPEG2VIDEO, AVCodecID_AV_CODEC_ID_MPEG4,
    AVCodecID_AV_CODEC_ID_NONE, AVCodecID_AV_CODEC_ID_OPUS, AVCodecID_AV_CODEC_ID_PCM_S16LE,
    AVCodecID_AV_CODEC_ID_PCM_S24LE, AVCodecID_AV_CODEC_ID_PNG, AVCodecID_AV_CODEC_ID_PRORES,
    AVCodecID_AV_CODEC_ID_VORBIS, AVCodecID_AV_CODEC_ID_VP8, AVCodecID_AV_CODEC_ID_VP9, AudioCodec,
    CString, EncodeError, VideoCodec, VideoEncoderInner, avcodec,
};

/// Convert VideoCodec to FFmpeg AVCodecID.
pub(super) fn codec_to_id(codec: VideoCodec) -> AVCodecID {
    match codec {
        VideoCodec::H264 => AVCodecID_AV_CODEC_ID_H264,
        VideoCodec::H265 => AVCodecID_AV_CODEC_ID_HEVC,
        VideoCodec::Vp9 => AVCodecID_AV_CODEC_ID_VP9,
        VideoCodec::Av1 => AVCodecID_AV_CODEC_ID_AV1,
        VideoCodec::Av1Svt => AVCodecID_AV_CODEC_ID_AV1,
        VideoCodec::ProRes => AVCodecID_AV_CODEC_ID_PRORES,
        VideoCodec::DnxHd => AVCodecID_AV_CODEC_ID_DNXHD,
        VideoCodec::Mpeg4 => AVCodecID_AV_CODEC_ID_MPEG4,
        VideoCodec::Vp8 => AVCodecID_AV_CODEC_ID_VP8,
        VideoCodec::Mpeg2 => AVCodecID_AV_CODEC_ID_MPEG2VIDEO,
        VideoCodec::Mjpeg => AVCodecID_AV_CODEC_ID_MJPEG,
        VideoCodec::Png => AVCodecID_AV_CODEC_ID_PNG,
        VideoCodec::Ffv1 => AVCodecID_AV_CODEC_ID_FFV1,
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

pub fn preset_to_string(preset: crate::Preset) -> String {
    match preset {
        crate::Preset::Ultrafast => "ultrafast",
        crate::Preset::Faster => "faster",
        crate::Preset::Fast => "fast",
        crate::Preset::Medium => "medium",
        crate::Preset::Slow => "slow",
        crate::Preset::Slower => "slower",
        crate::Preset::Veryslow => "veryslow",
    }
    .to_string()
}

/// Convert AudioCodec to FFmpeg AVCodecID.
pub(super) fn audio_codec_to_id(codec: AudioCodec) -> AVCodecID {
    match codec {
        AudioCodec::Aac => AVCodecID_AV_CODEC_ID_AAC,
        AudioCodec::Opus => AVCodecID_AV_CODEC_ID_OPUS,
        AudioCodec::Mp3 => AVCodecID_AV_CODEC_ID_MP3,
        AudioCodec::Flac => AVCodecID_AV_CODEC_ID_FLAC,
        AudioCodec::Pcm => AVCodecID_AV_CODEC_ID_PCM_S16LE,
        AudioCodec::Pcm16 => AVCodecID_AV_CODEC_ID_PCM_S16LE,
        AudioCodec::Pcm24 => AVCodecID_AV_CODEC_ID_PCM_S24LE,
        AudioCodec::Vorbis => AVCodecID_AV_CODEC_ID_VORBIS,
        AudioCodec::Ac3 => AVCodecID_AV_CODEC_ID_AC3,
        AudioCodec::Eac3 => AVCodecID_AV_CODEC_ID_EAC3,
        AudioCodec::Dts => AVCodecID_AV_CODEC_ID_DTS,
        AudioCodec::Alac => AVCodecID_AV_CODEC_ID_ALAC,
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

impl VideoEncoderInner {
    /// Apply per-codec options to an allocated (not yet opened) codec context.
    ///
    /// All option failures are logged as a warning and skipped — they never
    /// propagate as an error. Must be called **before** `avcodec_open2`.
    pub(super) fn apply_codec_options(
        codec_ctx: &mut ff_sys::CodecContext,
        opts: &crate::video::codec_options::VideoCodecOptions,
        encoder_name: &str,
    ) {
        use crate::video::codec_options::VideoCodecOptions;

        match opts {
            VideoCodecOptions::H264(h264) => {
                // profile
                if codec_ctx.set_opt("profile", h264.profile.as_str()).is_err() {
                    log::warn!(
                        "av_opt_set failed option=profile value={} encoder={encoder_name}",
                        h264.profile.as_str()
                    );
                }
                // level (only when explicitly set)
                if let Some(level) = h264.level
                    && codec_ctx.set_opt("level", &level.to_string()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=level value={level} \
                         encoder={encoder_name}"
                    );
                }
                // Direct codec context fields.
                codec_ctx.set_max_b_frames(h264.bframes as i32);
                codec_ctx.set_gop_size(h264.gop_size as i32);
                codec_ctx.set_refs(h264.refs as i32);
                // preset (libx264-specific; hardware encoders return a negative value
                // which we log and skip — never returned as an error)
                if let Some(preset) = h264.preset
                    && codec_ctx.set_opt("preset", preset.as_str()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=preset value={} \
                         encoder={encoder_name}",
                        preset.as_str()
                    );
                }
                // tune (libx264-specific; hardware encoders return a negative value
                // which we log and skip — never returned as an error)
                if let Some(tune) = h264.tune
                    && codec_ctx.set_opt("tune", tune.as_str()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=tune value={} \
                         encoder={encoder_name}",
                        tune.as_str()
                    );
                }
            }
            VideoCodecOptions::H265(h265) => {
                // profile
                if codec_ctx.set_opt("profile", h265.profile.as_str()).is_err() {
                    log::warn!(
                        "av_opt_set failed option=profile value={} encoder={encoder_name}",
                        h265.profile.as_str()
                    );
                }
                // Auto-select yuv420p10le for Main10 (may be overridden by an explicit
                // pixel_format() call applied after apply_codec_options returns).
                if h265.profile == crate::video::codec_options::H265Profile::Main10 {
                    codec_ctx.set_pix_fmt(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE);
                }
                // tier
                if codec_ctx.set_opt("tier", h265.tier.as_str()).is_err() {
                    log::warn!(
                        "av_opt_set failed option=tier value={} encoder={encoder_name}",
                        h265.tier.as_str()
                    );
                }
                // level (only when explicitly set)
                if let Some(level) = h265.level
                    && codec_ctx.set_opt("level", &level.to_string()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=level value={level} \
                         encoder={encoder_name}"
                    );
                }
                // preset (libx265-specific; hardware HEVC encoders return a negative value
                // which we log and skip — never returned as an error)
                if let Some(ref preset) = h265.preset
                    && codec_ctx.set_opt("preset", preset.as_str()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=preset value={preset} \
                         encoder={encoder_name}"
                    );
                }
                // x265-params raw passthrough (libx265 only)
                if let Some(ref params) = h265.x265_params
                    && codec_ctx.set_opt("x265-params", params.as_str()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=x265-params value={params} \
                         encoder={encoder_name}"
                    );
                }
            }
            VideoCodecOptions::Av1(av1) => {
                // cpu-used
                if codec_ctx
                    .set_opt("cpu-used", &av1.cpu_used.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=cpu-used value={} encoder={encoder_name}",
                        av1.cpu_used
                    );
                }
                // tile-rows
                if codec_ctx
                    .set_opt("tile-rows", &av1.tile_rows.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=tile-rows value={} encoder={encoder_name}",
                        av1.tile_rows
                    );
                }
                // tile-columns
                if codec_ctx
                    .set_opt("tile-columns", &av1.tile_cols.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=tile-columns value={} \
                         encoder={encoder_name}",
                        av1.tile_cols
                    );
                }
                // usage
                if codec_ctx.set_opt("usage", av1.usage.as_str()).is_err() {
                    log::warn!(
                        "av_opt_set failed option=usage value={} encoder={encoder_name}",
                        av1.usage.as_str()
                    );
                }
            }
            VideoCodecOptions::Av1Svt(svt) => {
                // preset (0–13)
                if codec_ctx
                    .set_opt("preset", &svt.preset.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=preset value={} \
                         encoder={encoder_name}",
                        svt.preset
                    );
                }
                // tile-rows
                if codec_ctx
                    .set_opt("tile_rows", &svt.tile_rows.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=tile_rows value={} \
                         encoder={encoder_name}",
                        svt.tile_rows
                    );
                }
                // tile-columns
                if codec_ctx
                    .set_opt("tile_columns", &svt.tile_cols.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=tile_columns value={} \
                         encoder={encoder_name}",
                        svt.tile_cols
                    );
                }
                // svtav1-params raw passthrough
                if let Some(ref params) = svt.svtav1_params
                    && codec_ctx.set_opt("svtav1-params", params.as_str()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=svtav1-params value={params} \
                         encoder={encoder_name}"
                    );
                }
            }
            VideoCodecOptions::Vp9(vp9) => {
                // CQ mode: override bitrate to 0 and set crf
                if let Some(cq) = vp9.cq_level {
                    codec_ctx.set_bit_rate(0);
                    if codec_ctx.set_opt("crf", &cq.to_string()).is_err() {
                        log::warn!(
                            "av_opt_set failed option=crf value={cq} \
                             encoder={encoder_name}"
                        );
                    }
                }
                // cpu-used
                if codec_ctx
                    .set_opt("cpu-used", &vp9.cpu_used.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=cpu-used value={} \
                         encoder={encoder_name}",
                        vp9.cpu_used
                    );
                }
                // tile-columns
                if codec_ctx
                    .set_opt("tile-columns", &vp9.tile_columns.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=tile-columns value={} \
                         encoder={encoder_name}",
                        vp9.tile_columns
                    );
                }
                // tile-rows
                if codec_ctx
                    .set_opt("tile-rows", &vp9.tile_rows.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=tile-rows value={} \
                         encoder={encoder_name}",
                        vp9.tile_rows
                    );
                }
                // row-mt
                let row_mt_str = if vp9.row_mt { "1" } else { "0" };
                if codec_ctx.set_opt("row-mt", row_mt_str).is_err() {
                    log::warn!(
                        "av_opt_set failed option=row-mt value={row_mt_str} \
                         encoder={encoder_name}"
                    );
                }
            }
            VideoCodecOptions::ProRes(prores) => {
                // Set pixel format based on profile before avcodec_open2.
                // 4444 profiles need yuva444p10le; 422 profiles need yuv422p10le.
                if prores.profile.is_4444() {
                    codec_ctx.set_pix_fmt(ff_sys::AVPixelFormat_AV_PIX_FMT_YUVA444P10LE);
                } else {
                    codec_ctx.set_pix_fmt(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE);
                }

                // Apply profile via the private-option setter.
                if codec_ctx
                    .set_opt("profile", &prores.profile.profile_id().to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=profile value={} \
                         encoder={encoder_name}",
                        prores.profile.profile_id()
                    );
                }

                // Apply optional vendor tag (a 4-byte FourCC). Pass the raw bytes
                // via a CString so a non-ASCII tag is not mangled by a lossy
                // UTF-8 conversion.
                if let Some(vendor) = prores.vendor {
                    match std::ffi::CString::new(&vendor[..]) {
                        Ok(vendor_c) => {
                            if codec_ctx.set_opt_cstr("vendor", &vendor_c).is_err() {
                                log::warn!(
                                    "av_opt_set failed option=vendor \
                                     encoder={encoder_name}"
                                );
                            }
                        }
                        Err(_) => log::warn!(
                            "prores vendor tag has an interior NUL; skipping \
                             encoder={encoder_name}"
                        ),
                    }
                }
            }
            VideoCodecOptions::Dnxhd(dnxhd) => {
                use ff_format::PixelFormat;

                // Set pixel format based on variant before avcodec_open2.
                codec_ctx.set_pix_fmt(match dnxhd.variant.pixel_format() {
                    PixelFormat::Yuv422p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P,
                    PixelFormat::Yuv422p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE,
                    PixelFormat::Yuv444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE,
                    _ => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P,
                });

                // For DNxHD variants, override bit_rate with the required fixed rate.
                if let Some(bps) = dnxhd.variant.fixed_bitrate_bps() {
                    codec_ctx.set_bit_rate(bps);
                }

                // Apply vprofile on the codec context with AV_OPT_SEARCH_CHILDREN.
                // Targeting the context (not priv_data) lets the option be found in
                // child objects including priv_data.
                if codec_ctx
                    .set_opt_search_children("vprofile", dnxhd.variant.vprofile_str())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=vprofile value={} \
                         encoder={encoder_name}",
                        dnxhd.variant.vprofile_str()
                    );
                }
            }
        }
    }

    /// Select best available video encoder for the given codec.
    ///
    /// This method implements LGPL-compliant codec selection with automatic fallback:
    /// - For H.264: Hardware encoders → libx264 (GPL only) → VP9 fallback
    /// - For H.265: Hardware encoders → libx265 (GPL only) → AV1 fallback
    /// - Hardware encoders (NVENC, QSV, AMF, VideoToolbox) are LGPL-compatible
    /// - VP9 and AV1 are LGPL-compatible
    pub(super) fn select_video_encoder(
        &self,
        codec: VideoCodec,
        hardware_encoder: crate::HardwareEncoder,
    ) -> Result<String, EncodeError> {
        // Early check: when Av1Svt is requested, verify that libsvtav1 is registered.
        if codec == VideoCodec::Av1Svt {
            // SAFETY: find_encoder_by_name is always safe to call with a valid NUL-terminated
            // C string literal; the returned pointer is owned by FFmpeg and must not be freed.
            let has_svt = unsafe {
                avcodec::find_encoder_by_name(b"libsvtav1\0".as_ptr() as *const i8).is_some()
            };
            if !has_svt {
                return Err(EncodeError::EncoderUnavailable {
                    codec: "av1/svt".to_string(),
                    hint: "Requires an FFmpeg build with --enable-libsvtav1 (LGPL)".to_string(),
                });
            }
        }

        // Early check: when H265 is requested, verify that at least one HEVC encoder
        // is registered in this FFmpeg build before attempting candidate selection.
        if codec == VideoCodec::H265 {
            // SAFETY: avcodec::find_encoder is always safe to call with a valid codec ID;
            // the returned pointer is owned by FFmpeg and must not be freed.
            let has_hevc = unsafe { avcodec::find_encoder(AVCodecID_AV_CODEC_ID_HEVC).is_some() };
            if !has_hevc {
                return Err(EncodeError::EncoderUnavailable {
                    codec: "h265/hevc".to_string(),
                    hint: "Requires an FFmpeg build with HEVC encoder support \
                           (hardware: hevc_nvenc/hevc_qsv/etc.; \
                           software: --enable-libx265, GPL)"
                        .to_string(),
                });
            }
        }

        let candidates: Vec<&str> = match codec {
            VideoCodec::H264 => self.select_h264_encoder_candidates(hardware_encoder),
            VideoCodec::H265 => self.select_h265_encoder_candidates(hardware_encoder),
            VideoCodec::Vp9 => vec!["libvpx-vp9"],
            VideoCodec::Av1 => vec!["libaom-av1", "libsvtav1", "av1"],
            VideoCodec::Av1Svt => vec!["libsvtav1"],
            VideoCodec::ProRes => vec!["prores_ks", "prores"],
            VideoCodec::DnxHd => vec!["dnxhd"],
            VideoCodec::Mpeg4 => vec!["mpeg4"],
            VideoCodec::Vp8 => vec!["libvpx"],
            VideoCodec::Mpeg2 => vec!["mpeg2video"],
            VideoCodec::Mjpeg => vec!["mjpeg"],
            VideoCodec::Png => vec!["png"],
            VideoCodec::Ffv1 => vec!["ffv1"],
            _ => vec![],
        };

        // Try each candidate
        for &name in &candidates {
            unsafe {
                let c_name = CString::new(name).map_err(|_| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Invalid encoder name".to_string(),
                })?;
                if avcodec::find_encoder_by_name(c_name.as_ptr()).is_some() {
                    return Ok(name.to_string());
                }
            }
        }

        Err(EncodeError::NoSuitableEncoder {
            codec: format!("{:?}", codec),
            tried: candidates.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    /// Select H.264 encoder candidates with LGPL compliance.
    ///
    /// Priority order:
    /// 1. Hardware encoders (LGPL-compatible)
    /// 2. libx264 (GPL only, requires `gpl` feature)
    /// 3. VP9 fallback (LGPL-compatible)
    pub(super) fn select_h264_encoder_candidates(
        &self,
        hardware_encoder: crate::HardwareEncoder,
    ) -> Vec<&'static str> {
        let mut candidates = Vec::new();

        // Add hardware encoders based on preference
        #[cfg(feature = "hwaccel")]
        match hardware_encoder {
            crate::HardwareEncoder::Nvenc => {
                candidates.extend_from_slice(&["h264_nvenc", "h264_qsv", "h264_amf"]);
            }
            crate::HardwareEncoder::Qsv => {
                candidates.extend_from_slice(&["h264_qsv", "h264_nvenc", "h264_amf"]);
            }
            crate::HardwareEncoder::Amf => {
                candidates.extend_from_slice(&["h264_amf", "h264_nvenc", "h264_qsv"]);
            }
            crate::HardwareEncoder::VideoToolbox => {
                candidates.push("h264_videotoolbox");
            }
            crate::HardwareEncoder::Vaapi => {
                candidates.push("h264_vaapi");
            }
            crate::HardwareEncoder::Auto => {
                candidates.extend_from_slice(&[
                    "h264_nvenc",
                    "h264_qsv",
                    "h264_amf",
                    "h264_videotoolbox",
                    "h264_vaapi",
                ]);
            }
            crate::HardwareEncoder::None => {
                // Skip hardware encoders
            }
        }

        // Add GPL encoder if feature is enabled
        #[cfg(feature = "gpl")]
        {
            candidates.push("libx264");
        }

        // Add LGPL-compatible fallback (VP9)
        candidates.push("libvpx-vp9");

        candidates
    }

    /// Select H.265 encoder candidates with LGPL compliance.
    ///
    /// Priority order:
    /// 1. Hardware encoders (LGPL-compatible)
    /// 2. libx265 (GPL only, requires `gpl` feature)
    /// 3. AV1 fallback (LGPL-compatible)
    pub(super) fn select_h265_encoder_candidates(
        &self,
        hardware_encoder: crate::HardwareEncoder,
    ) -> Vec<&'static str> {
        let mut candidates = Vec::new();

        // Add hardware encoders based on preference
        #[cfg(feature = "hwaccel")]
        match hardware_encoder {
            crate::HardwareEncoder::Nvenc => {
                candidates.extend_from_slice(&["hevc_nvenc", "hevc_qsv", "hevc_amf"]);
            }
            crate::HardwareEncoder::Qsv => {
                candidates.extend_from_slice(&["hevc_qsv", "hevc_nvenc", "hevc_amf"]);
            }
            crate::HardwareEncoder::Amf => {
                candidates.extend_from_slice(&["hevc_amf", "hevc_nvenc", "hevc_qsv"]);
            }
            crate::HardwareEncoder::VideoToolbox => {
                candidates.push("hevc_videotoolbox");
            }
            crate::HardwareEncoder::Vaapi => {
                candidates.push("hevc_vaapi");
            }
            crate::HardwareEncoder::Auto => {
                candidates.extend_from_slice(&[
                    "hevc_nvenc",
                    "hevc_qsv",
                    "hevc_amf",
                    "hevc_videotoolbox",
                    "hevc_vaapi",
                ]);
            }
            crate::HardwareEncoder::None => {
                // Skip hardware encoders
            }
        }

        // Add GPL encoder if feature is enabled
        #[cfg(feature = "gpl")]
        {
            candidates.push("libx265");
        }

        // Add LGPL-compatible fallback (AV1)
        candidates.extend_from_slice(&["libaom-av1", "libsvtav1"]);

        candidates
    }
}
