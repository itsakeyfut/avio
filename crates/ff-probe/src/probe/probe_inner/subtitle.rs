//! Subtitle stream extraction.

use ff_format::stream::SubtitleStreamInfo;

use super::audio::extract_language;
use super::mapping::map_subtitle_codec;
use super::video::{extract_codec_name, extract_stream_duration};

/// Extracts all subtitle streams from the demux context.
///
/// Iterates every stream and extracts detailed information for each subtitle
/// stream.
pub(super) fn extract_subtitle_streams(
    ctx: &ff_sys::InputFormatContext,
) -> Vec<SubtitleStreamInfo> {
    ctx.streams()
        .filter(|s| s.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_SUBTITLE)
        .map(extract_single_subtitle_stream)
        .collect()
}

/// Extracts information from a single subtitle stream handle.
fn extract_single_subtitle_stream(stream: ff_sys::StreamRef<'_>) -> SubtitleStreamInfo {
    let codecpar = stream.codecpar();

    let codec_id = codecpar.codec_id();
    let codec = map_subtitle_codec(codec_id);
    let codec_name = extract_codec_name(codec_id);

    // disposition is a c_int bitmask; cast to u32 for bitwise AND with the u32 constant
    #[expect(
        clippy::cast_sign_loss,
        reason = "disposition is a non-negative bitmask"
    )]
    let forced = (stream.disposition() as u32 & ff_sys::AV_DISPOSITION_FORCED) != 0;

    let duration = extract_stream_duration(stream);
    let language = extract_language(stream);
    let title = extract_stream_title(stream);

    #[expect(clippy::cast_sign_loss, reason = "stream index is always non-negative")]
    let index = stream.index() as u32;
    let mut builder = SubtitleStreamInfo::builder()
        .index(index)
        .codec(codec)
        .codec_name(codec_name)
        .forced(forced);

    if let Some(d) = duration {
        builder = builder.duration(d);
    }
    if let Some(lang) = language {
        builder = builder.language(lang);
    }
    if let Some(t) = title {
        builder = builder.title(t);
    }

    builder.build()
}

/// Extracts the `title` tag from a stream's metadata (`None` when absent).
pub(super) fn extract_stream_title(stream: ff_sys::StreamRef<'_>) -> Option<String> {
    stream.metadata().remove("title")
}
