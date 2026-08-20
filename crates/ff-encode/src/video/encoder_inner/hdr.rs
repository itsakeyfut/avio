//! HDR10 side-data helpers.
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
    AVPacket, AVPacketSideDataType_AV_PKT_DATA_CONTENT_LIGHT_LEVEL,
    AVPacketSideDataType_AV_PKT_DATA_MASTERING_DISPLAY_METADATA, VideoEncoderInner,
    av_packet_new_side_data,
};

/// FFmpeg `AVContentLightMetadata` — matches the C struct layout.
///
/// Not exposed by bindgen, so defined here with field order/types matching
/// the FFmpeg 7.x header `libavutil/mastering_display_metadata.h`.
#[repr(C)]
pub(super) struct AvContentLightMetadata {
    /// Maximum Content Light Level (nits). Corresponds to `MaxCLL` in C.
    pub(super) max_cll: u32,
    /// Maximum Frame-Average Light Level (nits). Corresponds to `MaxFALL` in C.
    pub(super) max_fall: u32,
}

/// FFmpeg `AVMasteringDisplayMetadata` — matches the C struct layout.
///
/// Not exposed by bindgen, so defined here with the exact field names and
/// types from the FFmpeg 7.x header `libavutil/mastering_display_metadata.h`.
#[repr(C)]
pub(super) struct AvMasteringDisplayMetadata {
    /// Chromaticity coordinates of the source primaries:
    /// `display_primaries[R=0/G=1/B=2][x=0/y=1]`.
    pub(super) display_primaries: [[ff_sys::AVRational; 2]; 3],
    /// White point chromaticity: `white_point[x=0/y=1]`.
    pub(super) white_point: [ff_sys::AVRational; 2],
    /// Minimum display luminance (AVRational with denominator 10000).
    pub(super) min_luminance: ff_sys::AVRational,
    /// Maximum display luminance (AVRational with denominator 10000).
    pub(super) max_luminance: ff_sys::AVRational,
    /// Flag: 1 if `display_primaries` is set, 0 otherwise.
    pub(super) has_primaries: std::os::raw::c_int,
    /// Flag: 1 if `min_luminance`/`max_luminance` are set, 0 otherwise.
    pub(super) has_luminance: std::os::raw::c_int,
}

impl VideoEncoderInner {
    /// Attach HDR10 static metadata as packet side data to a keyframe packet.
    ///
    /// Attaches `AV_PKT_DATA_CONTENT_LIGHT_LEVEL` (MaxCLL/MaxFALL) and
    /// `AV_PKT_DATA_MASTERING_DISPLAY_METADATA` to `pkt`.  Called from
    /// `receive_packets` for every keyframe when `hdr10_metadata` is set.
    ///
    /// # Safety
    ///
    /// `pkt` must be a valid, non-null pointer to an allocated `AVPacket`.
    pub(super) unsafe fn attach_hdr10_side_data(
        &self,
        pkt: *mut AVPacket,
        meta: &ff_format::Hdr10Metadata,
    ) {
        // ── Content light level (MaxCLL / MaxFALL) ──────────────────────────
        let cll_ptr = av_packet_new_side_data(
            pkt,
            AVPacketSideDataType_AV_PKT_DATA_CONTENT_LIGHT_LEVEL,
            std::mem::size_of::<AvContentLightMetadata>(),
        );
        if cll_ptr.is_null() {
            log::warn!(
                "hdr10 side_data allocation failed, skipping MaxCLL/MaxFALL type=content_light_level"
            );
        } else {
            // SAFETY: av_packet_new_side_data returns memory allocated for exactly
            // sizeof(AvContentLightMetadata) bytes with alignment suitable for the
            // data type. We use ptr::write to avoid creating a reference to
            // potentially-unaligned memory (ptr::write handles any alignment).
            let cll_value = AvContentLightMetadata {
                max_cll: u32::from(meta.max_cll),
                max_fall: u32::from(meta.max_fall),
            };
            // SAFETY: write_unaligned does not require the pointer to be aligned;
            // it copies bytes. FFmpeg's av_packet_new_side_data returns
            // malloc-aligned memory in practice, but write_unaligned is correct
            // regardless of alignment.
            std::ptr::write_unaligned(cll_ptr.cast::<AvContentLightMetadata>(), cll_value);
        }

        // ── Mastering display colour volume ─────────────────────────────────
        let md_ptr = av_packet_new_side_data(
            pkt,
            AVPacketSideDataType_AV_PKT_DATA_MASTERING_DISPLAY_METADATA,
            std::mem::size_of::<AvMasteringDisplayMetadata>(),
        );
        if md_ptr.is_null() {
            log::warn!(
                "hdr10 side_data allocation failed, skipping mastering display type=mastering_display_metadata"
            );
        } else {
            let d = &meta.mastering_display;
            let r = |n: u16| ff_sys::AVRational {
                num: i32::from(n),
                den: 50000,
            };
            let lum = |n: u32| ff_sys::AVRational {
                num: i32::try_from(n).unwrap_or(i32::MAX),
                den: 10000,
            };
            // SAFETY: av_packet_new_side_data returns memory allocated for exactly
            // sizeof(AvMasteringDisplayMetadata) bytes. We use ptr::write to safely
            // write the struct without creating a potentially-unaligned reference.
            let md_value = AvMasteringDisplayMetadata {
                // Chromaticity coordinates: denominator = 50000 (CIE 1931 xy).
                // Order: [R, G, B][x, y]
                display_primaries: [
                    [r(d.red_x), r(d.red_y)],
                    [r(d.green_x), r(d.green_y)],
                    [r(d.blue_x), r(d.blue_y)],
                ],
                white_point: [r(d.white_x), r(d.white_y)],
                // Luminance: denominator = 10000.
                min_luminance: lum(d.min_luminance),
                max_luminance: lum(d.max_luminance),
                has_primaries: 1,
                has_luminance: 1,
            };
            // SAFETY: write_unaligned does not require the pointer to be aligned;
            // it copies bytes. FFmpeg's av_packet_new_side_data returns
            // malloc-aligned memory in practice, but write_unaligned is correct
            // regardless of alignment.
            std::ptr::write_unaligned(md_ptr.cast::<AvMasteringDisplayMetadata>(), md_value);
        }
    }
}
