//! Internal audio encoder implementation.
//!
//! This module contains the internal implementation details of the audio encoder,
//! including FFmpeg context management and encoding operations.

// Rust 2024: Allow unsafe operations in unsafe functions for FFmpeg C API
#![allow(unsafe_code)]
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

use crate::audio::codec_options::{AudioCodecOptions, Mp3Quality};
use crate::{AudioCodec, EncodeError};
use ff_format::AudioFrame;
use ff_sys::{
    AVAudioFifo, AVChannelLayout, AVCodecID, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_AC3,
    AVCodecID_AV_CODEC_ID_ALAC, AVCodecID_AV_CODEC_ID_DTS, AVCodecID_AV_CODEC_ID_EAC3,
    AVCodecID_AV_CODEC_ID_FLAC, AVCodecID_AV_CODEC_ID_MP3, AVCodecID_AV_CODEC_ID_NONE,
    AVCodecID_AV_CODEC_ID_OPUS, AVCodecID_AV_CODEC_ID_PCM_S16LE, AVCodecID_AV_CODEC_ID_PCM_S24LE,
    AVCodecID_AV_CODEC_ID_VORBIS, OutputFormatContext, swresample,
};
use std::ptr;

/// Internal encoder state with FFmpeg contexts.
pub(super) struct AudioEncoderInner {
    /// Output format context (owned; frees itself and closes its IO on drop)
    pub(super) format_ctx: OutputFormatContext,

    /// Audio codec context
    pub(super) codec_ctx: Option<ff_sys::CodecContext>,

    /// Audio stream index
    pub(super) stream_index: i32,

    /// Resampling context for audio format conversion
    pub(super) swr_ctx: Option<ff_sys::ResampleContext>,

    /// Sample counter
    pub(super) sample_count: u64,

    /// Bytes written
    pub(super) bytes_written: u64,

    /// Actual audio codec name being used
    pub(super) actual_codec: String,

    /// FFmpeg format-aware sample FIFO.  Non-null when the encoder requires a
    /// fixed number of samples per frame (e.g. AAC: 1024, FLAC: 4096).
    fifo: *mut AVAudioFifo,

    /// Required samples per frame; 0 when the encoder accepts variable sizes.
    frame_size: usize,
}

/// AudioEncoder configuration (stored from builder).
#[derive(Debug, Clone)]
pub(super) struct AudioEncoderConfig {
    pub(super) path: std::path::PathBuf,
    pub(super) sample_rate: u32,
    pub(super) channels: u32,
    pub(super) codec: AudioCodec,
    pub(super) bitrate: Option<u64>,
    pub(super) codec_options: Option<AudioCodecOptions>,
    pub(super) _progress_callback: bool,
}

impl AudioEncoderInner {
    /// Create a new encoder with the given configuration.
    pub(super) fn new(config: &AudioEncoderConfig) -> Result<Self, EncodeError> {
        unsafe {
            ff_sys::ensure_initialized();

            // Allocate output format context (owned; muxer guessed from the path).
            // On any early return below, its `Drop` closes the IO and frees it.
            let format_ctx =
                OutputFormatContext::new(None, &config.path).map_err(|e| EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "Cannot create output context: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                })?;

            let mut encoder = Self {
                format_ctx,
                codec_ctx: None,
                stream_index: -1,
                swr_ctx: None,
                sample_count: 0,
                bytes_written: 0,
                actual_codec: String::new(),
                fifo: ptr::null_mut(),
                frame_size: 0,
            };

            // Initialize audio encoder
            encoder.init_audio_encoder(config)?;

            // Open output file (the owned context closes it on drop).
            encoder.format_ctx.open_io(&config.path).map_err(|_| {
                EncodeError::CannotCreateFile {
                    path: config.path.clone(),
                }
            })?;

            // Write file header
            encoder
                .format_ctx
                .write_header()
                .map_err(|e| EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!("Cannot write header: {}", ff_sys::av_error_string(e.code())),
                })?;

            Ok(encoder)
        }
    }

    /// Initialize audio encoder.
    unsafe fn init_audio_encoder(
        &mut self,
        config: &AudioEncoderConfig,
    ) -> Result<(), EncodeError> {
        // Select encoder based on codec and availability
        let encoder_name = self.select_audio_encoder(config.codec)?;
        self.actual_codec.clone_from(&encoder_name);

        let codec = ff_sys::Codec::find_encoder_by_name(&encoder_name).ok_or_else(|| {
            EncodeError::NoSuitableEncoder {
                codec: format!("{:?}", config.codec),
                tried: vec![encoder_name.clone()],
            }
        })?;
        let codec_ptr = codec.as_ptr();

        // Allocate codec context
        let mut codec_ctx = ff_sys::CodecContext::new(Some(codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // Configure codec context
        codec_ctx.set_codec_id(codec_to_id(config.codec));
        codec_ctx.set_sample_rate(config.sample_rate as i32);

        // Set channel layout using FFmpeg 7.x API
        codec_ctx.set_ch_layout_default(config.channels as i32);

        // Select the first sample format the codec actually supports; fall back to FLTP.
        // Reading sample_fmts before avcodec_open2 is required — the encoder uses
        // this value to decide its internal pipeline.
        let target_fmt = {
            let fmts = (*codec_ptr).sample_fmts;
            if !fmts.is_null() && *fmts != ff_sys::swresample::sample_format::NONE {
                // SAFETY: sample_fmts is a null-terminated array owned by the codec descriptor
                *fmts
            } else {
                ff_sys::swresample::sample_format::FLTP
            }
        };
        codec_ctx.set_sample_fmt(target_fmt);

        // Set bitrate
        if let Some(br) = config.bitrate {
            codec_ctx.set_bit_rate(br as i64);
        } else {
            // Default bitrate based on codec
            codec_ctx.set_bit_rate(match config.codec {
                AudioCodec::Aac => 192_000,
                AudioCodec::Opus => 128_000,
                AudioCodec::Mp3 => 192_000,
                AudioCodec::Flac => 0,  // Lossless
                AudioCodec::Pcm => 0,   // Uncompressed
                AudioCodec::Pcm16 => 0, // Uncompressed
                AudioCodec::Pcm24 => 0, // Uncompressed
                AudioCodec::Vorbis => 192_000,
                AudioCodec::Ac3 => 192_000,
                AudioCodec::Eac3 => 192_000,
                AudioCodec::Dts => 0,  // Lossless/variable
                AudioCodec::Alac => 0, // Lossless
                _ => 192_000,
            });
        }

        // Set time base
        codec_ctx.set_time_base(ff_sys::AVRational {
            num: 1,
            den: config.sample_rate as i32,
        });

        // Apply per-codec options before opening the codec context.
        if let Some(opts) = &config.codec_options {
            // Options are applied before avcodec_open2 so they take effect during
            // codec initialisation.
            Self::apply_codec_options(&mut codec_ctx, opts, &encoder_name);
        }

        // Open codec
        codec_ctx
            .open_codec(codec)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // After open2, frame_size is populated.  Allocate an AVAudioFifo when
        // the encoder requires a fixed number of samples and does not advertise
        // VARIABLE_FRAME_SIZE.  AVAudioFifo handles both planar and packed
        // layouts internally and is backed by a ring-buffer (O(1) read/write).
        let required = codec_ctx.frame_size() as usize;
        let caps = (*codec_ptr).capabilities as u32;
        if required > 0 && caps & ff_sys::avcodec::codec_caps::VARIABLE_FRAME_SIZE == 0 {
            self.fifo =
                swresample::audio_fifo::alloc(target_fmt, config.channels as i32, required as i32)
                    .map_err(|e| EncodeError::Ffmpeg {
                        code: e,
                        message: format!(
                            "Cannot allocate audio FIFO: {}",
                            ff_sys::av_error_string(e)
                        ),
                    })?;
            self.frame_size = required;
        }

        // Create stream. The owned codec_ctx frees itself if this returns.
        let stream_idx = self
            .format_ctx
            .new_stream(Some(&codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        self.format_ctx
            .set_stream_time_base(stream_idx, codec_ctx.time_base());

        // Copy ALL codec parameters (including extradata) from the open codec
        // context to the stream.  avcodec_parameters_from_context must be
        // called after avcodec_open2 because some codecs (e.g. FLAC, AAC)
        // populate extradata only after the codec is opened.  Manual field
        // copies would miss extradata, causing avformat_write_header to fail.
        self.format_ctx
            .apply_stream_params_from_context(stream_idx, &codec_ctx)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        self.stream_index = stream_idx as i32;
        self.codec_ctx = Some(codec_ctx);

        Ok(())
    }

    /// Apply per-codec options before `avcodec_open2`.
    ///
    /// All option failures are logged as a warning and the option is skipped
    /// (never returns an error).
    fn apply_codec_options(
        codec_ctx: &mut ff_sys::CodecContext,
        opts: &AudioCodecOptions,
        encoder_name: &str,
    ) {
        match opts {
            AudioCodecOptions::Opus(opus) => {
                // application
                if codec_ctx
                    .set_opt("application", opus.application.as_str())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=application value={} encoder={encoder_name}",
                        opus.application.as_str()
                    );
                }
                // frame_duration (libopus expects microseconds)
                if let Some(dur_ms) = opus.frame_duration_ms {
                    let dur_us_str = (i64::from(dur_ms) * 1000).to_string();
                    if codec_ctx.set_opt("frame_duration", &dur_us_str).is_err() {
                        log::warn!(
                            "av_opt_set failed option=frame_duration value={dur_us_str} \
                             encoder={encoder_name}"
                        );
                    }
                }
            }
            AudioCodecOptions::Aac(aac) => {
                // profile (aac_low / aac_he / aac_he_v2)
                let profile_str = aac.profile.as_str();
                if codec_ctx.set_opt("profile", profile_str).is_err() {
                    log::warn!(
                        "AAC profile={profile_str} not supported by encoder \
                         encoder={encoder_name}"
                    );
                }
                // vbr (libfdk_aac VBR quality 1–5)
                if let Some(q) = aac.vbr_quality
                    && codec_ctx.set_opt("vbr", &q.to_string()).is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=vbr value={q} \
                         encoder={encoder_name}"
                    );
                }
            }
            AudioCodecOptions::Mp3(mp3) => {
                match mp3.quality {
                    Mp3Quality::Vbr(q) => {
                        // VBR mode: override bitrate to 0 and set the libmp3lame q scale.
                        codec_ctx.set_bit_rate(0);
                        if codec_ctx.set_opt("q", &q.to_string()).is_err() {
                            log::warn!(
                                "av_opt_set failed option=q value={q} \
                                 encoder={encoder_name}"
                            );
                        }
                    }
                    Mp3Quality::Cbr(bitrate) => {
                        // CBR mode: set the fixed bitrate directly on the codec context.
                        codec_ctx.set_bit_rate(i64::from(bitrate));
                    }
                }
            }
            AudioCodecOptions::Flac(flac) => {
                // compression_level
                if codec_ctx
                    .set_opt("compression_level", &flac.compression_level.to_string())
                    .is_err()
                {
                    log::warn!(
                        "av_opt_set failed option=compression_level value={} \
                         encoder={encoder_name}",
                        flac.compression_level
                    );
                }
            }
        }
    }

    /// Select best available audio encoder for the given codec.
    fn select_audio_encoder(&self, codec: AudioCodec) -> Result<String, EncodeError> {
        let candidates: Vec<&str> = match codec {
            AudioCodec::Aac => vec!["aac", "libfdk_aac"],
            AudioCodec::Opus => vec!["libopus"],
            AudioCodec::Mp3 => vec!["libmp3lame", "mp3"],
            AudioCodec::Flac => vec!["flac"],
            AudioCodec::Pcm => vec!["pcm_s16le"],
            AudioCodec::Pcm16 => vec!["pcm_s16le"],
            AudioCodec::Pcm24 => vec!["pcm_s24le"],
            AudioCodec::Vorbis => vec!["libvorbis", "vorbis"],
            AudioCodec::Ac3 => vec!["ac3"],
            AudioCodec::Eac3 => vec!["eac3"],
            AudioCodec::Dts => vec![],
            AudioCodec::Alac => vec!["alac"],
            _ => vec![],
        };

        // Try each candidate
        for &name in &candidates {
            if ff_sys::Codec::find_encoder_by_name(name).is_some() {
                return Ok(name.to_string());
            }
        }

        Err(EncodeError::NoSuitableEncoder {
            codec: format!("{:?}", codec),
            tried: candidates.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    /// Push an audio frame for encoding.
    pub(super) fn push_frame(&mut self, frame: &AudioFrame) -> Result<(), EncodeError> {
        // SAFETY: self is properly initialised; all raw FFmpeg pointers are valid and exclusively owned.
        unsafe {
            if self.codec_ctx.is_none() {
                return Err(EncodeError::InvalidConfig {
                    reason: "Audio codec not initialized".to_string(),
                });
            }

            if !self.fifo.is_null() {
                // Fixed-frame-size path: convert → write into AVAudioFifo → drain
                // complete frames.  AVAudioFifo manages the ring buffer internally.
                let mut av_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Cannot allocate frame".to_string(),
                })?;

                self.convert_audio_frame(frame, &mut av_frame)?;

                let nb_samples = av_frame.nb_samples();

                // Write converted samples into AVAudioFifo.
                // SAFETY: av_frame data buffers were allocated by convert_audio_frame
                let write_result =
                    swresample::audio_fifo::write_frame(self.fifo, &av_frame, nb_samples);

                write_result.map_err(|e| EncodeError::Ffmpeg {
                    code: e,
                    message: format!(
                        "Failed to write to audio FIFO: {}",
                        ff_sys::av_error_string(e)
                    ),
                })?;

                // Drain all complete frames from the FIFO
                let frame_size = self.frame_size as i32;
                while swresample::audio_fifo::size(self.fifo) >= frame_size {
                    self.drain_fifo_frame(frame_size, false)?;
                }
            } else {
                // Direct path: send frame straight to the encoder.
                let mut av_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Cannot allocate frame".to_string(),
                })?;

                self.convert_audio_frame(frame, &mut av_frame)?;

                av_frame.set_pts(self.sample_count as i64);

                self.codec_ctx
                    .as_mut()
                    .ok_or_else(|| EncodeError::InvalidConfig {
                        reason: "Audio codec not initialized".to_string(),
                    })?
                    .send_frame(Some(&av_frame))
                    .map_err(|e| EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Failed to send audio frame: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    })?;

                self.receive_packets()?;

                self.sample_count += frame.samples() as u64;
            }

            Ok(())
        } // unsafe
    }

    /// Read `frame_size` samples from the FIFO into a new AVFrame and encode it.
    ///
    /// When `zero_pad` is `true` the frame buffer is zeroed before reading so
    /// that a partial tail (fewer samples than `frame_size`) is silence-padded
    /// to the required length.  `zero_pad` should be `false` in the normal drain
    /// loop (FIFO always contains a full frame's worth of samples) and `true`
    /// only in the EOF flush called from [`Self::finish`].
    unsafe fn drain_fifo_frame(
        &mut self,
        frame_size: i32,
        zero_pad: bool,
    ) -> Result<(), EncodeError> {
        let codec_ctx = self
            .codec_ctx
            .as_mut()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Audio codec not initialized".to_string(),
            })?;

        let mut av_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
            code: 0,
            message: "Cannot allocate frame".to_string(),
        })?;

        // Configure the audio scalar fields from the codec context.
        av_frame.set_format(codec_ctx.sample_fmt());
        av_frame.set_sample_rate(codec_ctx.sample_rate());
        av_frame.set_nb_samples(frame_size);
        av_frame.set_pts(self.sample_count as i64);
        av_frame
            .set_ch_layout(codec_ctx.ch_layout())
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        av_frame.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Cannot allocate audio frame buffer: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        if zero_pad {
            // Zero all plane sample regions so the unread tail is silence, not
            // garbage. Each plane slice spans exactly the frame's sample bytes.
            for i in 0..ff_sys::AV_NUM_DATA_POINTERS as usize {
                if let Some(plane) = av_frame.audio_plane_mut(i) {
                    plane.fill(0);
                }
            }
        }

        // Read from AVAudioFifo into the frame's plane buffers.
        // For zero_pad=false: FIFO has >= frame_size samples → returns exactly frame_size.
        // For zero_pad=true:  FIFO has < frame_size samples → returns < frame_size;
        //                     the zeroed tail provides silence padding.
        // SAFETY: get_buffer allocated the plane buffers; they are large enough
        swresample::audio_fifo::read_frame(self.fifo, &mut av_frame, frame_size).map_err(|e| {
            EncodeError::Ffmpeg {
                code: e,
                message: format!(
                    "Failed to read from audio FIFO: {}",
                    ff_sys::av_error_string(e)
                ),
            }
        })?;
        // nb_samples stays as frame_size: the encoder always receives a full frame.

        self.codec_ctx
            .as_mut()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Audio codec not initialized".to_string(),
            })?
            .send_frame(Some(&av_frame))
            .map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to send audio frame: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        self.receive_packets()?;
        self.sample_count += frame_size as u64;

        Ok(())
    }

    /// Convert AudioFrame into an owned output [`ff_sys::Frame`] with resampling
    /// if needed.
    unsafe fn convert_audio_frame(
        &mut self,
        src: &AudioFrame,
        dst: &mut ff_sys::Frame,
    ) -> Result<(), EncodeError> {
        let codec_ctx = self
            .codec_ctx
            .as_ref()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Audio codec not initialized".to_string(),
            })?;

        let target_sample_rate = codec_ctx.sample_rate();
        let target_format = codec_ctx.sample_fmt();
        let target_ch_layout = codec_ctx.ch_layout();

        // Check if we need to resample
        let src_sample_rate = src.sample_rate() as i32;
        let src_format = sample_format_to_av(src.format());
        let src_ch_layout = {
            let mut layout = AVChannelLayout::default();
            swresample::channel_layout::set_default(&raw mut layout, src.channels() as i32);
            layout
        };

        let needs_resampling = src_sample_rate != target_sample_rate
            || src_format != target_format
            || !swresample::channel_layout::is_equal(&raw const src_ch_layout, target_ch_layout);

        if needs_resampling {
            // Initialize resampler if needed (RAII: allocates, configures, and
            // initializes internally; frees itself on drop).
            if self.swr_ctx.is_none() {
                self.swr_ctx = Some(
                    ff_sys::ResampleContext::new(
                        target_ch_layout,
                        target_format,
                        target_sample_rate,
                        &src_ch_layout,
                        src_format,
                        src_sample_rate,
                    )
                    .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?,
                );
            }

            // Estimate output sample count
            let out_samples = swresample::estimate_output_samples(
                target_sample_rate,
                src_sample_rate,
                src.samples() as i32,
            );

            // Set frame properties from the encoder's target audio format.
            dst.set_format(target_format);
            dst.set_sample_rate(target_sample_rate);
            dst.set_nb_samples(out_samples);
            dst.set_ch_layout(target_ch_layout)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

            // Allocate frame buffer
            dst.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Cannot allocate audio frame buffer: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

            // Prepare input plane slices (planar: one per channel; packed: one).
            let in_planes: Vec<&[u8]> = if src.format().is_planar() {
                src.planes().iter().map(Vec::as_slice).collect()
            } else {
                vec![src.planes()[0].as_slice()]
            };

            // Convert into the output frame's planes.
            let samples_out = self
                .swr_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Resampling context not initialized".to_string(),
                })?
                .convert_into_frame(dst, &in_planes, src.samples() as i32)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

            dst.set_nb_samples(samples_out);
        } else {
            // No resampling needed, direct copy from the source's audio format.
            dst.set_format(src_format);
            dst.set_sample_rate(src_sample_rate);
            dst.set_nb_samples(src.samples() as i32);
            dst.set_ch_layout(&src_ch_layout)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

            // Allocate frame buffer
            dst.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Cannot allocate audio frame buffer: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

            // Copy audio data into the destination frame's planes.
            if src.format().is_planar() {
                for (i, plane) in src.planes().iter().enumerate() {
                    if let Some(dst_plane) = dst.audio_plane_mut(i) {
                        let n = plane.len().min(dst_plane.len());
                        dst_plane[..n].copy_from_slice(&plane[..n]);
                    }
                }
            } else if let Some(dst_plane) = dst.audio_plane_mut(0) {
                let src_plane = &src.planes()[0];
                let n = src_plane.len().min(dst_plane.len());
                dst_plane[..n].copy_from_slice(&src_plane[..n]);
            }
        }

        Ok(())
    }

    /// Receive encoded packets from the encoder.
    unsafe fn receive_packets(&mut self) -> Result<(), EncodeError> {
        if self.codec_ctx.is_none() {
            return Err(EncodeError::InvalidConfig {
                reason: "Audio codec not initialized".to_string(),
            });
        }

        let mut packet = ff_sys::Packet::new().map_err(|_| EncodeError::Ffmpeg {
            code: 0,
            message: "Cannot allocate packet".to_string(),
        })?;

        loop {
            let recv = self
                .codec_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Audio codec not initialized".to_string(),
                })?
                .receive_packet(&mut packet);
            match recv {
                Ok(ff_sys::ReceiveOutcome::Frame) => {
                    // Packet received successfully
                }
                Ok(ff_sys::ReceiveOutcome::NeedInput | ff_sys::ReceiveOutcome::Drained) => {
                    // No more packets available
                    break;
                }
                Err(e) => {
                    return Err(EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Error receiving audio packet: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    });
                }
            }

            // Set stream index
            packet.set_stream_index(self.stream_index);

            // Write packet
            if let Err(e) = self.format_ctx.write_interleaved(&mut packet) {
                packet.unref();
                return Err(EncodeError::MuxingFailed {
                    reason: ff_sys::av_error_string(e.code()),
                });
            }

            self.bytes_written += packet.size() as u64;

            packet.unref();
        }

        Ok(())
    }

    /// Finish encoding and write trailer.
    pub(super) fn finish(&mut self) -> Result<(), EncodeError> {
        // SAFETY: self is properly initialised; all raw FFmpeg pointers are valid and exclusively owned.
        unsafe {
            // Flush any remaining samples from the AVAudioFifo (silence-padded to a
            // full frame so the encoder always receives its required frame_size).
            if !self.fifo.is_null() && swresample::audio_fifo::size(self.fifo) > 0 {
                self.drain_fifo_frame(self.frame_size as i32, true)?;
            }

            // Flush audio encoder
            if self.codec_ctx.is_some() {
                // Send NULL frame to flush
                self.codec_ctx
                    .as_mut()
                    .ok_or_else(|| EncodeError::InvalidConfig {
                        reason: "Audio codec not initialized".to_string(),
                    })?
                    .send_frame(None)
                    .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
                self.receive_packets()?;
            }

            // Write trailer
            self.format_ctx
                .write_trailer()
                .map_err(|e| EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "Cannot write trailer: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                })?;

            Ok(())
        } // unsafe
    }

    /// Cleanup FFmpeg resources.
    unsafe fn cleanup(&mut self) {
        // Free AVAudioFifo
        if !self.fifo.is_null() {
            swresample::audio_fifo::free(self.fifo);
            self.fifo = ptr::null_mut();
        }

        // Free audio codec context (owned CodecContext frees itself when dropped).
        self.codec_ctx = None;

        // Free resampling context (owned ResampleContext drops on assignment).
        self.swr_ctx = None;

        // The owned `format_ctx` closes its IO and frees itself when the struct
        // drops; nothing to close here.
    }
}

impl Drop for AudioEncoderInner {
    fn drop(&mut self) {
        // SAFETY: We own all the FFmpeg resources and need to free them
        unsafe {
            self.cleanup();
        }
    }
}

// Helper functions

/// Convert AudioCodec to FFmpeg AVCodecID.
fn codec_to_id(codec: AudioCodec) -> AVCodecID {
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

/// Convert ff-format SampleFormat to FFmpeg AVSampleFormat.
fn sample_format_to_av(format: ff_format::SampleFormat) -> ff_sys::AVSampleFormat {
    use ff_format::SampleFormat;
    use ff_sys::swresample::sample_format;

    match format {
        SampleFormat::U8 => sample_format::U8,
        SampleFormat::I16 => sample_format::S16,
        SampleFormat::I32 => sample_format::S32,
        SampleFormat::F32 => sample_format::FLT,
        SampleFormat::F64 => sample_format::DBL,
        SampleFormat::U8p => sample_format::U8P,
        SampleFormat::I16p => sample_format::S16P,
        SampleFormat::I32p => sample_format::S32P,
        SampleFormat::F32p => sample_format::FLTP,
        SampleFormat::F64p => sample_format::DBLP,
        _ => {
            log::warn!(
                "sample_format has no AV mapping, falling back to FLTP \
                 format={format:?} fallback=FLTP"
            );
            sample_format::FLTP
        }
    }
}

// SAFETY: AudioEncoderInner owns all FFmpeg contexts exclusively.
//         These contexts are not accessed from multiple threads simultaneously;
//         all access is serialized by whichever thread holds the AudioEncoder.
//         Ownership transfer between threads is safe because FFmpeg contexts
//         are created and destroyed on the same thread (via std::thread::spawn).
unsafe impl Send for AudioEncoderInner {}

#[cfg(test)]
mod tests {
    use ff_format::SampleFormat;
    use ff_sys::swresample::sample_format;
    use ff_sys::{
        AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_FLAC, AVCodecID_AV_CODEC_ID_MP3,
        AVCodecID_AV_CODEC_ID_OPUS, AVCodecID_AV_CODEC_ID_PCM_S16LE, AVCodecID_AV_CODEC_ID_VORBIS,
    };

    use crate::AudioCodec;

    use super::{codec_to_id, sample_format_to_av};

    // -------------------------------------------------------------------------
    // codec_to_id
    // -------------------------------------------------------------------------

    #[test]
    fn codec_to_id_aac() {
        assert_eq!(codec_to_id(AudioCodec::Aac), AVCodecID_AV_CODEC_ID_AAC);
    }

    #[test]
    fn codec_to_id_opus() {
        assert_eq!(codec_to_id(AudioCodec::Opus), AVCodecID_AV_CODEC_ID_OPUS);
    }

    #[test]
    fn codec_to_id_mp3() {
        assert_eq!(codec_to_id(AudioCodec::Mp3), AVCodecID_AV_CODEC_ID_MP3);
    }

    #[test]
    fn codec_to_id_flac() {
        assert_eq!(codec_to_id(AudioCodec::Flac), AVCodecID_AV_CODEC_ID_FLAC);
    }

    #[test]
    fn codec_to_id_pcm() {
        assert_eq!(
            codec_to_id(AudioCodec::Pcm),
            AVCodecID_AV_CODEC_ID_PCM_S16LE
        );
    }

    #[test]
    fn codec_to_id_vorbis() {
        assert_eq!(
            codec_to_id(AudioCodec::Vorbis),
            AVCodecID_AV_CODEC_ID_VORBIS
        );
    }

    // -------------------------------------------------------------------------
    // sample_format_to_av
    // -------------------------------------------------------------------------

    #[test]
    fn sample_format_u8() {
        assert_eq!(sample_format_to_av(SampleFormat::U8), sample_format::U8);
    }

    #[test]
    fn sample_format_i16() {
        assert_eq!(sample_format_to_av(SampleFormat::I16), sample_format::S16);
    }

    #[test]
    fn sample_format_i32() {
        assert_eq!(sample_format_to_av(SampleFormat::I32), sample_format::S32);
    }

    #[test]
    fn sample_format_f32() {
        assert_eq!(sample_format_to_av(SampleFormat::F32), sample_format::FLT);
    }

    #[test]
    fn sample_format_f64() {
        assert_eq!(sample_format_to_av(SampleFormat::F64), sample_format::DBL);
    }

    #[test]
    fn sample_format_u8p() {
        assert_eq!(sample_format_to_av(SampleFormat::U8p), sample_format::U8P);
    }

    #[test]
    fn sample_format_i16p() {
        assert_eq!(sample_format_to_av(SampleFormat::I16p), sample_format::S16P);
    }

    #[test]
    fn sample_format_i32p() {
        assert_eq!(sample_format_to_av(SampleFormat::I32p), sample_format::S32P);
    }

    #[test]
    fn sample_format_f32p() {
        assert_eq!(sample_format_to_av(SampleFormat::F32p), sample_format::FLTP);
    }

    #[test]
    fn sample_format_f64p() {
        assert_eq!(sample_format_to_av(SampleFormat::F64p), sample_format::DBLP);
    }

    #[test]
    fn sample_format_unknown_falls_back_to_fltp() {
        assert_eq!(
            sample_format_to_av(SampleFormat::Other(99)),
            sample_format::FLTP
        );
    }
}
