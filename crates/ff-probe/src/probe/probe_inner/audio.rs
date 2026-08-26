//! Audio stream extraction.

use ff_format::channel::ChannelLayout;
use ff_format::stream::AudioStreamInfo;

use super::mapping::{map_audio_codec, map_sample_format};
use super::video::{extract_codec_name, extract_stream_bitrate, extract_stream_duration};

/// Extracts all audio streams from the demux context.
///
/// Iterates every stream and extracts detailed information for each audio stream.
pub(super) fn extract_audio_streams(ctx: &ff_sys::InputFormatContext) -> Vec<AudioStreamInfo> {
    ctx.streams()
        .filter(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_AUDIO)
        .map(extract_single_audio_stream)
        .collect()
}

/// Extracts information from a single audio stream handle.
fn extract_single_audio_stream(stream: ff_sys::StreamRef<'_>) -> AudioStreamInfo {
    let codecpar = stream.codecpar();

    // Extract codec info
    let codec_id = codecpar.codec_id();
    let codec = map_audio_codec(codec_id);
    let codec_name = extract_codec_name(codec_id);

    // Extract audio parameters
    #[expect(clippy::cast_sign_loss, reason = "sample_rate is always positive")]
    let sample_rate = codecpar.sample_rate() as u32;

    // FFmpeg 5.1+ uses ch_layout, older versions use channels
    let channels = extract_channel_count(codecpar);

    // Extract channel layout
    let channel_layout = extract_channel_layout(codecpar, channels);

    // Extract sample format
    let sample_format = map_sample_format(codecpar.format());

    // Extract bitrate
    let bitrate = extract_stream_bitrate(codecpar);

    // Extract duration if available
    let duration = extract_stream_duration(stream);

    // Extract language from stream metadata
    let language = extract_language(stream);

    // Build the AudioStreamInfo
    #[expect(clippy::cast_sign_loss, reason = "stream index is always non-negative")]
    let index = stream.index() as u32;
    let mut builder = AudioStreamInfo::builder()
        .index(index)
        .codec(codec)
        .codec_name(codec_name)
        .sample_rate(sample_rate)
        .channels(channels)
        .channel_layout(channel_layout)
        .sample_format(sample_format);

    if let Some(d) = duration {
        builder = builder.duration(d);
    }

    if let Some(b) = bitrate {
        builder = builder.bitrate(b);
    }

    if let Some(lang) = language {
        builder = builder.language(lang);
    }

    builder.build()
}

/// Extracts the channel count from codec parameters.
///
/// `FFmpeg` 5.1+ uses `ch_layout.nb_channels`. Returns the actual channel count;
/// if it is 0 (uninitialized or unknown), returns 1 (mono) as a safe minimum.
fn extract_channel_count(codecpar: ff_sys::CodecParameters<'_>) -> u32 {
    #[expect(clippy::cast_sign_loss, reason = "channel count is always positive")]
    let channels = codecpar.ch_layout().nb_channels as u32;

    // If channel count is 0 (uninitialized/unknown), use 1 (mono) as safe minimum
    if channels > 0 {
        channels
    } else {
        log::warn!(
            "channel_count is 0 (uninitialized), falling back to mono \
             fallback=1"
        );
        1
    }
}

/// Extracts the channel layout from codec parameters.
fn extract_channel_layout(codecpar: ff_sys::CodecParameters<'_>, channels: u32) -> ChannelLayout {
    // FFmpeg 5.1+ uses ch_layout structure with channel masks. `ch_layout()`
    // copies the POD layout out of the codec parameters.
    let ch_layout = codecpar.ch_layout();

    // Check if we have a specific channel layout mask
    // AV_CHANNEL_ORDER_NATIVE means we have a valid channel mask
    if ch_layout.order == ff_sys::AVChannelOrder_AV_CHANNEL_ORDER_NATIVE {
        // Map common FFmpeg channel masks to our ChannelLayout
        // These are AVChannelLayout masks for standard configurations
        // SAFETY: When order is AV_CHANNEL_ORDER_NATIVE, the `mask` union field is valid.
        let mask = unsafe { ch_layout.u.mask };
        match mask {
            // AV_CH_LAYOUT_MONO = 0x4 (front center)
            0x4 => ChannelLayout::Mono,
            // AV_CH_LAYOUT_STEREO = 0x3 (front left + front right)
            0x3 => ChannelLayout::Stereo,
            // AV_CH_LAYOUT_2_1 = 0x103 (stereo + LFE)
            0x103 => ChannelLayout::Stereo2_1,
            // AV_CH_LAYOUT_SURROUND = 0x7 (FL + FR + FC)
            0x7 => ChannelLayout::Surround3_0,
            // AV_CH_LAYOUT_QUAD = 0x33 (FL + FR + BL + BR)
            0x33 => ChannelLayout::Quad,
            // AV_CH_LAYOUT_5POINT0 = 0x37 (FL + FR + FC + BL + BR)
            0x37 => ChannelLayout::Surround5_0,
            // AV_CH_LAYOUT_5POINT1 = 0x3F (FL + FR + FC + LFE + BL + BR)
            0x3F => ChannelLayout::Surround5_1,
            // AV_CH_LAYOUT_6POINT1 = 0x13F (FL + FR + FC + LFE + BC + SL + SR)
            0x13F => ChannelLayout::Surround6_1,
            // AV_CH_LAYOUT_7POINT1 = 0x63F (FL + FR + FC + LFE + BL + BR + SL + SR)
            0x63F => ChannelLayout::Surround7_1,
            _ => {
                log::warn!(
                    "channel_layout mask has no mapping, deriving from channel count \
                     mask={mask} channels={channels}"
                );
                ChannelLayout::from_channels(channels)
            }
        }
    } else {
        log::warn!(
            "channel_layout order is not NATIVE, deriving from channel count \
             order={order} channels={channels}",
            order = ch_layout.order
        );
        ChannelLayout::from_channels(channels)
    }
}

/// Extracts the `language` tag from a stream's metadata (`None` when absent).
pub(super) fn extract_language(stream: ff_sys::StreamRef<'_>) -> Option<String> {
    stream.metadata().remove("language")
}
