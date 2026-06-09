//! List all available video encoders.
//!
//! This example shows which video encoders are available in the current FFmpeg build.

// Querying FFmpeg for available encoders requires unsafe FFI calls.
#![allow(unsafe_code)]

fn main() {
    unsafe {
        ff_sys::ensure_initialized();

        println!("Available video encoders:");
        println!("{:-<60}", "");

        let encoders = [
            // H.264 encoders
            ("h264", "H.264 (built-in)"),
            ("libx264", "H.264 (libx264)"),
            ("h264_nvenc", "H.264 (NVENC)"),
            ("h264_qsv", "H.264 (Quick Sync)"),
            ("h264_amf", "H.264 (AMF)"),
            ("h264_videotoolbox", "H.264 (VideoToolbox)"),
            // H.265 encoders
            ("hevc", "H.265 (built-in)"),
            ("libx265", "H.265 (libx265)"),
            ("hevc_nvenc", "H.265 (NVENC)"),
            ("hevc_qsv", "H.265 (Quick Sync)"),
            ("hevc_amf", "H.265 (AMF)"),
            ("hevc_videotoolbox", "H.265 (VideoToolbox)"),
            // Other codecs
            ("libvpx-vp9", "VP9"),
            ("libaom-av1", "AV1 (libaom)"),
            ("libsvtav1", "AV1 (SVT)"),
            ("mpeg4", "MPEG-4"),
            ("prores_ks", "ProRes (Kostya)"),
            ("prores", "ProRes"),
            ("dnxhd", "DNxHD"),
        ];

        for (name, description) in encoders {
            let c_name = std::ffi::CString::new(name).unwrap();
            let available = ff_sys::avcodec::find_encoder_by_name(c_name.as_ptr()).is_some();

            let status = if available { "✓" } else { "✗" };
            println!("{} {:20} - {}", status, name, description);
        }
    }
}
