use super::{
    AVCodecID, AVMediaType_AVMEDIA_TYPE_VIDEO, CStr, ContainerInfo, DecodeError, Duration,
    InputFormatContext, Rational, VideoDecoderInner, VideoStreamInfo,
};

impl VideoDecoderInner {
    /// Finds the first video stream in the format context.
    ///
    /// Returns `Some((index, codec_id))` if a video stream is found, `None` otherwise.
    pub(super) fn find_video_stream(format_ctx: &InputFormatContext) -> Option<(usize, AVCodecID)> {
        for stream in format_ctx.streams() {
            let codecpar = stream.codecpar();
            if codecpar.codec_type() == AVMediaType_AVMEDIA_TYPE_VIDEO {
                return Some((stream.index() as usize, codecpar.codec_id()));
            }
        }
        None
    }

    /// Returns the human-readable codec name for a given `AVCodecID`.
    pub(super) unsafe fn extract_codec_name(codec_id: ff_sys::AVCodecID) -> String {
        // SAFETY: avcodec_get_name is safe for any codec ID value
        let name_ptr = unsafe { ff_sys::avcodec_get_name(codec_id) };
        if name_ptr.is_null() {
            return String::from("unknown");
        }
        // SAFETY: avcodec_get_name returns a valid C string with static lifetime
        unsafe { CStr::from_ptr(name_ptr).to_string_lossy().into_owned() }
    }

    /// Extracts video stream information from the borrowed stream and codec
    /// context.
    pub(super) fn extract_stream_info(
        stream: ff_sys::StreamRef<'_>,
        codec_ctx: &ff_sys::CodecContext,
        duration_val: i64,
    ) -> Result<VideoStreamInfo, DecodeError> {
        let codecpar = stream.codecpar();
        let stream_index = stream.index();
        let width = codecpar.width() as u32;
        let height = codecpar.height() as u32;
        let fps_rational = stream.avg_frame_rate();
        let pix_fmt = codec_ctx.pix_fmt();
        let color_space_val = codecpar.color_space();
        let color_range_val = codecpar.color_range();
        let color_primaries_val = codecpar.color_primaries();
        let codec_id = codecpar.codec_id();

        // Extract frame rate
        let frame_rate = if fps_rational.den != 0 {
            Rational::new(fps_rational.num as i32, fps_rational.den as i32)
        } else {
            log::warn!(
                "invalid frame rate, falling back to 30fps num={} den=0 fallback=30/1",
                fps_rational.num
            );
            Rational::new(30, 1)
        };

        // Extract duration
        let duration = if duration_val > 0 {
            let duration_secs = duration_val as f64 / 1_000_000.0;
            Some(Duration::from_secs_f64(duration_secs))
        } else {
            None
        };

        // Extract pixel format
        let pixel_format = Self::convert_pixel_format(pix_fmt);

        // Extract color information
        let color_space = Self::convert_color_space(color_space_val);
        let color_range = Self::convert_color_range(color_range_val);
        let color_primaries = Self::convert_color_primaries(color_primaries_val);

        // Extract codec
        let codec = Self::convert_codec(codec_id);
        let codec_name = unsafe { Self::extract_codec_name(codec_id) };

        // Build stream info
        let mut builder = VideoStreamInfo::builder()
            .index(stream_index as u32)
            .codec(codec)
            .codec_name(codec_name)
            .width(width)
            .height(height)
            .frame_rate(frame_rate)
            .pixel_format(pixel_format)
            .color_space(color_space)
            .color_range(color_range)
            .color_primaries(color_primaries);

        if let Some(d) = duration {
            builder = builder.duration(d);
        }

        Ok(builder.build())
    }

    /// Extracts container-level information from the format context.
    pub(super) fn extract_container_info(format_ctx: &InputFormatContext) -> ContainerInfo {
        let format_name = format_ctx.iformat_name().unwrap_or_default();

        let bit_rate = {
            let br = format_ctx.bit_rate();
            if br > 0 { Some(br as u64) } else { None }
        };

        let nb_streams = format_ctx.nb_streams();

        let mut builder = ContainerInfo::builder()
            .format_name(format_name)
            .nb_streams(nb_streams);
        if let Some(br) = bit_rate {
            builder = builder.bit_rate(br);
        }
        builder.build()
    }
}
