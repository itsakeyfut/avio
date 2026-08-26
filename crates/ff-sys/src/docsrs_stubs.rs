// Shape-compatible stubs used by docs.rs builds (DOCS_RS=1).
//
// These definitions mirror the real bindgen-generated FFmpeg bindings in type
// and name, but contain no actual FFmpeg code.  All functions are no-op stubs
// that never run; they exist only to make the dependent crates compile so that
// rustdoc can render their public APIs.
//
// Maintenance note: add entries here whenever a new ff_sys symbol is referenced
// in ff-probe, ff-decode, or ff-encode.  The values of integer constants are
// taken from the FFmpeg 7.x headers for reference accuracy, but correctness at
// runtime is irrelevant — docs.rs never executes this code.

use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::ptr;

// ── Type aliases ──────────────────────────────────────────────────────────────

pub type AVCodecID = c_uint;
pub type AVPixelFormat = c_int;
pub type AVSampleFormat = c_int;
pub type AVMediaType = c_int;
pub type AVColorPrimaries = c_uint;
pub type AVColorRange = c_uint;
pub type AVColorSpace = c_uint;
pub type AVHWDeviceType = c_int;
pub type AVChannelOrder = c_uint;
pub type AVPictureType = c_int;

// ── Opaque types (only ever used behind raw pointers) ─────────────────────────

pub struct AVDictionary(());
pub struct SwsContext(());
pub struct SwrContext(());
pub struct AVBufferRef(());
pub struct AVIOContext(());
pub struct AVOutputFormat {
    pub flags: c_int,
}
pub struct AVAudioFifo(());

pub struct AVInputFormat {
    pub name: *const c_char,
    pub long_name: *const c_char,
    pub flags: c_int,
}

// ── Structs with field-level access ───────────────────────────────────────────

#[derive(Copy, Clone)]
#[repr(C)]
pub union AVChannelLayout__bindgen_ty_1 {
    pub mask: u64,
}

impl Default for AVChannelLayout__bindgen_ty_1 {
    fn default() -> Self {
        Self { mask: 0 }
    }
}

#[derive(Copy, Clone)]
pub struct AVChannelLayout {
    pub order: AVChannelOrder,
    pub nb_channels: c_int,
    pub u: AVChannelLayout__bindgen_ty_1,
}

impl Default for AVChannelLayout {
    fn default() -> Self {
        Self {
            order: 0,
            nb_channels: 0,
            u: AVChannelLayout__bindgen_ty_1::default(),
        }
    }
}

#[derive(Copy, Clone)]
pub struct AVRational {
    pub num: c_int,
    pub den: c_int,
}

pub struct AVDictionaryEntry {
    pub key: *mut c_char,
    pub value: *mut c_char,
}

pub struct AVChapter {
    pub id: i64,
    pub time_base: AVRational,
    pub start: i64,
    pub end: i64,
    pub metadata: *mut AVDictionary,
}

pub struct AVCodecParameters {
    pub codec_type: AVMediaType,
    pub codec_id: AVCodecID,
    pub codec_tag: c_uint,
    pub extradata: *mut u8,
    pub extradata_size: c_int,
    pub format: c_int,
    pub bit_rate: i64,
    pub width: c_int,
    pub height: c_int,
    pub sample_rate: c_int,
    pub ch_layout: AVChannelLayout,
    pub sample_fmt: AVSampleFormat,
    pub color_space: AVColorSpace,
    pub color_range: AVColorRange,
    pub color_primaries: AVColorPrimaries,
}

pub struct AVStream {
    pub index: c_int,
    pub codecpar: *mut AVCodecParameters,
    pub nb_frames: i64,
    pub duration: i64,
    pub time_base: AVRational,
    pub avg_frame_rate: AVRational,
    pub r_frame_rate: AVRational,
    pub start_time: i64,
    pub disposition: c_int,
    pub metadata: *mut AVDictionary,
}

pub struct AVFormatContext {
    pub nb_streams: c_uint,
    pub streams: *mut *mut AVStream,
    pub duration: i64,
    pub metadata: *mut AVDictionary,
    pub nb_chapters: c_uint,
    pub chapters: *mut *mut AVChapter,
    pub iformat: *mut AVInputFormat,
    pub oformat: *const AVOutputFormat,
    pub bit_rate: i64,
    pub pb: *mut AVIOContext,
    pub priv_data: *mut c_void,
}

pub struct AVFrame {
    pub data: [*mut u8; 8],
    pub linesize: [c_int; 8],
    pub width: c_int,
    pub height: c_int,
    pub nb_samples: c_int,
    pub format: c_int,
    pub key_frame: c_int,
    pub pict_type: AVPictureType,
    pub pts: i64,
    pub pkt_dts: i64,
    pub sample_rate: c_int,
    pub ch_layout: AVChannelLayout,
    pub duration: i64,
    pub time_base: AVRational,
    pub hw_frames_ctx: *mut AVBufferRef,
    pub metadata: *mut AVDictionary,
}

pub struct AVPacket {
    pub pts: i64,
    pub dts: i64,
    pub data: *mut u8,
    pub size: c_int,
    pub stream_index: c_int,
    pub flags: c_int,
    pub duration: i64,
}

pub type AVColorTransferCharacteristic = c_uint;

pub struct AVCodecContext {
    pub codec_id: AVCodecID,
    pub bit_rate: i64,
    pub width: c_int,
    pub height: c_int,
    pub pix_fmt: AVPixelFormat,
    pub sample_rate: c_int,
    pub ch_layout: AVChannelLayout,
    pub sample_fmt: AVSampleFormat,
    pub time_base: AVRational,
    pub framerate: AVRational,
    pub gop_size: c_int,
    pub max_b_frames: c_int,
    pub qmin: c_int,
    pub qmax: c_int,
    pub thread_count: c_int,
    pub hw_device_ctx: *mut AVBufferRef,
    pub hw_frames_ctx: *mut AVBufferRef,
    pub priv_data: *mut c_void,
    pub color_primaries: AVColorPrimaries,
    pub color_trc: AVColorTransferCharacteristic,
    pub colorspace: AVColorSpace,
    // Fields added for v0.7.0 feature coverage
    pub frame_size: c_int,
    pub color_range: AVColorRange,
    pub refs: c_int,
    pub rc_max_rate: i64,
    pub rc_buffer_size: c_int,
    pub flags: c_int,
    pub stats_out: *mut c_char,
    pub stats_in: *mut c_char,
}

pub struct AVCodec {
    pub id: AVCodecID,
    pub sample_fmts: *const AVSampleFormat,
    pub capabilities: c_int,
}

// ── Constants ─────────────────────────────────────────────────────────────────

pub const AV_DICT_IGNORE_SUFFIX: u32 = 2;
pub const AV_NUM_DATA_POINTERS: usize = 8;
pub const AV_TIME_BASE: u32 = 1_000_000;

pub const AVMediaType_AVMEDIA_TYPE_VIDEO: AVMediaType = 0;
pub const AVMediaType_AVMEDIA_TYPE_AUDIO: AVMediaType = 1;
pub const AVMediaType_AVMEDIA_TYPE_SUBTITLE: AVMediaType = 3;
pub const AVMediaType_AVMEDIA_TYPE_ATTACHMENT: AVMediaType = 4;

pub const AV_DISPOSITION_FORCED: u32 = 0x0040;
pub const AV_DISPOSITION_ATTACHED_PIC: u32 = 0x0400;
pub const AV_INPUT_BUFFER_PADDING_SIZE: u32 = 64;

pub const AVChannelOrder_AV_CHANNEL_ORDER_UNSPEC: AVChannelOrder = 0;
pub const AVChannelOrder_AV_CHANNEL_ORDER_NATIVE: AVChannelOrder = 1;

// AVCodecID — video
pub const AVCodecID_AV_CODEC_ID_NONE: AVCodecID = 0;
pub const AVCodecID_AV_CODEC_ID_MPEG2VIDEO: AVCodecID = 2;
pub const AVCodecID_AV_CODEC_ID_MJPEG: AVCodecID = 7;
pub const AVCodecID_AV_CODEC_ID_MPEG4: AVCodecID = 13;
pub const AVCodecID_AV_CODEC_ID_H264: AVCodecID = 27;
pub const AVCodecID_AV_CODEC_ID_THEORA: AVCodecID = 30;
pub const AVCodecID_AV_CODEC_ID_VP8: AVCodecID = 139;
pub const AVCodecID_AV_CODEC_ID_PRORES: AVCodecID = 147;
pub const AVCodecID_AV_CODEC_ID_VP9: AVCodecID = 167;
pub const AVCodecID_AV_CODEC_ID_HEVC: AVCodecID = 173;
pub const AVCodecID_AV_CODEC_ID_AV1: AVCodecID = 226;
pub const AVCodecID_AV_CODEC_ID_DNXHD: AVCodecID = 99;
pub const AVCodecID_AV_CODEC_ID_FFV1: AVCodecID = 33;

// AVCodecID — image (still image)
pub const AVCodecID_AV_CODEC_ID_PNG: AVCodecID = 61;
pub const AVCodecID_AV_CODEC_ID_BMP: AVCodecID = 76;
pub const AVCodecID_AV_CODEC_ID_TIFF: AVCodecID = 90;
pub const AVCodecID_AV_CODEC_ID_WEBP: AVCodecID = 219;
pub const AVCodecID_AV_CODEC_ID_EXR: AVCodecID = 178;

// AVCodecID — animation
pub const AVCodecID_AV_CODEC_ID_GIF: AVCodecID = 97;

// AVCodecID — subtitle
pub const AVCodecID_AV_CODEC_ID_DVB_SUBTITLE: AVCodecID = 94209;
pub const AVCodecID_AV_CODEC_ID_SSA: AVCodecID = 94212;
pub const AVCodecID_AV_CODEC_ID_HDMV_PGS_SUBTITLE: AVCodecID = 94214;
pub const AVCodecID_AV_CODEC_ID_SRT: AVCodecID = 94216;
pub const AVCodecID_AV_CODEC_ID_SUBRIP: AVCodecID = 94248;
pub const AVCodecID_AV_CODEC_ID_WEBVTT: AVCodecID = 94249;
pub const AVCodecID_AV_CODEC_ID_ASS: AVCodecID = 94253;

// AVCodecID — attachment / data
pub const AVCodecID_AV_CODEC_ID_BIN_DATA: AVCodecID = 98314;

// AVCodecID — audio
pub const AVCodecID_AV_CODEC_ID_PCM_S16LE: AVCodecID = 65536;
pub const AVCodecID_AV_CODEC_ID_PCM_S16BE: AVCodecID = 65537;
pub const AVCodecID_AV_CODEC_ID_PCM_U8: AVCodecID = 65542;
pub const AVCodecID_AV_CODEC_ID_PCM_S24LE: AVCodecID = 65544;
pub const AVCodecID_AV_CODEC_ID_PCM_S24BE: AVCodecID = 65545;
pub const AVCodecID_AV_CODEC_ID_PCM_S32LE: AVCodecID = 65556;
pub const AVCodecID_AV_CODEC_ID_PCM_S32BE: AVCodecID = 65557;
pub const AVCodecID_AV_CODEC_ID_PCM_F32LE: AVCodecID = 65558;
pub const AVCodecID_AV_CODEC_ID_PCM_F32BE: AVCodecID = 65559;
pub const AVCodecID_AV_CODEC_ID_PCM_F64LE: AVCodecID = 65560;
pub const AVCodecID_AV_CODEC_ID_PCM_F64BE: AVCodecID = 65561;
pub const AVCodecID_AV_CODEC_ID_MP3: AVCodecID = 86017;
pub const AVCodecID_AV_CODEC_ID_AAC: AVCodecID = 86018;
pub const AVCodecID_AV_CODEC_ID_AC3: AVCodecID = 86019;
pub const AVCodecID_AV_CODEC_ID_DTS: AVCodecID = 86020;
pub const AVCodecID_AV_CODEC_ID_VORBIS: AVCodecID = 86021;
pub const AVCodecID_AV_CODEC_ID_FLAC: AVCodecID = 86028;
pub const AVCodecID_AV_CODEC_ID_ALAC: AVCodecID = 86032;
pub const AVCodecID_AV_CODEC_ID_WMAV2: AVCodecID = 86047;
pub const AVCodecID_AV_CODEC_ID_EAC3: AVCodecID = 86056;
pub const AVCodecID_AV_CODEC_ID_OPUS: AVCodecID = 86076;

// AVPixelFormat
pub const AVPixelFormat_AV_PIX_FMT_NONE: AVPixelFormat = -1;
pub const AVPixelFormat_AV_PIX_FMT_YUV420P: AVPixelFormat = 0;
pub const AVPixelFormat_AV_PIX_FMT_RGB24: AVPixelFormat = 2;
pub const AVPixelFormat_AV_PIX_FMT_BGR24: AVPixelFormat = 3;
pub const AVPixelFormat_AV_PIX_FMT_YUV422P: AVPixelFormat = 4;
pub const AVPixelFormat_AV_PIX_FMT_YUV444P: AVPixelFormat = 5;
pub const AVPixelFormat_AV_PIX_FMT_GRAY8: AVPixelFormat = 8;
pub const AVPixelFormat_AV_PIX_FMT_PAL8: AVPixelFormat = 77;
pub const AVPixelFormat_AV_PIX_FMT_NV12: AVPixelFormat = 23;
pub const AVPixelFormat_AV_PIX_FMT_NV21: AVPixelFormat = 24;
pub const AVPixelFormat_AV_PIX_FMT_RGBA: AVPixelFormat = 26;
pub const AVPixelFormat_AV_PIX_FMT_BGRA: AVPixelFormat = 28;
pub const AVPixelFormat_AV_PIX_FMT_YUVJ420P: AVPixelFormat = 12;
pub const AVPixelFormat_AV_PIX_FMT_YUVJ422P: AVPixelFormat = 13;
pub const AVPixelFormat_AV_PIX_FMT_YUVJ444P: AVPixelFormat = 14;
pub const AVPixelFormat_AV_PIX_FMT_VAAPI: AVPixelFormat = 51;
pub const AVPixelFormat_AV_PIX_FMT_DXVA2_VLD: AVPixelFormat = 53;
pub const AV_OPT_SEARCH_CHILDREN: u32 = 1;
pub const AVPixelFormat_AV_PIX_FMT_YUV420P10LE: AVPixelFormat = 66;
pub const AVPixelFormat_AV_PIX_FMT_YUV422P10LE: AVPixelFormat = 64;
pub const AVPixelFormat_AV_PIX_FMT_YUV444P10LE: AVPixelFormat = 68;
pub const AVPixelFormat_AV_PIX_FMT_YUVA444P10LE: AVPixelFormat = 91;
pub const AVPixelFormat_AV_PIX_FMT_VDPAU: AVPixelFormat = 101;
pub const AVPixelFormat_AV_PIX_FMT_CUDA: AVPixelFormat = 119;
pub const AVPixelFormat_AV_PIX_FMT_QSV: AVPixelFormat = 123;
pub const AVPixelFormat_AV_PIX_FMT_VIDEOTOOLBOX: AVPixelFormat = 135;
pub const AVPixelFormat_AV_PIX_FMT_MEDIACODEC: AVPixelFormat = 165;
pub const AVPixelFormat_AV_PIX_FMT_P010LE: AVPixelFormat = 161;
pub const AVPixelFormat_AV_PIX_FMT_GBRPF32LE: AVPixelFormat = 175;
pub const AVPixelFormat_AV_PIX_FMT_D3D11: AVPixelFormat = 174;
pub const AVPixelFormat_AV_PIX_FMT_OPENCL: AVPixelFormat = 180;
pub const AVPixelFormat_AV_PIX_FMT_VULKAN: AVPixelFormat = 193;

// AVSampleFormat
pub const AVSampleFormat_AV_SAMPLE_FMT_NONE: AVSampleFormat = -1;
pub const AVSampleFormat_AV_SAMPLE_FMT_U8: AVSampleFormat = 0;
pub const AVSampleFormat_AV_SAMPLE_FMT_S16: AVSampleFormat = 1;
pub const AVSampleFormat_AV_SAMPLE_FMT_S32: AVSampleFormat = 2;
pub const AVSampleFormat_AV_SAMPLE_FMT_FLT: AVSampleFormat = 3;
pub const AVSampleFormat_AV_SAMPLE_FMT_DBL: AVSampleFormat = 4;
pub const AVSampleFormat_AV_SAMPLE_FMT_U8P: AVSampleFormat = 5;
pub const AVSampleFormat_AV_SAMPLE_FMT_S16P: AVSampleFormat = 6;
pub const AVSampleFormat_AV_SAMPLE_FMT_S32P: AVSampleFormat = 7;
pub const AVSampleFormat_AV_SAMPLE_FMT_FLTP: AVSampleFormat = 8;
pub const AVSampleFormat_AV_SAMPLE_FMT_DBLP: AVSampleFormat = 9;
pub const AVSampleFormat_AV_SAMPLE_FMT_S64: AVSampleFormat = 10;
pub const AVSampleFormat_AV_SAMPLE_FMT_S64P: AVSampleFormat = 11;

// AVColorPrimaries
pub const AVColorPrimaries_AVCOL_PRI_BT709: AVColorPrimaries = 1;
pub const AVColorPrimaries_AVCOL_PRI_UNSPECIFIED: AVColorPrimaries = 2;
pub const AVColorPrimaries_AVCOL_PRI_BT470BG: AVColorPrimaries = 5;
pub const AVColorPrimaries_AVCOL_PRI_SMPTE170M: AVColorPrimaries = 6;
pub const AVColorPrimaries_AVCOL_PRI_SMPTE240M: AVColorPrimaries = 7;
pub const AVColorPrimaries_AVCOL_PRI_FILM: AVColorPrimaries = 8;
pub const AVColorPrimaries_AVCOL_PRI_BT2020: AVColorPrimaries = 9;
pub const AVColorPrimaries_AVCOL_PRI_SMPTE431: AVColorPrimaries = 11;
pub const AVColorPrimaries_AVCOL_PRI_SMPTE432: AVColorPrimaries = 12;

// AVColorRange
pub const AVColorRange_AVCOL_RANGE_UNSPECIFIED: AVColorRange = 0;
pub const AVColorRange_AVCOL_RANGE_MPEG: AVColorRange = 1;
pub const AVColorRange_AVCOL_RANGE_JPEG: AVColorRange = 2;

// AVColorSpace
pub const AVColorSpace_AVCOL_SPC_RGB: AVColorSpace = 0;
pub const AVColorSpace_AVCOL_SPC_BT709: AVColorSpace = 1;
pub const AVColorSpace_AVCOL_SPC_UNSPECIFIED: AVColorSpace = 2;
pub const AVColorSpace_AVCOL_SPC_FCC: AVColorSpace = 4;
pub const AVColorSpace_AVCOL_SPC_BT470BG: AVColorSpace = 5;
pub const AVColorSpace_AVCOL_SPC_SMPTE170M: AVColorSpace = 6;
pub const AVColorSpace_AVCOL_SPC_SMPTE240M: AVColorSpace = 7;
pub const AVColorSpace_AVCOL_SPC_YCGCO: AVColorSpace = 8;
pub const AVColorSpace_AVCOL_SPC_BT2020_NCL: AVColorSpace = 9;
pub const AVColorSpace_AVCOL_SPC_BT2020_CL: AVColorSpace = 10;

// AVColorTransferCharacteristic
pub const AVColorTransferCharacteristic_AVCOL_TRC_BT709: AVColorTransferCharacteristic = 1;
pub const AVColorTransferCharacteristic_AVCOL_TRC_UNSPECIFIED: AVColorTransferCharacteristic = 2;
pub const AVColorTransferCharacteristic_AVCOL_TRC_GAMMA22: AVColorTransferCharacteristic = 4;
pub const AVColorTransferCharacteristic_AVCOL_TRC_GAMMA28: AVColorTransferCharacteristic = 5;
pub const AVColorTransferCharacteristic_AVCOL_TRC_SMPTE170M: AVColorTransferCharacteristic = 6;
pub const AVColorTransferCharacteristic_AVCOL_TRC_SMPTE240M: AVColorTransferCharacteristic = 7;
pub const AVColorTransferCharacteristic_AVCOL_TRC_LINEAR: AVColorTransferCharacteristic = 8;
pub const AVColorTransferCharacteristic_AVCOL_TRC_IEC61966_2_1: AVColorTransferCharacteristic = 13;
pub const AVColorTransferCharacteristic_AVCOL_TRC_BT2020_10: AVColorTransferCharacteristic = 14;
pub const AVColorTransferCharacteristic_AVCOL_TRC_BT2020_12: AVColorTransferCharacteristic = 15;
pub const AVColorTransferCharacteristic_AVCOL_TRC_SMPTEST2084: AVColorTransferCharacteristic = 16;
pub const AVColorTransferCharacteristic_AVCOL_TRC_ARIB_STD_B67: AVColorTransferCharacteristic = 18;

// AVPacketSideDataType
pub type AVPacketSideDataType = c_uint;
pub const AVPacketSideDataType_AV_PKT_DATA_MASTERING_DISPLAY_METADATA: AVPacketSideDataType = 20;
pub const AVPacketSideDataType_AV_PKT_DATA_CONTENT_LIGHT_LEVEL: AVPacketSideDataType = 22;

// AVHWDeviceType
pub const AVHWDeviceType_AV_HWDEVICE_TYPE_CUDA: AVHWDeviceType = 2;
pub const AVHWDeviceType_AV_HWDEVICE_TYPE_VAAPI: AVHWDeviceType = 4;
pub const AVHWDeviceType_AV_HWDEVICE_TYPE_QSV: AVHWDeviceType = 5;
pub const AVHWDeviceType_AV_HWDEVICE_TYPE_VIDEOTOOLBOX: AVHWDeviceType = 7;
pub const AVHWDeviceType_AV_HWDEVICE_TYPE_D3D11VA: AVHWDeviceType = 8;

// AVPictureType constants
pub const AVPictureType_AV_PICTURE_TYPE_NONE: AVPictureType = 0;
pub const AVPictureType_AV_PICTURE_TYPE_I: AVPictureType = 1;

// ── Raw FFmpeg functions (bindgen-generated counterparts) ─────────────────────
//
// These mirror what bindgen would emit from the real FFmpeg headers.
// All bodies are stubs; the code is compiled but never executed on docs.rs.

// SAFETY: docs.rs stubs — never called at runtime.
pub unsafe fn av_strerror(_errnum: c_int, _errbuf: *mut c_char, _errbuf_size: usize) -> c_int {
    0
}

pub unsafe fn av_dict_get(
    _m: *const AVDictionary,
    _key: *const c_char,
    _prev: *const AVDictionaryEntry,
    _flags: c_int,
) -> *mut AVDictionaryEntry {
    std::ptr::null_mut()
}

pub unsafe fn av_dict_set(
    _pm: *mut *mut AVDictionary,
    _key: *const c_char,
    _value: *const c_char,
    _flags: c_int,
) -> c_int {
    0
}

pub unsafe fn av_dict_free(_m: *mut *mut AVDictionary) {}

pub unsafe fn av_find_input_format(_short_name: *const c_char) -> *const AVInputFormat {
    std::ptr::null()
}

pub unsafe fn avcodec_get_name(_id: AVCodecID) -> *const c_char {
    std::ptr::null()
}

pub unsafe fn av_frame_alloc() -> *mut AVFrame {
    std::ptr::null_mut()
}

pub unsafe fn av_frame_free(_frame: *mut *mut AVFrame) {}

pub unsafe fn av_frame_get_buffer(_frame: *mut AVFrame, _align: c_int) -> c_int {
    0
}

pub unsafe fn av_frame_move_ref(_dst: *mut AVFrame, _src: *mut AVFrame) {}

pub unsafe fn av_frame_unref(_frame: *mut AVFrame) {}

pub unsafe fn av_packet_alloc() -> *mut AVPacket {
    std::ptr::null_mut()
}

pub unsafe fn av_packet_free(_pkt: *mut *mut AVPacket) {}

pub unsafe fn av_packet_unref(_pkt: *mut AVPacket) {}

pub unsafe fn av_packet_new_side_data(
    _pkt: *mut AVPacket,
    _type_: AVPacketSideDataType,
    _size: usize,
) -> *mut u8 {
    std::ptr::null_mut()
}

pub unsafe fn av_buffer_ref(_buf: *mut AVBufferRef) -> *mut AVBufferRef {
    std::ptr::null_mut()
}

pub unsafe fn av_buffer_unref(_buf: *mut *mut AVBufferRef) {}

pub unsafe fn av_hwdevice_ctx_create(
    _device_ctx: *mut *mut AVBufferRef,
    _type_: AVHWDeviceType,
    _device: *const c_char,
    _opts: *mut AVDictionary,
    _flags: c_int,
) -> c_int {
    -1
}

pub unsafe fn av_hwframe_transfer_data(
    _dst: *mut AVFrame,
    _src: *const AVFrame,
    _flags: c_int,
) -> c_int {
    -1
}

pub unsafe fn av_opt_set(
    _obj: *mut c_void,
    _name: *const c_char,
    _val: *const c_char,
    _search_flags: c_int,
) -> c_int {
    0
}

pub unsafe fn av_read_frame(_s: *mut AVFormatContext, _pkt: *mut AVPacket) -> c_int {
    -1
}

pub unsafe fn av_write_trailer(_s: *mut AVFormatContext) -> c_int {
    0
}

pub unsafe fn av_interleaved_write_frame(
    _s: *mut AVFormatContext,
    _pkt: *mut AVPacket,
) -> c_int {
    0
}

pub unsafe fn avcodec_receive_frame(_avctx: *mut AVCodecContext, _frame: *mut AVFrame) -> c_int {
    -1
}

pub unsafe fn avcodec_send_packet(
    _avctx: *mut AVCodecContext,
    _avpkt: *const AVPacket,
) -> c_int {
    -1
}

pub unsafe fn avformat_alloc_output_context2(
    _ctx: *mut *mut AVFormatContext,
    _oformat: *mut AVOutputFormat,
    _format_name: *const c_char,
    _filename: *const c_char,
) -> c_int {
    -1
}

pub unsafe fn avformat_free_context(_s: *mut AVFormatContext) {}

pub unsafe fn avformat_new_stream(
    _s: *mut AVFormatContext,
    _c: *const AVCodec,
) -> *mut AVStream {
    std::ptr::null_mut()
}

pub unsafe fn avformat_write_header(
    _s: *mut AVFormatContext,
    _options: *mut *mut AVDictionary,
) -> c_int {
    -1
}

pub unsafe fn swr_alloc_set_opts2(
    _ps: *mut *mut SwrContext,
    _out_ch_layout: *const AVChannelLayout,
    _out_sample_fmt: AVSampleFormat,
    _out_sample_rate: c_int,
    _in_ch_layout: *const AVChannelLayout,
    _in_sample_fmt: AVSampleFormat,
    _in_sample_rate: c_int,
    _log_offset: c_int,
    _log_ctx: *mut c_void,
) -> c_int {
    -1
}

pub unsafe fn swr_convert(
    _s: *mut SwrContext,
    _out: *mut *mut u8,
    _out_count: c_int,
    _in_: *const *const u8,
    _in_count: c_int,
) -> c_int {
    -1
}

pub unsafe fn swr_free(_s: *mut *mut SwrContext) {}

pub unsafe fn swr_get_out_samples(_s: *mut SwrContext, _in_samples: c_int) -> c_int {
    0
}

pub unsafe fn swr_init(_s: *mut SwrContext) -> c_int {
    -1
}

pub unsafe fn av_channel_layout_default(_ch_layout: *mut AVChannelLayout, _nb_channels: c_int) {}

pub unsafe fn av_channel_layout_uninit(_ch_layout: *mut AVChannelLayout) {}

pub unsafe fn av_rescale_q(_a: i64, _bq: AVRational, _cq: AVRational) -> i64 {
    0
}

pub unsafe fn av_mallocz(_size: usize) -> *mut c_void {
    std::ptr::null_mut()
}

pub unsafe fn av_malloc(_size: usize) -> *mut c_void {
    std::ptr::null_mut()
}

pub unsafe fn av_free(_ptr: *mut c_void) {}

pub unsafe fn av_new_packet(_pkt: *mut AVPacket, _size: c_int) -> c_int {
    -1
}

pub unsafe fn avcodec_parameters_copy(
    _dst: *mut AVCodecParameters,
    _src: *const AVCodecParameters,
) -> c_int {
    0
}

pub unsafe fn av_packet_rescale_ts(
    _pkt: *mut AVPacket,
    _tb_src: AVRational,
    _tb_dst: AVRational,
) {
}

// ── Wrapper module stubs ──────────────────────────────────────────────────────
//
// These mirror the safe wrapper modules in avformat.rs, avcodec.rs,
// swresample.rs, and swscale.rs.  Signatures must exactly match those files.

// ── libavfilter opaque types ──────────────────────────────────────────────────

pub struct AVFilterGraph(());
pub struct AVFilter(());

pub struct AVFilterContext {
    pub hw_device_ctx: *mut AVBufferRef,
}

// ── libavfilter constants ─────────────────────────────────────────────────────

/// Flag for `av_buffersrc_add_frame_flags`: keep a reference to the frame.
pub const AV_BUFFERSRC_FLAG_KEEP_REF: c_int = 8;

// ── libavfilter functions ─────────────────────────────────────────────────────

// SAFETY: docs.rs stubs — never called at runtime.

pub unsafe fn avfilter_graph_alloc() -> *mut AVFilterGraph {
    ptr::null_mut()
}

pub unsafe fn avfilter_graph_free(_graph: *mut *mut AVFilterGraph) {}

pub unsafe fn avfilter_get_by_name(_name: *const c_char) -> *const AVFilter {
    ptr::null()
}

pub unsafe fn avfilter_graph_create_filter(
    _filt_ctx: *mut *mut AVFilterContext,
    _filt: *const AVFilter,
    _name: *const c_char,
    _args: *const c_char,
    _opaque: *mut c_void,
    _graph_ctx: *mut AVFilterGraph,
) -> c_int {
    0
}

pub unsafe fn avfilter_link(
    _src: *mut AVFilterContext,
    _srcpad: c_uint,
    _dst: *mut AVFilterContext,
    _dstpad: c_uint,
) -> c_int {
    0
}

pub unsafe fn avfilter_graph_config(
    _graphctx: *mut AVFilterGraph,
    _log_ctx: *mut c_void,
) -> c_int {
    0
}

pub unsafe fn avfilter_graph_set_auto_convert(_graph: *mut AVFilterGraph, _flags: c_uint) {}

pub unsafe fn avfilter_graph_send_command(
    _graph: *mut AVFilterGraph,
    _target: *const c_char,
    _cmd: *const c_char,
    _arg: *const c_char,
    _res: *mut c_char,
    _res_len: c_int,
    _flags: c_int,
) -> c_int {
    0
}

pub unsafe fn av_buffersrc_add_frame_flags(
    _ctx: *mut AVFilterContext,
    _frame: *mut AVFrame,
    _flags: c_int,
) -> c_int {
    0
}

pub unsafe fn av_buffersrc_close(
    _ctx: *mut AVFilterContext,
    _pts: i64,
    _flags: c_uint,
) -> c_int {
    0
}

pub unsafe fn av_buffersink_get_frame(
    _ctx: *mut AVFilterContext,
    _frame: *mut AVFrame,
) -> c_int {
    // Return EAGAIN to signal no frame available
    -11
}

pub unsafe fn av_buffersink_get_time_base(_ctx: *const AVFilterContext) -> AVRational {
    AVRational { num: 0, den: 1 }
}

pub unsafe fn av_image_get_linesize(
    _pix_fmt: AVPixelFormat,
    _width: c_int,
    _plane: c_int,
) -> c_int {
    0
}

/// Stub `avformat` wrapper module.
pub mod avformat {
    use std::os::raw::c_int;
    use std::path::Path;

    use super::{AVFormatContext, AVIOContext, AVPacket};

    pub fn srt_available() -> bool {
        false
    }

    pub unsafe fn open_input(_path: &Path) -> Result<*mut AVFormatContext, c_int> {
        Err(-1)
    }

    pub unsafe fn open_input_url(
        _url: &str,
        _connect_timeout: std::time::Duration,
        _read_timeout: std::time::Duration,
    ) -> Result<*mut AVFormatContext, c_int> {
        Err(-1)
    }

    pub unsafe fn open_input_image_sequence(
        _path: &Path,
        _framerate: u32,
    ) -> Result<*mut AVFormatContext, c_int> {
        Err(-1)
    }

    pub unsafe fn close_input(_ctx: *mut *mut AVFormatContext) {}

    pub unsafe fn find_stream_info(_ctx: *mut AVFormatContext) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn seek_frame(
        _ctx: *mut AVFormatContext,
        _stream_index: c_int,
        _timestamp: i64,
        _flags: c_int,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn seek_file(
        _ctx: *mut AVFormatContext,
        _stream_index: c_int,
        _min_ts: i64,
        _ts: i64,
        _max_ts: i64,
        _flags: c_int,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn read_frame(
        _ctx: *mut AVFormatContext,
        _pkt: *mut AVPacket,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn write_frame(
        _ctx: *mut AVFormatContext,
        _pkt: *mut AVPacket,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn open_output(_path: &Path, _flags: c_int) -> Result<*mut AVIOContext, c_int> {
        Err(-1)
    }

    pub unsafe fn close_output(_pb: *mut *mut AVIOContext) {}

    pub mod avio_flags {
        use std::os::raw::c_int;
        pub const READ: c_int = 1;
        pub const WRITE: c_int = 2;
        pub const READ_WRITE: c_int = 3;
    }

    pub mod seek_flags {
        pub const BACKWARD: i32 = 1;
        pub const BYTE: i32 = 2;
        pub const ANY: i32 = 4;
        pub const FRAME: i32 = 8;
    }
}

/// Stub `avcodec` wrapper module.
pub mod avcodec {
    use std::os::raw::c_int;

    use super::{AVCodec, AVCodecContext, AVCodecID, AVCodecParameters, AVDictionary, AVFrame, AVPacket};

    pub unsafe fn find_decoder(_codec_id: AVCodecID) -> Option<*const AVCodec> {
        None
    }

    pub unsafe fn find_decoder_by_name(_name: *const i8) -> Option<*const AVCodec> {
        None
    }

    pub unsafe fn find_encoder(_codec_id: AVCodecID) -> Option<*const AVCodec> {
        None
    }

    pub unsafe fn find_encoder_by_name(_name: *const i8) -> Option<*const AVCodec> {
        None
    }

    pub unsafe fn alloc_context3(_codec: *const AVCodec) -> Result<*mut AVCodecContext, c_int> {
        Err(-1)
    }

    pub unsafe fn parameters_to_context(
        _codec_ctx: *mut AVCodecContext,
        _par: *const AVCodecParameters,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn open2(
        _avctx: *mut AVCodecContext,
        _codec: *const AVCodec,
        _options: *mut *mut AVDictionary,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn send_packet(
        _ctx: *mut AVCodecContext,
        _pkt: *const AVPacket,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn receive_frame(
        _ctx: *mut AVCodecContext,
        _frame: *mut AVFrame,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn send_frame(
        _ctx: *mut AVCodecContext,
        _frame: *const AVFrame,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn receive_packet(
        _ctx: *mut AVCodecContext,
        _pkt: *mut AVPacket,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn flush_buffers(_ctx: *mut AVCodecContext) {}

    pub unsafe fn parameters_from_context(
        _par: *mut AVCodecParameters,
        _ctx: *const AVCodecContext,
    ) -> Result<(), c_int> {
        Err(-1)
    }

    pub mod codec_caps {
        pub const EXPERIMENTAL: u32 = 1 << 9;
        pub const HARDWARE: u32 = 1 << 10;
        pub const HYBRID: u32 = 1 << 11;
        pub const VARIABLE_FRAME_SIZE: u32 = 1 << 16;
        pub const AVOID_PROBING: u32 = 1 << 17;
    }
}

/// Stub `swresample` wrapper module.
pub mod swresample {
    use std::os::raw::c_int;

    use super::{AVChannelLayout, AVSampleFormat, SwrContext};

    pub unsafe fn alloc() -> Result<*mut SwrContext, c_int> {
        Err(-1)
    }

    pub unsafe fn alloc_set_opts2(
        _out_ch_layout: *const AVChannelLayout,
        _out_sample_fmt: AVSampleFormat,
        _out_sample_rate: c_int,
        _in_ch_layout: *const AVChannelLayout,
        _in_sample_fmt: AVSampleFormat,
        _in_sample_rate: c_int,
    ) -> Result<*mut SwrContext, c_int> {
        Err(-1)
    }

    pub unsafe fn init(_ctx: *mut SwrContext) -> Result<(), c_int> {
        Err(-1)
    }

    pub unsafe fn is_initialized(_ctx: *const SwrContext) -> bool {
        false
    }

    pub unsafe fn convert(
        _s: *mut SwrContext,
        _out: *mut *mut u8,
        _out_count: c_int,
        _in_: *const *const u8,
        _in_count: c_int,
    ) -> Result<c_int, c_int> {
        Err(-1)
    }

    pub unsafe fn get_delay(_ctx: *mut SwrContext, _base: i64) -> i64 {
        0
    }

    pub fn estimate_output_samples(
        _out_sample_rate: i32,
        _in_sample_rate: i32,
        _in_samples: i32,
    ) -> i32 {
        0
    }

    pub mod channel_layout {
        use super::super::AVChannelLayout;

        pub unsafe fn set_default(_ch_layout: *mut AVChannelLayout, _nb_channels: i32) {}
        pub unsafe fn uninit(_ch_layout: *mut AVChannelLayout) {}
        pub unsafe fn copy(
            _dst: *mut AVChannelLayout,
            _src: *const AVChannelLayout,
        ) -> Result<(), i32> {
            Err(-1)
        }
        pub unsafe fn is_equal(
            _chl: *const AVChannelLayout,
            _chl1: *const AVChannelLayout,
        ) -> bool {
            false
        }
        pub fn mono() -> AVChannelLayout {
            AVChannelLayout::default()
        }
        pub fn stereo() -> AVChannelLayout {
            AVChannelLayout::default()
        }
        pub fn with_channels(_nb_channels: i32) -> AVChannelLayout {
            AVChannelLayout::default()
        }
        pub fn is_valid(ch_layout: &AVChannelLayout) -> bool {
            ch_layout.nb_channels > 0
        }
        pub fn nb_channels(ch_layout: &AVChannelLayout) -> i32 {
            ch_layout.nb_channels
        }
        pub fn is_native_order(_ch_layout: &AVChannelLayout) -> bool {
            false
        }
    }

    pub mod audio_fifo {
        use std::ffi::c_void;
        use std::os::raw::c_int;

        use super::super::{AVAudioFifo, AVSampleFormat};

        pub unsafe fn alloc(
            _sample_fmt: AVSampleFormat,
            _channels: c_int,
            _nb_samples: c_int,
        ) -> Result<*mut AVAudioFifo, c_int> {
            Err(-1)
        }

        pub unsafe fn free(_fifo: *mut AVAudioFifo) {}

        pub unsafe fn write(
            _fifo: *mut AVAudioFifo,
            _data: *const *mut c_void,
            _nb_samples: c_int,
        ) -> Result<c_int, c_int> {
            Err(-1)
        }

        pub unsafe fn read(
            _fifo: *mut AVAudioFifo,
            _data: *const *mut c_void,
            _nb_samples: c_int,
        ) -> Result<c_int, c_int> {
            Err(-1)
        }

        pub unsafe fn size(_fifo: *mut AVAudioFifo) -> c_int {
            0
        }

        pub unsafe fn write_frame(
            _fifo: *mut AVAudioFifo,
            _src: &super::super::Frame,
            _nb_samples: c_int,
        ) -> Result<c_int, c_int> {
            Err(-1)
        }

        pub unsafe fn read_frame(
            _fifo: *mut AVAudioFifo,
            _dst: &mut super::super::Frame,
            _nb_samples: c_int,
        ) -> Result<c_int, c_int> {
            Err(-1)
        }
    }

    pub mod sample_format {
        use super::super::{
            AVSampleFormat, AVSampleFormat_AV_SAMPLE_FMT_NONE, AVSampleFormat_AV_SAMPLE_FMT_U8,
            AVSampleFormat_AV_SAMPLE_FMT_S16, AVSampleFormat_AV_SAMPLE_FMT_S32,
            AVSampleFormat_AV_SAMPLE_FMT_FLT, AVSampleFormat_AV_SAMPLE_FMT_DBL,
            AVSampleFormat_AV_SAMPLE_FMT_U8P, AVSampleFormat_AV_SAMPLE_FMT_S16P,
            AVSampleFormat_AV_SAMPLE_FMT_S32P, AVSampleFormat_AV_SAMPLE_FMT_FLTP,
            AVSampleFormat_AV_SAMPLE_FMT_DBLP, AVSampleFormat_AV_SAMPLE_FMT_S64,
            AVSampleFormat_AV_SAMPLE_FMT_S64P,
        };

        pub const NONE: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_NONE;
        pub const U8: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_U8;
        pub const S16: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S16;
        pub const S32: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S32;
        pub const FLT: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_FLT;
        pub const DBL: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_DBL;
        pub const U8P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_U8P;
        pub const S16P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S16P;
        pub const S32P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S32P;
        pub const FLTP: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_FLTP;
        pub const DBLP: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_DBLP;
        pub const S64: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S64;
        pub const S64P: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_S64P;

        pub fn bytes_per_sample(_sample_fmt: AVSampleFormat) -> i32 {
            0
        }
        pub fn is_planar(_sample_fmt: AVSampleFormat) -> bool {
            false
        }
    }
}

/// Stub `swscale` wrapper module.
pub mod swscale {
    use std::os::raw::c_int;

    use super::{AVPixelFormat, SwsContext};

    pub unsafe fn get_context(
        _src_w: c_int,
        _src_h: c_int,
        _src_fmt: AVPixelFormat,
        _dst_w: c_int,
        _dst_h: c_int,
        _dst_fmt: AVPixelFormat,
        _flags: c_int,
    ) -> Result<*mut SwsContext, c_int> {
        Err(-1)
    }

    pub unsafe fn scale(
        _ctx: *mut SwsContext,
        _src: *const *const u8,
        _src_stride: *const c_int,
        _src_slice_y: c_int,
        _src_slice_h: c_int,
        _dst: *const *mut u8,
        _dst_stride: *const c_int,
    ) -> Result<c_int, c_int> {
        Err(-1)
    }

    pub unsafe fn is_supported_input(_pix_fmt: AVPixelFormat) -> bool {
        false
    }

    pub unsafe fn is_supported_output(_pix_fmt: AVPixelFormat) -> bool {
        false
    }

    pub unsafe fn is_supported_endianness_conversion(_pix_fmt: AVPixelFormat) -> bool {
        false
    }

    pub mod scale_flags {
        pub const FAST_BILINEAR: i32 = 1;
        pub const BILINEAR: i32 = 2;
        pub const BICUBIC: i32 = 4;
        pub const X: i32 = 8;
        pub const POINT: i32 = 16;
        pub const AREA: i32 = 32;
        pub const BICUBLIN: i32 = 64;
        pub const GAUSS: i32 = 128;
        pub const SINC: i32 = 256;
        pub const LANCZOS: i32 = 512;
        pub const SPLINE: i32 = 1024;
    }
}

// ── RAII owner stub (mirrors crate::codec_context::CodecContext) ──────────────
// Under DOCS_RS the real `codec_context` module is cfg'd out (it depends on the
// docsrs-gated `avcodec` wrapper); this shape-compatible stub keeps
// `ff_sys::CodecContext` available so ff-decode compiles for docs.rs.

/// Stub mirror of `crate::codec_context::ReceiveOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveOutcome {
    Frame,
    NeedInput,
    Drained,
}

#[derive(Debug)]
pub struct CodecContext {
    _ptr: std::ptr::NonNull<AVCodecContext>,
}

impl CodecContext {
    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn new(_codec: Option<Codec>) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const AVCodecContext {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVCodecContext {
        self._ptr.as_ptr()
    }

    pub fn set_thread_count(&mut self, _thread_count: c_int) {}

    pub fn set_width(&mut self, _width: c_int) {}
    pub fn set_height(&mut self, _height: c_int) {}
    pub fn set_pix_fmt(&mut self, _pix_fmt: AVPixelFormat) {}
    pub fn set_time_base(&mut self, _time_base: AVRational) {}

    #[must_use]
    pub fn pix_fmt(&self) -> AVPixelFormat {
        0
    }
    #[must_use]
    pub fn sample_fmt(&self) -> AVSampleFormat {
        0
    }
    #[must_use]
    pub fn width(&self) -> c_int {
        0
    }
    #[must_use]
    pub fn height(&self) -> c_int {
        0
    }

    pub fn set_codec_id(&mut self, _codec_id: AVCodecID) {}
    pub fn set_framerate(&mut self, _framerate: AVRational) {}
    pub fn set_bit_rate(&mut self, _bit_rate: i64) {}
    #[must_use]
    pub fn bit_rate(&self) -> i64 {
        0
    }
    pub fn set_rc_max_rate(&mut self, _rc_max_rate: i64) {}
    pub fn set_rc_buffer_size(&mut self, _rc_buffer_size: c_int) {}
    pub fn set_color_primaries(&mut self, _color_primaries: AVColorPrimaries) {}
    pub fn set_color_trc(&mut self, _color_trc: AVColorTransferCharacteristic) {}
    pub fn set_colorspace(&mut self, _colorspace: AVColorSpace) {}
    pub fn set_color_range(&mut self, _color_range: AVColorRange) {}
    pub fn set_sample_rate(&mut self, _sample_rate: c_int) {}
    pub fn set_sample_fmt(&mut self, _sample_fmt: AVSampleFormat) {}
    pub fn set_max_b_frames(&mut self, _max_b_frames: c_int) {}
    pub fn set_gop_size(&mut self, _gop_size: c_int) {}
    pub fn set_refs(&mut self, _refs: c_int) {}
    pub fn set_qmin(&mut self, _qmin: c_int) {}
    pub fn set_qmax(&mut self, _qmax: c_int) {}
    pub fn set_ch_layout_default(&mut self, _nb_channels: c_int) {}
    pub fn set_flags(&mut self, _flags: c_int) {}

    #[must_use]
    pub fn flags(&self) -> c_int {
        0
    }
    #[must_use]
    pub fn time_base(&self) -> AVRational {
        AVRational { num: 0, den: 1 }
    }
    #[must_use]
    pub fn codec_id(&self) -> AVCodecID {
        0
    }
    #[must_use]
    pub fn frame_size(&self) -> c_int {
        0
    }
    #[must_use]
    pub fn sample_rate(&self) -> c_int {
        0
    }
    #[must_use]
    pub fn channels(&self) -> c_int {
        0
    }
    #[must_use]
    pub fn ch_layout(&self) -> &AVChannelLayout {
        Box::leak(Box::new(AVChannelLayout::default()))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn set_opt(&mut self, _key: &str, _value: &str) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn set_opt_cstr(
        &mut self,
        _key: &str,
        _value: &core::ffi::CStr,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn set_opt_search_children(
        &mut self,
        _key: &str,
        _value: &str,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    pub fn set_stats_in(&mut self, _stats: &std::ffi::CStr) {}

    pub fn clear_stats_in(&mut self) {}

    #[must_use]
    pub fn stats_out(&self) -> Option<std::ffi::CString> {
        None
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn apply_parameters(
        &mut self,
        _params: &CodecParameters<'_>,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn open_codec(&mut self, _codec: Codec) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn parameters_to_context(
        &mut self,
        _par: *const AVCodecParameters,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn open(
        &mut self,
        _codec: Codec,
        _options: *mut *mut AVDictionary,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn send_packet(&mut self, _pkt: &Packet) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn send_eof(&mut self) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn receive_frame(&mut self, _frame: &mut Frame) -> Result<ReceiveOutcome, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn flush_buffers(&mut self) {}

    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn send_frame(&mut self, _frame: *const AVFrame) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn receive_packet(
        &mut self,
        _pkt: *mut AVPacket,
    ) -> Result<ReceiveOutcome, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn send_frame_ref(&mut self, _frame: Option<&Frame>) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn receive_packet_into(
        &mut self,
        _pkt: &mut Packet,
    ) -> Result<ReceiveOutcome, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn parameters_from_context(
        &self,
        _par: *mut AVCodecParameters,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }
}

impl Drop for CodecContext {
    fn drop(&mut self) {}
}

// SAFETY: stub; never executed on docs.rs.
unsafe impl Send for CodecContext {}

// ── RAII owner stub (mirrors crate::format_context::InputFormatContext) ───────
// Under DOCS_RS the real `format_context` module is cfg'd out (it depends on the
// docsrs-gated `avformat` wrapper); this shape-compatible stub keeps
// `ff_sys::InputFormatContext` available so ff-decode compiles for docs.rs.

#[derive(Debug)]
pub struct InputFormatContext {
    _ptr: std::ptr::NonNull<AVFormatContext>,
}

impl InputFormatContext {
    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn open(_path: &std::path::Path) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn open_url(
        _url: &str,
        _connect_timeout: std::time::Duration,
        _read_timeout: std::time::Duration,
    ) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn open_image_sequence(
        _path: &std::path::Path,
        _framerate: u32,
    ) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const AVFormatContext {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVFormatContext {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn nb_streams(&self) -> u32 {
        0
    }

    #[must_use]
    pub fn duration(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn iformat_flags(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn bit_rate(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn iformat_name(&self) -> Option<String> {
        None
    }

    #[must_use]
    pub fn iformat_long_name(&self) -> Option<String> {
        None
    }

    #[must_use]
    pub fn metadata(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    #[must_use]
    pub fn nb_chapters(&self) -> u32 {
        0
    }

    #[must_use]
    pub fn chapter(&self, _index: usize) -> Option<ChapterRef<'_>> {
        None
    }

    pub fn chapters(&self) -> impl Iterator<Item = ChapterRef<'_>> + '_ {
        std::iter::empty()
    }

    #[must_use]
    pub fn stream(&self, _index: usize) -> Option<StreamRef<'_>> {
        None
    }

    pub fn streams(&self) -> impl Iterator<Item = StreamRef<'_>> + '_ {
        std::iter::empty()
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn find_stream_info(&mut self) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn seek_frame(
        &mut self,
        _stream_index: c_int,
        _timestamp: i64,
        _flags: c_int,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn seek_file(
        &mut self,
        _stream_index: c_int,
        _min_ts: i64,
        _ts: i64,
        _max_ts: i64,
        _flags: c_int,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn read_frame(&mut self, _pkt: &mut Packet) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }
}

impl Drop for InputFormatContext {
    fn drop(&mut self) {}
}

// SAFETY: stub; never executed on docs.rs.
unsafe impl Send for InputFormatContext {}

// ── RAII owner stub (mirrors crate::format_context::OutputFormatContext) ──────
// Keeps `ff_sys::OutputFormatContext` available so the mux consumers compile for
// docs.rs while the real `format_context` module is cfg'd out.

#[derive(Debug)]
pub struct OutputFormatContext {
    _ptr: std::ptr::NonNull<AVFormatContext>,
}

impl OutputFormatContext {
    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn new(
        _format_name: Option<&str>,
        _filename: &std::path::Path,
    ) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const AVFormatContext {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVFormatContext {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn nb_streams(&self) -> u32 {
        0
    }

    #[must_use]
    pub fn oformat_flags(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn is_nofile(&self) -> bool {
        false
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn open_io(&mut self, _path: &std::path::Path) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn write_header(&mut self) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn write_trailer(&mut self) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    pub fn close_io(&mut self) {}

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn new_stream(&mut self, _codec: Option<&Codec>) -> Result<usize, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    #[must_use]
    pub fn stream_time_base(&self, _idx: usize) -> AVRational {
        AVRational { num: 0, den: 0 }
    }

    pub fn set_stream_time_base(&mut self, _idx: usize, _time_base: AVRational) {}

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn apply_stream_params_from_context(
        &mut self,
        _idx: usize,
        _ctx: &CodecContext,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn copy_stream_params(
        &mut self,
        _idx: usize,
        _src: CodecParameters<'_>,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn set_opt(
        &mut self,
        _key: &std::ffi::CStr,
        _value: &std::ffi::CStr,
    ) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn write_interleaved(&mut self, _pkt: &mut Packet) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn set_metadata(&mut self, _key: &str, _value: &str) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn add_attachment_stream(
        &mut self,
        _data: &[u8],
        _mime_type: &str,
        _filename: &str,
    ) -> Result<usize, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn set_chapters(&mut self, _chapters: &[ChapterSpec<'_>]) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }
}

impl Drop for OutputFormatContext {
    fn drop(&mut self) {}
}

// SAFETY: stub; never executed on docs.rs.
unsafe impl Send for OutputFormatContext {}

#[derive(Clone, Copy, Debug)]
pub struct ChapterSpec<'a> {
    pub id: i64,
    pub start_us: i64,
    pub end_us: i64,
    pub title: Option<&'a str>,
}

// ── Borrowed stream / params stubs (mirror crate::format_context) ─────────────

#[derive(Clone, Copy, Debug)]
pub struct StreamRef<'a> {
    _ptr: std::ptr::NonNull<AVStream>,
    _marker: std::marker::PhantomData<&'a InputFormatContext>,
}

impl<'a> StreamRef<'a> {
    #[must_use]
    pub fn index(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn time_base(&self) -> AVRational {
        AVRational { num: 0, den: 1 }
    }

    #[must_use]
    pub fn avg_frame_rate(&self) -> AVRational {
        AVRational { num: 0, den: 1 }
    }

    #[must_use]
    pub fn r_frame_rate(&self) -> AVRational {
        AVRational { num: 0, den: 1 }
    }

    #[must_use]
    pub fn nb_frames(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn codecpar(&self) -> CodecParameters<'a> {
        // SAFETY: stub; never executed on docs.rs.
        unsafe {
            CodecParameters {
                _ptr: std::ptr::NonNull::new_unchecked(ptr::null_mut()),
                _marker: std::marker::PhantomData,
            }
        }
    }

    #[must_use]
    pub fn duration(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn disposition(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn metadata(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CodecParameters<'a> {
    _ptr: std::ptr::NonNull<AVCodecParameters>,
    _marker: std::marker::PhantomData<&'a InputFormatContext>,
}

impl CodecParameters<'_> {
    #[must_use]
    pub fn codec_type(&self) -> AVMediaType {
        0
    }

    #[must_use]
    pub fn codec_id(&self) -> AVCodecID {
        0
    }

    #[must_use]
    pub fn width(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn height(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn sample_rate(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn color_space(&self) -> AVColorSpace {
        0
    }

    #[must_use]
    pub fn color_range(&self) -> AVColorRange {
        0
    }

    #[must_use]
    pub fn color_primaries(&self) -> AVColorPrimaries {
        0
    }

    #[must_use]
    pub fn ch_layout(&self) -> AVChannelLayout {
        AVChannelLayout::default()
    }

    #[must_use]
    pub fn format(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn bit_rate(&self) -> i64 {
        0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ChapterRef<'a> {
    _ptr: std::ptr::NonNull<AVChapter>,
    _marker: std::marker::PhantomData<&'a InputFormatContext>,
}

impl ChapterRef<'_> {
    #[must_use]
    pub fn id(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn time_base(&self) -> AVRational {
        AVRational { num: 0, den: 1 }
    }

    #[must_use]
    pub fn start(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn end(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn metadata(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }
}

// ── Owned Frame / Packet / Codec stubs (mirror crate::{frame,packet,codec}) ───
// Under DOCS_RS the real modules are cfg'd out; these shape-compatible stubs keep
// `ff_sys::{Frame, Packet, Codec}` available for docs.rs.

#[derive(Debug)]
pub struct Frame {
    _ptr: std::ptr::NonNull<AVFrame>,
}

impl Frame {
    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn new() -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const AVFrame {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVFrame {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn width(&self) -> c_int {
        0
    }
    pub fn set_width(&mut self, _width: c_int) {}
    #[must_use]
    pub fn height(&self) -> c_int {
        0
    }
    pub fn set_height(&mut self, _height: c_int) {}
    #[must_use]
    pub fn format(&self) -> c_int {
        0
    }
    pub fn set_format(&mut self, _format: c_int) {}
    pub fn set_pict_type(&mut self, _pict_type: AVPictureType) {}
    #[must_use]
    pub fn pts(&self) -> i64 {
        0
    }
    pub fn set_pts(&mut self, _pts: i64) {}
    #[must_use]
    pub fn pkt_dts(&self) -> i64 {
        0
    }
    pub fn set_pkt_dts(&mut self, _pkt_dts: i64) {}
    #[must_use]
    pub fn duration(&self) -> i64 {
        0
    }
    pub fn set_duration(&mut self, _duration: i64) {}
    #[must_use]
    pub fn time_base(&self) -> AVRational {
        AVRational { num: 0, den: 1 }
    }
    pub fn set_time_base(&mut self, _time_base: AVRational) {}
    #[must_use]
    pub fn nb_samples(&self) -> c_int {
        0
    }
    pub fn set_nb_samples(&mut self, _nb_samples: c_int) {}
    #[must_use]
    pub fn sample_rate(&self) -> c_int {
        0
    }
    pub fn set_sample_rate(&mut self, _sample_rate: c_int) {}
    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn set_ch_layout(&mut self, _layout: &AVChannelLayout) -> Result<(), crate::AvError> {
        Ok(())
    }
    #[must_use]
    pub fn channels(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn video_plane(&self, _i: usize) -> Option<&[u8]> {
        None
    }

    pub fn video_plane_mut(&mut self, _i: usize) -> Option<&mut [u8]> {
        None
    }

    #[must_use]
    pub fn linesize(&self, _i: usize) -> c_int {
        0
    }

    /// # Safety
    ///
    /// Docs.rs stub; never called.
    pub unsafe fn copy_plane_rows(
        &self,
        _i: usize,
        _dst: &mut [u8],
        _dst_stride: usize,
        _rows: usize,
        _row_bytes: usize,
    ) -> Option<()> {
        None
    }

    #[must_use]
    pub fn audio_plane(&self, _i: usize) -> Option<&[u8]> {
        None
    }

    pub fn audio_plane_mut(&mut self, _i: usize) -> Option<&mut [u8]> {
        None
    }

    pub fn unref(&mut self) {}

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn get_buffer(&mut self, _align: c_int) -> Result<(), crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    pub fn move_ref(&mut self, _src: &mut Frame) {}

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn try_clone(&self) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }
}

impl Drop for Frame {
    fn drop(&mut self) {}
}

// SAFETY: stub; never executed on docs.rs.
unsafe impl Send for Frame {}

#[derive(Debug)]
pub struct Packet {
    _ptr: std::ptr::NonNull<AVPacket>,
}

impl Packet {
    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn new() -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const AVPacket {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVPacket {
        self._ptr.as_ptr()
    }

    #[must_use]
    pub fn stream_index(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn pts(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn dts(&self) -> i64 {
        0
    }

    #[must_use]
    pub fn size(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn flags(&self) -> c_int {
        0
    }

    #[must_use]
    pub fn duration(&self) -> i64 {
        0
    }

    pub fn set_stream_index(&mut self, _stream_index: c_int) {}

    pub fn set_pts(&mut self, _pts: i64) {}

    pub fn set_dts(&mut self, _dts: i64) {}

    pub fn set_duration(&mut self, _duration: i64) {}

    pub fn rescale_ts(&mut self, _src_tb: AVRational, _dst_tb: AVRational) {}

    pub fn new_side_data(
        &mut self,
        _kind: AVPacketSideDataType,
        _size: usize,
    ) -> Option<&mut [u8]> {
        None
    }

    pub fn unref(&mut self) {}

    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn try_clone(&self) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }
}

impl Drop for Packet {
    fn drop(&mut self) {}
}

// SAFETY: stub; never executed on docs.rs.
unsafe impl Send for Packet {}

#[derive(Clone, Copy, Debug)]
pub struct Codec {
    _ptr: std::ptr::NonNull<AVCodec>,
}

impl Codec {
    #[must_use]
    pub fn find_decoder(_codec_id: AVCodecID) -> Option<Self> {
        None
    }

    #[must_use]
    pub fn find_encoder(_codec_id: AVCodecID) -> Option<Self> {
        None
    }

    #[must_use]
    pub fn find_encoder_by_name(_name: &str) -> Option<Self> {
        None
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const AVCodec {
        self._ptr.as_ptr()
    }
}

// ── RAII owner stub (mirrors crate::scale_context::ScaleContext) ──────────────
// Under DOCS_RS the real `scale_context` module is cfg'd out; this
// shape-compatible stub keeps `ff_sys::ScaleContext` available for docs.rs.

#[derive(Debug)]
pub struct ScaleContext {
    _ptr: std::ptr::NonNull<SwsContext>,
}

impl ScaleContext {
    /// # Errors
    /// Stub; never executed on docs.rs.
    pub fn new(
        _src_w: c_int,
        _src_h: c_int,
        _src_fmt: AVPixelFormat,
        _dst_w: c_int,
        _dst_h: c_int,
        _dst_fmt: AVPixelFormat,
        _flags: c_int,
    ) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn scale_frames(
        &mut self,
        _src: &Frame,
        _dst: &mut Frame,
    ) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn scale_planes(
        &mut self,
        _src: &[&[u8]],
        _src_strides: &[c_int],
        _src_h: c_int,
        _dst: &mut Frame,
    ) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn scale_slices(
        &mut self,
        _src: &[&[u8]],
        _src_strides: &[c_int],
        _src_h: c_int,
        _dst: &mut [&mut [u8]],
        _dst_strides: &[c_int],
    ) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn scale(
        &mut self,
        _src: *const *const u8,
        _src_stride: *const c_int,
        _src_slice_y: c_int,
        _src_slice_h: c_int,
        _dst: *const *mut u8,
        _dst_stride: *const c_int,
    ) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }
}

impl Drop for ScaleContext {
    fn drop(&mut self) {}
}

// SAFETY: stub; never executed on docs.rs.
unsafe impl Send for ScaleContext {}

// ── RAII owner stub (mirrors crate::resample_context::ResampleContext) ────────
// Under DOCS_RS the real `resample_context` module is cfg'd out; this
// shape-compatible stub keeps `ff_sys::ResampleContext` available for docs.rs.

#[derive(Debug)]
pub struct ResampleContext {
    _ptr: std::ptr::NonNull<SwrContext>,
}

impl ResampleContext {
    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn new(
        _out_ch_layout: *const AVChannelLayout,
        _out_sample_fmt: AVSampleFormat,
        _out_sample_rate: c_int,
        _in_ch_layout: *const AVChannelLayout,
        _in_sample_fmt: AVSampleFormat,
        _in_sample_rate: c_int,
    ) -> Result<Self, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn convert_into_planes(
        &mut self,
        _dst: &mut [&mut [u8]],
        _dst_count: c_int,
        _src: &Frame,
    ) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn convert_into_frame(
        &mut self,
        _dst: &mut Frame,
        _in_planes: &[&[u8]],
        _in_count: c_int,
    ) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn flush_into_frame(&mut self, _dst: &mut Frame) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    /// # Errors
    /// Stub; never executed on docs.rs.
    /// # Safety
    /// Stub; never executed on docs.rs.
    pub unsafe fn convert(
        &mut self,
        _out: *mut *mut u8,
        _out_count: c_int,
        _in_: *const *const u8,
        _in_count: c_int,
    ) -> Result<c_int, crate::AvError> {
        Err(crate::AvError::new(-1))
    }

    #[must_use]
    pub fn get_out_samples(&mut self, _in_samples: c_int) -> c_int {
        0
    }
}

impl Drop for ResampleContext {
    fn drop(&mut self) {}
}

// SAFETY: stub; never executed on docs.rs.
unsafe impl Send for ResampleContext {}

// ── Free-function stub (mirrors crate::buffersink) ────────────────────────────
// Under DOCS_RS the real `buffersink` module is cfg'd out; these shape-compatible
// stubs keep `ff_sys::BufferSinkOutcome` / `ff_sys::buffersink_get_frame`
// available for docs.rs.

/// Stub mirror of `crate::buffersink::BufferSinkOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferSinkOutcome {
    Frame,
    NeedMore,
    Drained,
}

/// # Safety
/// Stub; never executed on docs.rs.
pub unsafe fn buffersink_get_frame(
    _sink: *mut AVFilterContext,
    _dst: &mut Frame,
) -> Result<BufferSinkOutcome, crate::AvError> {
    Err(crate::AvError::new(-1))
}
