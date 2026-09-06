//! Global FFmpeg constants.

/// Invalid PTS value (no presentation timestamp).
///
/// This constant indicates that a frame or packet does not have a valid
/// presentation timestamp.
pub const AV_NOPTS_VALUE: i64 = i64::MIN;

/// `AVFMT_TS_DISCONT` — `AVInputFormat` flag indicating discontinuous timestamps.
///
/// Set for live/streaming formats such as HLS (live playlists), RTMP, RTSP,
/// MPEG-TS (UDP/SRT), and similar sources. Used to detect live streams after
/// `avformat_find_stream_info` has been called.
pub const AVFMT_TS_DISCONT: i32 = 0x0200;

/// `AVFMT_NOFILE` — `AVOutputFormat` flag indicating the muxer manages its own
/// I/O and has no `AVIOContext` (`pb`) opened by the caller.
///
/// Set for muxers such as `image2` (image sequences), `hls`, `dash`, and similar
/// segment/directory formats. When set, the caller must not `avio_open` a `pb`,
/// and the owned [`OutputFormatContext`](crate::OutputFormatContext) must not
/// close one on drop.
pub const AVFMT_NOFILE: i32 = 0x0001;

/// `AVFMT_FLAG_CUSTOM_IO`: the caller supplied the context's `pb`, so libavformat
/// must not `avio_close()` it.
///
/// Hand-defined because bindgen's `AV_.*` variable allowlist does not match
/// `AVFMT_`, the same reason [`AVFMT_NOFILE`] is defined here. The value is from
/// `libavformat/avformat.h`.
pub const AVFMT_FLAG_CUSTOM_IO: i32 = 0x0080;

/// `AV_BUFFERSRC_FLAG_KEEP_REF` normalized to `i32` for cross-platform use.
///
/// bindgen generates `AV_BUFFERSRC_FLAG_KEEP_REF` as `u32` on Linux/macOS
/// (pkg-config / Homebrew) but as `i32` on Windows (VCPKG). Use this constant
/// instead of the raw bindgen symbol when calling `av_buffersrc_add_frame_flags`.
///
/// The cfg flag `ffmpeg_buffersrc_flag_u32` is emitted by the build script on
/// platforms where the bindgen type is `u32` (Linux, macOS).
#[cfg(ffmpeg_buffersrc_flag_u32)]
pub const BUFFERSRC_FLAG_KEEP_REF: i32 = crate::AV_BUFFERSRC_FLAG_KEEP_REF as i32;
#[cfg(not(ffmpeg_buffersrc_flag_u32))]
pub const BUFFERSRC_FLAG_KEEP_REF: i32 = crate::AV_BUFFERSRC_FLAG_KEEP_REF;
