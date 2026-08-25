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
    AV_TIME_BASE, AVChapter, AVMediaType_AVMEDIA_TYPE_SUBTITLE, AVPixelFormat_AV_PIX_FMT_YUV420P,
    AudioCodec, EncodeError, VideoCodec, VideoEncoderInner, av_interleaved_write_frame, av_mallocz,
    avformat_new_stream, ptr,
};

impl VideoEncoderInner {
    /// Call `av_dict_set` for each metadata entry before `avformat_write_header`.
    ///
    /// # Safety
    /// `format_ctx` must be a valid non-null pointer to an allocated `AVFormatContext`.
    /// Must be called before `avformat_write_header`.
    pub(super) unsafe fn apply_metadata(
        format_ctx: *mut ff_sys::AVFormatContext,
        metadata: &[(String, String)],
    ) {
        for (key, value) in metadata {
            let Ok(c_key) = std::ffi::CString::new(key.as_str()) else {
                log::warn!("metadata key contains null byte, skipping key={key}");
                continue;
            };
            let Ok(c_value) = std::ffi::CString::new(value.as_str()) else {
                log::warn!("metadata value contains null byte, skipping key={key}");
                continue;
            };
            // SAFETY: format_ctx is valid and non-null. c_key/c_value are valid
            // CStrings covering this call. av_dict_set copies both strings.
            let ret = ff_sys::av_dict_set(
                &raw mut (*format_ctx).metadata,
                c_key.as_ptr(),
                c_value.as_ptr(),
                0,
            );
            if ret < 0 {
                log::warn!(
                    "av_dict_set failed for metadata entry, skipping \
                     key={key} error={}",
                    ff_sys::av_error_string(ret)
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
    /// # Safety
    /// `format_ctx` must be a valid non-null pointer to an allocated `AVFormatContext`
    /// whose `priv_data` is non-null. Must be called before `avformat_write_header`.
    pub(super) unsafe fn apply_movflags(
        format_ctx: *mut ff_sys::AVFormatContext,
        container: Option<crate::OutputContainer>,
    ) {
        if container.is_some_and(|c| c.is_fragmented()) {
            // SAFETY: format_ctx and priv_data are non-null; string literals are
            // static and NUL-terminated. av_opt_set does not retain the pointers.
            let ret = ff_sys::av_opt_set(
                (*format_ctx).priv_data,
                c"movflags".as_ptr(),
                c"+frag_keyframe+empty_moov+default_base_moof".as_ptr(),
                0,
            );
            if ret < 0 {
                log::warn!(
                    "av_opt_set movflags failed for fMP4 container error={}",
                    ff_sys::av_error_string(ret)
                );
            }
        }
    }

    /// Allocate `AVChapter` entries on the format context before `avformat_write_header`.
    ///
    /// # Safety
    /// `format_ctx` must be a valid non-null pointer to an allocated `AVFormatContext`.
    /// Must be called before `avformat_write_header`.
    pub(super) unsafe fn apply_chapters(
        format_ctx: *mut ff_sys::AVFormatContext,
        chapters: &[ff_format::chapter::ChapterInfo],
    ) {
        if chapters.is_empty() {
            return;
        }
        let n = chapters.len();
        // SAFETY: allocating an array of n pointers for the chapters field.
        let chapters_arr =
            av_mallocz(std::mem::size_of::<*mut AVChapter>() * n) as *mut *mut AVChapter;
        if chapters_arr.is_null() {
            log::warn!("av_mallocz failed for chapters array, skipping chapters");
            return;
        }
        (*format_ctx).chapters = chapters_arr;
        (*format_ctx).nb_chapters = 0;

        for (i, chapter) in chapters.iter().enumerate() {
            // SAFETY: allocating a zeroed AVChapter struct.
            let chap = av_mallocz(std::mem::size_of::<AVChapter>()) as *mut AVChapter;
            if chap.is_null() {
                log::warn!(
                    "av_mallocz failed for AVChapter, skipping chapter id={}",
                    chapter.id()
                );
                continue;
            }
            // SAFETY: chap is freshly allocated, non-null, and zeroed.
            (*chap).id = chapter.id();
            (*chap).time_base = ff_sys::AVRational {
                num: 1,
                den: AV_TIME_BASE as i32,
            };
            (*chap).start = chapter.start().as_micros() as i64;
            (*chap).end = chapter.end().as_micros() as i64;
            (*chap).metadata = std::ptr::null_mut();

            if let Some(title) = chapter.title() {
                let Ok(c_title) = std::ffi::CString::new(title) else {
                    log::warn!(
                        "chapter title contains null byte, skipping title id={}",
                        chapter.id()
                    );
                    // SAFETY: chapters_arr is valid with capacity n.
                    *chapters_arr.add(i) = chap;
                    (*format_ctx).nb_chapters += 1;
                    continue;
                };
                // SAFETY: chap->metadata is null; av_dict_set allocates and copies.
                let ret = ff_sys::av_dict_set(
                    &raw mut (*chap).metadata,
                    b"title\0".as_ptr() as *const _,
                    c_title.as_ptr(),
                    0,
                );
                if ret < 0 {
                    log::warn!(
                        "av_dict_set failed for chapter title, skipping title \
                         id={} error={}",
                        chapter.id(),
                        ff_sys::av_error_string(ret)
                    );
                }
            }
            // SAFETY: i < n so the write is in bounds.
            *chapters_arr.add(i) = chap;
            (*format_ctx).nb_chapters += 1;
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
            .open(selected_codec, ptr::null_mut())
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
        let actual_pix_fmt = from_av_pixel_format(codec_ctx.pix_fmt());
        log::info!(
            "codec opened codec={encoder_name} width={width} height={height} fps={fps} \
             pix_fmt={actual_pix_fmt}"
        );

        // Create stream
        let stream = avformat_new_stream(self.format_ctx.as_mut_ptr(), codec_ptr);
        if stream.is_null() {
            // The owned codec_ctx frees itself on return.
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot create stream".to_string(),
            });
        }

        (*stream).time_base = codec_ctx.time_base();

        // Copy codec parameters to stream
        if !(*stream).codecpar.is_null() {
            (*(*stream).codecpar).codec_id = codec_ctx.codec_id();
            (*(*stream).codecpar).codec_type = ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO;
            (*(*stream).codecpar).width = codec_ctx.width();
            (*(*stream).codecpar).height = codec_ctx.height();
            (*(*stream).codecpar).format = codec_ctx.pix_fmt();
        }

        self.video_stream_index = (self.format_ctx.nb_streams() - 1) as i32;

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
            .open(selected_codec, ptr::null_mut())
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // Create stream
        let stream = avformat_new_stream(self.format_ctx.as_mut_ptr(), codec_ptr);
        if stream.is_null() {
            // The owned codec_ctx frees itself on return.
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot create stream".to_string(),
            });
        }

        (*stream).time_base = codec_ctx.time_base();

        // Copy all codec parameters to the stream — including extradata (e.g. AAC
        // AudioSpecificConfig) that is only available after avcodec_open2.
        // Using avcodec_parameters_from_context instead of manual field copies
        // ensures extradata, frame_size, channel layout, and codec_tag are all
        // propagated correctly so container muxers and hardware decoders can
        // identify and decode the stream.
        if !(*stream).codecpar.is_null() {
            codec_ctx
                .parameters_from_context((*stream).codecpar)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
        }

        // Read the FIFO parameters before moving the owned context into `self`.
        let frame_size = codec_ctx.frame_size();
        let fifo_sample_fmt = codec_ctx.sample_fmt();
        let fifo_nb_channels = codec_ctx.channels();

        self.audio_stream_index = (self.format_ctx.nb_streams() - 1) as i32;
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
    ///
    /// # Safety
    ///
    /// `self.format_ctx` must be a valid, non-null `AVFormatContext` pointer.
    /// Must be called before `avformat_write_header`.
    pub(super) unsafe fn init_attachments(&mut self, attachments: &[(Vec<u8>, String, String)]) {
        for (data, mime_type, filename) in attachments {
            // Create a new stream for the attachment.
            // SAFETY: format_ctx is valid; null codec means the muxer selects a default.
            let out_stream = avformat_new_stream(self.format_ctx.as_mut_ptr(), std::ptr::null());
            if out_stream.is_null() {
                log::warn!("attachment: avformat_new_stream failed, skipping filename={filename}");
                continue;
            }

            let codecpar = (*out_stream).codecpar;
            (*codecpar).codec_type = ff_sys::AVMediaType_AVMEDIA_TYPE_ATTACHMENT;
            (*codecpar).codec_id = ff_sys::AVCodecID_AV_CODEC_ID_BIN_DATA;

            // Allocate extradata with FFmpeg's allocator so it can be freed by
            // avcodec_parameters_free. The padding bytes are zeroed by av_mallocz.
            let alloc_size = data.len() + ff_sys::AV_INPUT_BUFFER_PADDING_SIZE as usize;
            let extradata = ff_sys::av_mallocz(alloc_size) as *mut u8;
            if extradata.is_null() {
                log::warn!(
                    "attachment: av_mallocz failed for extradata, skipping filename={filename}"
                );
                continue;
            }
            // SAFETY: extradata has at least `data.len()` bytes; data slice is valid.
            std::ptr::copy_nonoverlapping(data.as_ptr(), extradata, data.len());
            (*codecpar).extradata = extradata;
            (*codecpar).extradata_size = data.len() as i32;

            // Set stream metadata so the muxer records the filename and MIME type.
            let Ok(c_filename) = std::ffi::CString::new(filename.as_str()) else {
                log::warn!("attachment: filename contains null byte, skipping filename={filename}");
                continue;
            };
            let Ok(c_mime) = std::ffi::CString::new(mime_type.as_str()) else {
                log::warn!(
                    "attachment: mime_type contains null byte, skipping filename={filename}"
                );
                continue;
            };
            // SAFETY: out_stream->metadata pointer is valid (initialized by avformat_new_stream).
            ff_sys::av_dict_set(
                &raw mut (*out_stream).metadata,
                b"filename\0".as_ptr() as *const i8,
                c_filename.as_ptr(),
                0,
            );
            ff_sys::av_dict_set(
                &raw mut (*out_stream).metadata,
                b"mimetype\0".as_ptr() as *const i8,
                c_mime.as_ptr(),
                0,
            );

            log::info!(
                "attachment: registered filename={filename} mime={mime_type} size={}",
                data.len()
            );
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

        // SAFETY: source_stream_index < nb_streams; streams is a valid array.
        let in_stream = *(*src_ctx.as_ptr()).streams.add(source_stream_index);

        if (*(*in_stream).codecpar).codec_type != AVMediaType_AVMEDIA_TYPE_SUBTITLE {
            log::warn!(
                "subtitle_passthrough: stream at index {source_stream_index} \
                 is not a subtitle stream"
            );
            return;
        }

        // Record the output stream index before adding the new stream.
        let out_stream_index = self.format_ctx.nb_streams() as i32;
        // SAFETY: format_ctx is valid; null codec means the muxer selects a default.
        let out_stream = avformat_new_stream(self.format_ctx.as_mut_ptr(), std::ptr::null());
        if out_stream.is_null() {
            log::warn!("subtitle_passthrough: avformat_new_stream failed");
            return;
        }

        // SAFETY: out_stream and in_stream->codecpar are valid non-null pointers.
        let ret = ff_sys::avcodec_parameters_copy((*out_stream).codecpar, (*in_stream).codecpar);
        if ret < 0 {
            log::warn!(
                "subtitle_passthrough: avcodec_parameters_copy failed error={}",
                ff_sys::av_error_string(ret)
            );
            return;
        }

        // Reset codec_tag so the muxer can pick the appropriate value for the container.
        (*(*out_stream).codecpar).codec_tag = 0;

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

        // SAFETY: source_stream_index was validated in init_subtitle_passthrough.
        let in_stream = *(*src_ctx.as_ptr()).streams.add(source_stream_index);
        let in_time_base = (*in_stream).time_base;

        // SAFETY: out_stream_index was set by avformat_new_stream; format_ctx is valid.
        let out_stream = *(*self.format_ctx.as_mut_ptr())
            .streams
            .add(out_stream_index as usize);
        let out_time_base = (*out_stream).time_base;

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

            // This is a demux-input packet; its fields have no typed accessor and
            // are read/written through the packet pointer.
            let p = pkt.as_mut_ptr();

            // Skip packets from other streams.
            if (*p).stream_index != source_stream_index as i32 {
                pkt.unref();
                continue;
            }

            // Rescale timestamps from the source stream's time base to the output stream's.
            // SAFETY: pkt is valid; time bases are plain value types.
            ff_sys::av_packet_rescale_ts(p, in_time_base, out_time_base);
            (*p).stream_index = out_stream_index;

            // SRT/subtitle packets typically carry only PTS (DTS is AV_NOPTS_VALUE).
            // The matroska muxer requires a valid DTS for av_interleaved_write_frame;
            // mirror PTS → DTS when DTS is absent so packets are not silently dropped.
            if (*p).dts == i64::MIN {
                (*p).dts = (*p).pts;
            }

            let write_ret = av_interleaved_write_frame(self.format_ctx.as_mut_ptr(), p);
            if write_ret < 0 {
                log::warn!(
                    "subtitle_passthrough: av_interleaved_write_frame failed \
                     error={}",
                    ff_sys::av_error_string(write_ret)
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
