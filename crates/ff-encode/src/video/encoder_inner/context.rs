//! Format context, stream, and subtitle initialization helpers.
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

use super::color::{
    color_primaries_to_av, color_space_to_av, color_transfer_to_av, from_av_pixel_format,
    pixel_format_to_av,
};
use super::options::audio_codec_to_id;
use super::two_pass::AV_CODEC_FLAG_PASS1;
use super::{
    AVMediaType_AVMEDIA_TYPE_SUBTITLE, AVPixelFormat_AV_PIX_FMT_YUV420P, AudioCodec, EncodeError,
    VideoCodec, VideoEncoderInner,
};

impl VideoEncoderInner {
    /// Set each container metadata entry before `avformat_write_header`.
    pub(super) fn apply_metadata(
        format_ctx: &mut ff_sys::OutputFormatContext,
        metadata: &[(String, String)],
    ) {
        for (key, value) in metadata {
            if let Err(e) = format_ctx.set_metadata(key, value) {
                log::warn!(
                    "metadata entry skipped key={key} error={}",
                    ff_sys::av_error_string(e.code())
                );
            }
        }
    }

    /// Apply `movflags` for fMP4 containers before `avformat_write_header`.
    ///
    /// When `container` is [`crate::OutputContainer::FMp4`], sets
    /// `movflags=+frag_keyframe+empty_moov+default_base_moof` via `av_opt_set`
    /// on the format context's `priv_data`. This enables CMAF-compatible
    /// fragmented output required for HLS fMP4 segments and MPEG-DASH.
    ///
    pub(super) fn apply_movflags(
        format_ctx: &mut ff_sys::OutputFormatContext,
        container: Option<crate::OutputContainer>,
    ) {
        if container.is_some_and(|c| c.is_fragmented())
            && let Err(e) =
                format_ctx.set_opt(c"movflags", c"+frag_keyframe+empty_moov+default_base_moof")
        {
            log::warn!(
                "av_opt_set movflags failed for fMP4 container error={}",
                ff_sys::av_error_string(e.code())
            );
        }
    }

    /// Set the container chapters before `avformat_write_header`.
    pub(super) fn apply_chapters(
        format_ctx: &mut ff_sys::OutputFormatContext,
        chapters: &[ff_format::chapter::ChapterInfo],
    ) {
        if chapters.is_empty() {
            return;
        }
        let specs: Vec<ff_sys::ChapterSpec> = chapters
            .iter()
            .map(|c| ff_sys::ChapterSpec {
                id: c.id(),
                start_us: c.start().as_micros() as i64,
                end_us: c.end().as_micros() as i64,
                title: c.title(),
            })
            .collect();
        if let Err(e) = format_ctx.set_chapters(&specs) {
            log::warn!(
                "set_chapters failed, skipping chapters error={}",
                ff_sys::av_error_string(e.code())
            );
        }
    }

    /// Initialize video encoder.
    ///
    /// When `two_pass` is `true` the codec context is opened with
    /// `AV_CODEC_FLAG_PASS1` and stored in `pass1_codec_ctx`; in single-pass
    /// mode it is stored in `video_codec_ctx` as usual.
    // The encoder parameters (dimensions, fps, codec, bitrate mode, options,
    // two-pass) map one-to-one onto FFmpeg's `AVCodecContext` fields; grouping
    // them into a struct would only shuffle the same values across the FFI call.
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn init_video_encoder(
        &mut self,
        width: u32,
        height: u32,
        fps: f64,
        codec: VideoCodec,
        bitrate_mode: Option<&crate::BitrateMode>,
        preset: &str,
        hardware_encoder: crate::HardwareEncoder,
        two_pass: bool,
        codec_options: Option<&crate::video::codec_options::VideoCodecOptions>,
        pixel_format: Option<&ff_format::PixelFormat>,
        color_space: Option<ff_format::ColorSpace>,
        color_transfer: Option<ff_format::ColorTransfer>,
        color_primaries: Option<ff_format::ColorPrimaries>,
    ) -> Result<(), EncodeError> {
        use crate::BitrateMode;
        // Select encoder based on codec and availability
        let encoder_name = self.select_video_encoder(codec, hardware_encoder)?;
        self.actual_video_codec.clone_from(&encoder_name);

        let selected_codec =
            ff_sys::Codec::find_encoder_by_name(&encoder_name).ok_or_else(|| {
                EncodeError::NoSuitableEncoder {
                    codec: format!("{:?}", codec),
                    tried: vec![encoder_name.clone()],
                }
            })?;
        let codec_ptr = selected_codec.as_ptr();

        // Allocate codec context
        let mut codec_ctx = ff_sys::CodecContext::new(Some(selected_codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // Configure codec context.
        // Use the encoder's own codec_id rather than codec_to_id(codec): when a
        // fallback encoder is selected (e.g. libvpx-vp9 instead of libx264 for
        // H.264), the codec_id must match the actual encoder, not the requested
        // codec family, otherwise avcodec_open2 rejects it with EINVAL.
        codec_ctx.set_codec_id((*codec_ptr).id);
        codec_ctx.set_width(width as i32);
        codec_ctx.set_height(height as i32);
        codec_ctx.set_time_base(ff_sys::AVRational {
            num: 1,
            den: (fps * 1000.0) as i32, // Use millisecond precision
        });
        codec_ctx.set_framerate(ff_sys::AVRational {
            num: fps as i32,
            den: 1,
        });
        codec_ctx.set_pix_fmt(AVPixelFormat_AV_PIX_FMT_YUV420P);

        // Set bitrate control mode
        match bitrate_mode {
            Some(BitrateMode::Cbr(bps)) => {
                codec_ctx.set_bit_rate(*bps as i64);
            }
            Some(BitrateMode::Vbr { target, max }) => {
                codec_ctx.set_bit_rate(*target as i64);
                codec_ctx.set_rc_max_rate(*max as i64);
                codec_ctx.set_rc_buffer_size((*max * 2) as i32);
            }
            Some(BitrateMode::Crf(q)) => {
                if codec_ctx.set_opt("crf", &q.to_string()).is_err() {
                    log::warn!(
                        "crf option not supported by encoder, falling back to default bitrate \
                         encoder={encoder_name} crf={q}"
                    );
                    codec_ctx.set_bit_rate(2_000_000);
                }
            }
            None => {
                // Default 2 Mbps
                codec_ctx.set_bit_rate(2_000_000);
            }
        }

        // Set preset for x264/x265
        if (encoder_name.contains("264") || encoder_name.contains("265"))
            && codec_ctx.set_opt("preset", preset).is_err()
        {
            log::warn!(
                "preset option not supported by encoder, ignoring \
                 encoder={encoder_name} preset={preset}"
            );
        }

        // Apply per-codec options before opening the codec context.
        if let Some(opts) = codec_options {
            // Options are applied before avcodec_open2 so they take effect during
            // codec initialisation.
            Self::apply_codec_options(&mut codec_ctx, opts, &encoder_name);
        }

        // Apply explicit pixel format override (takes priority over codec-option auto-select).
        if let Some(fmt) = pixel_format {
            codec_ctx.set_pix_fmt(pixel_format_to_av(*fmt));
        }

        // Apply HDR10 color context: BT.2020 primaries, PQ transfer, BT.2020 NCL colorspace.
        if self.hdr10_metadata.is_some() {
            codec_ctx.set_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020);
            codec_ctx.set_color_trc(ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_SMPTEST2084);
            codec_ctx.set_colorspace(ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL);
        }

        // Apply explicit color overrides (take priority over HDR10 automatic defaults).
        if let Some(cs) = color_space {
            codec_ctx.set_colorspace(color_space_to_av(cs));
        }
        if let Some(trc) = color_transfer {
            codec_ctx.set_color_trc(color_transfer_to_av(trc));
        }
        if let Some(cp) = color_primaries {
            codec_ctx.set_color_primaries(color_primaries_to_av(cp));
        }

        // For two-pass, set the pass-1 flag before opening the codec.
        if two_pass {
            codec_ctx.set_flags(codec_ctx.flags() | AV_CODEC_FLAG_PASS1);
        }

        // Open codec
        codec_ctx
            .open_codec(selected_codec)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
        let actual_pix_fmt = from_av_pixel_format(codec_ctx.pix_fmt());
        log::info!(
            "codec opened codec={encoder_name} width={width} height={height} fps={fps} \
             pix_fmt={actual_pix_fmt}"
        );

        // Create stream. The owned codec_ctx frees itself if this returns.
        let stream_idx = self
            .format_ctx
            .new_stream(Some(&selected_codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        self.format_ctx
            .set_stream_time_base(stream_idx, codec_ctx.time_base());

        // Copy all codec parameters (including extradata) to the stream.
        self.format_ctx
            .apply_stream_params_from_context(stream_idx, &codec_ctx)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        self.video_stream_index = stream_idx as i32;

        // In two-pass mode the pass-1 context is stored separately; the real
        // (pass-2) video_codec_ctx is initialised later in run_pass2().
        if two_pass {
            self.pass1_codec_ctx = Some(codec_ctx);
        } else {
            self.video_codec_ctx = Some(codec_ctx);
        }

        // Note: SwsContext initialization is deferred to convert_video_frame()
        // for better optimization (skip unnecessary conversions, reuse context)

        Ok(())
    }

    /// Initialize audio encoder.
    pub(super) unsafe fn init_audio_encoder(
        &mut self,
        sample_rate: u32,
        channels: u32,
        codec: AudioCodec,
        bitrate: Option<u64>,
    ) -> Result<(), EncodeError> {
        // Select encoder based on codec and availability
        let encoder_name = self.select_audio_encoder(codec)?;
        self.actual_audio_codec.clone_from(&encoder_name);

        let selected_codec =
            ff_sys::Codec::find_encoder_by_name(&encoder_name).ok_or_else(|| {
                EncodeError::NoSuitableEncoder {
                    codec: format!("{:?}", codec),
                    tried: vec![encoder_name.clone()],
                }
            })?;
        let codec_ptr = selected_codec.as_ptr();

        // Allocate codec context
        let mut codec_ctx = ff_sys::CodecContext::new(Some(selected_codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // Configure codec context
        codec_ctx.set_codec_id(audio_codec_to_id(codec));
        codec_ctx.set_sample_rate(sample_rate as i32);

        // Set channel layout using FFmpeg 7.x API
        codec_ctx.set_ch_layout_default(channels as i32);

        // Use the first sample format the codec actually declares; fall back to
        // FLTP only when the codec exposes no preference.  FLTP is NOT valid for
        // Opus (which requires s16 or flt), so we must not hard-code it.
        let target_fmt = {
            let fmts = (*codec_ptr).sample_fmts;
            if !fmts.is_null() && *fmts != ff_sys::swresample::sample_format::NONE {
                *fmts
            } else {
                ff_sys::swresample::sample_format::FLTP
            }
        };
        codec_ctx.set_sample_fmt(target_fmt);

        // Set bitrate
        if let Some(br) = bitrate {
            codec_ctx.set_bit_rate(br as i64);
        } else {
            // Default bitrate based on codec
            codec_ctx.set_bit_rate(match codec {
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
            den: sample_rate as i32,
        });

        // Open codec
        codec_ctx
            .open_codec(selected_codec)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // Create stream. The owned codec_ctx frees itself if this returns.
        let stream_idx = self
            .format_ctx
            .new_stream(Some(&selected_codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        self.format_ctx
            .set_stream_time_base(stream_idx, codec_ctx.time_base());

        // Copy all codec parameters to the stream — including extradata (e.g. AAC
        // AudioSpecificConfig) that is only available after avcodec_open2.
        // Using avcodec_parameters_from_context instead of manual field copies
        // ensures extradata, frame_size, channel layout, and codec_tag are all
        // propagated correctly so container muxers and hardware decoders can
        // identify and decode the stream.
        self.format_ctx
            .apply_stream_params_from_context(stream_idx, &codec_ctx)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // Read the FIFO parameters before moving the owned context into `self`.
        let frame_size = codec_ctx.frame_size();
        let fifo_sample_fmt = codec_ctx.sample_fmt();
        let fifo_nb_channels = codec_ctx.channels();

        self.audio_stream_index = stream_idx as i32;
        self.audio_codec_ctx = Some(codec_ctx);

        // Allocate sample FIFO for codecs that require a fixed frame_size (AAC, FLAC, ALAC …).
        // Leave audio_fifo as None for variable-frame-size codecs (frame_size == 0).
        if frame_size > 0 {
            let fifo = ff_sys::swresample::audio_fifo::alloc(
                fifo_sample_fmt,
                fifo_nb_channels,
                frame_size * 2,
            )
            .map_err(EncodeError::from_ffmpeg_error)?;
            self.audio_fifo = Some(fifo);
        }

        Ok(())
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

    /// Register binary attachment streams in the output container.
    ///
    /// Each attachment is stored as an `AVMEDIA_TYPE_ATTACHMENT` stream with
    /// `AV_CODEC_ID_BIN_DATA`. The attachment data is placed in `extradata`
    /// so the MKV muxer can write it into the container's `Attachments` element.
    ///
    /// Failures per entry are non-fatal: a warning is logged and the entry is
    /// skipped so the rest of encoding can continue.
    pub(super) fn init_attachments(&mut self, attachments: &[(Vec<u8>, String, String)]) {
        for (data, mime_type, filename) in attachments {
            match self
                .format_ctx
                .add_attachment_stream(data, mime_type, filename)
            {
                Ok(_) => log::info!(
                    "attachment: registered filename={filename} mime={mime_type} size={}",
                    data.len()
                ),
                Err(e) => log::warn!(
                    "attachment: registration failed, skipping filename={filename} error={}",
                    ff_sys::av_error_string(e.code())
                ),
            }
        }
    }

    /// Open the subtitle source file, find the requested stream, register an output subtitle
    /// stream with copied codec parameters, and close the source.
    ///
    /// Stores `(source_path, source_stream_index, output_stream_index)` in
    /// `self.subtitle_passthrough` on success. On any failure it logs a warning and returns
    /// without modifying state, so encoding can continue without subtitles.
    ///
    /// # Safety
    ///
    /// `self.format_ctx` must be a valid, non-null `AVFormatContext` pointer.
    /// Must be called before `avformat_write_header`.
    pub(super) unsafe fn init_subtitle_passthrough(
        &mut self,
        source_path: &str,
        source_stream_index: usize,
    ) {
        let path = std::path::Path::new(source_path);
        // The owned demux context frees itself (closing the input) on every early
        // return below and at scope end, so no manual teardown is needed.
        let mut src_ctx = match ff_sys::InputFormatContext::open(path) {
            Ok(ctx) => ctx,
            Err(e) => {
                log::warn!(
                    "subtitle_passthrough: failed to open source file \
                     path={source_path} error={}",
                    ff_sys::av_error_string(e.code())
                );
                return;
            }
        };

        if let Err(e) = src_ctx.find_stream_info() {
            log::warn!(
                "subtitle_passthrough: failed to find stream info \
                 path={source_path} error={}",
                ff_sys::av_error_string(e.code())
            );
            return;
        }

        let nb_streams = src_ctx.nb_streams() as usize;
        if source_stream_index >= nb_streams {
            log::warn!(
                "subtitle_passthrough: stream index out of range \
                 index={source_stream_index} nb_streams={nb_streams}"
            );
            return;
        }

        let Some(in_stream) = src_ctx.stream(source_stream_index) else {
            return;
        };

        if in_stream.codecpar().codec_type() != AVMediaType_AVMEDIA_TYPE_SUBTITLE {
            log::warn!(
                "subtitle_passthrough: stream at index {source_stream_index} \
                 is not a subtitle stream"
            );
            return;
        }

        // Record the output stream index before adding the new stream.
        let out_stream_index = self.format_ctx.nb_streams() as i32;
        // A null codec means the muxer selects a default for the copied stream.
        let Ok(new_idx) = self.format_ctx.new_stream(None) else {
            log::warn!("subtitle_passthrough: avformat_new_stream failed");
            return;
        };

        // Copy the source codec parameters into the new stream (also clears
        // codec_tag so the muxer picks the container's value).
        if let Err(e) = self
            .format_ctx
            .copy_stream_params(new_idx, in_stream.codecpar())
        {
            log::warn!(
                "subtitle_passthrough: avcodec_parameters_copy failed error={}",
                ff_sys::av_error_string(e.code())
            );
            return;
        }

        self.subtitle_passthrough = Some((
            source_path.to_string(),
            source_stream_index,
            out_stream_index,
        ));
        log::info!(
            "subtitle_passthrough: registered subtitle stream \
             source={source_path} stream_index={source_stream_index} \
             out_stream_index={out_stream_index}"
        );
    }

    /// Re-open the subtitle source file, read all packets from the registered subtitle stream,
    /// rescale their timestamps, and write them to the output.
    ///
    /// No-op if `self.subtitle_passthrough` is `None`.  On non-fatal errors (open failure,
    /// read errors) it logs a warning and returns `Ok(())` so the caller can still write the
    /// trailer.
    ///
    /// # Safety
    ///
    /// `self.format_ctx` must be valid. Must be called before `av_write_trailer`.
    pub(super) unsafe fn write_subtitle_packets(&mut self) -> Result<(), EncodeError> {
        let Some((source_path, source_stream_index, out_stream_index)) =
            self.subtitle_passthrough.clone()
        else {
            return Ok(());
        };

        let path = std::path::Path::new(&source_path);
        // The owned demux context frees itself (closing the input) on every early
        // return below and at scope end, so no manual teardown is needed.
        let mut src_ctx = match ff_sys::InputFormatContext::open(path) {
            Ok(ctx) => ctx,
            Err(e) => {
                log::warn!(
                    "subtitle_passthrough: failed to re-open source file \
                     path={source_path} error={}",
                    ff_sys::av_error_string(e.code())
                );
                return Ok(());
            }
        };

        if let Err(e) = src_ctx.find_stream_info() {
            log::warn!(
                "subtitle_passthrough: failed to find stream info on re-open \
                 path={source_path} error={}",
                ff_sys::av_error_string(e.code())
            );
            return Ok(());
        }

        // source_stream_index was validated in init_subtitle_passthrough.
        let Some(in_time_base) = src_ctx.stream(source_stream_index).map(|s| s.time_base()) else {
            return Ok(());
        };

        // out_stream_index was set by new_stream when the output stream was added.
        let out_time_base = self.format_ctx.stream_time_base(out_stream_index as usize);

        let Ok(mut pkt) = ff_sys::Packet::new() else {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "subtitle_passthrough: av_packet_alloc failed".to_string(),
            });
        };

        loop {
            match src_ctx.read_frame(&mut pkt) {
                Err(e) if e.is_eof() => break,
                Err(e) => {
                    log::warn!(
                        "subtitle_passthrough: read_frame error, stopping \
                         path={source_path} error={}",
                        ff_sys::av_error_string(e.code())
                    );
                    break;
                }
                Ok(()) => {}
            }

            // Skip packets from other streams.
            if pkt.stream_index() != source_stream_index as i32 {
                pkt.unref();
                continue;
            }

            // Rescale timestamps from the source stream's time base to the output stream's.
            pkt.rescale_ts(in_time_base, out_time_base);
            pkt.set_stream_index(out_stream_index);

            // SRT/subtitle packets typically carry only PTS (DTS is AV_NOPTS_VALUE).
            // The matroska muxer requires a valid DTS for av_interleaved_write_frame;
            // mirror PTS → DTS when DTS is absent so packets are not silently dropped.
            if pkt.dts() == i64::MIN {
                pkt.set_dts(pkt.pts());
            }

            if let Err(e) = self.format_ctx.write_interleaved(&mut pkt) {
                log::warn!(
                    "subtitle_passthrough: av_interleaved_write_frame failed \
                     error={}",
                    ff_sys::av_error_string(e.code())
                );
            }
            pkt.unref();
        }

        // `pkt` and the owned `src_ctx` drop at end of scope, freeing them.
        Ok(())
    }

    /// Cleanup FFmpeg resources.
    pub(super) unsafe fn cleanup(&mut self) {
        // Free video codec context. For two-pass encoding the context may still
        // reference a Rust-owned `stats_in` buffer; `CodecContext::Drop` nulls the
        // field before `avcodec_free_context`, so dropping the owned context here
        // is sufficient.
        self.video_codec_ctx = None;

        // Free pass-1 codec context (only set in two-pass mode); drops on assignment.
        self.pass1_codec_ctx = None;

        // Free audio codec context; drops on assignment.
        self.audio_codec_ctx = None;

        // Free scaling context (owned ScaleContext drops on assignment).
        self.sws_ctx = None;

        // Free resampling context (owned ResampleContext drops on assignment).
        self.swr_ctx = None;

        // Free audio FIFO
        if let Some(fifo) = self.audio_fifo.take() {
            ff_sys::swresample::audio_fifo::free(fifo);
        }

        // The owned `format_ctx` closes its IO and frees itself when the struct
        // drops; nothing to close here.
    }
}
