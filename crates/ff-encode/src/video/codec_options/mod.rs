//! Per-codec encoding options for [`VideoEncoderBuilder`](super::builder::VideoEncoderBuilder).
//!
//! Pass a [`VideoCodecOptions`] value to
//! `VideoEncoderBuilder::codec_options()` to control codec-specific behaviour.
//! Options are applied via `av_opt_set` / direct field assignment **before**
//! `avcodec_open2`.  Any option that the chosen encoder does not support is
//! logged as a warning and skipped — it never causes `build()` to return an
//! error.

mod av1;
mod dnxhd;
mod h264;
mod h265;
mod prores;
mod svt_av1;
mod vp9;

pub use av1::{Av1Options, Av1Usage};
pub use dnxhd::{DnxhdOptions, DnxhdVariant};
pub use h264::{H264Options, H264Preset, H264Profile, H264Tune};
pub use h265::{H265Options, H265Profile, H265Tier};
pub use prores::{ProResOptions, ProResProfile};
pub use svt_av1::SvtAv1Options;
pub use vp9::Vp9Options;

/// Per-codec encoding options.
///
/// The variant must match the codec passed to
/// `VideoEncoderBuilder::video_codec()`.  A mismatch is silently ignored
/// (the options are not applied).
///
/// All variants are fully implemented.
#[derive(Debug, Clone)]
pub enum VideoCodecOptions {
    /// H.264 (AVC) encoding options.
    H264(H264Options),
    /// H.265 (HEVC) encoding options.
    H265(H265Options),
    /// AV1 (libaom-av1) encoding options.
    Av1(Av1Options),
    /// AV1 (SVT-AV1 / libsvtav1) encoding options.
    Av1Svt(SvtAv1Options),
    /// VP9 encoding options (reserved for a future issue).
    Vp9(Vp9Options),
    /// Apple `ProRes` encoding options (reserved for a future issue).
    ProRes(ProResOptions),
    /// Avid `DNxHD` / `DNxHR` encoding options (reserved for a future issue).
    Dnxhd(DnxhdOptions),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_codec_options_enum_variants_are_accessible() {
        let _h264 = VideoCodecOptions::H264(H264Options::default());
        let _h265 = VideoCodecOptions::H265(H265Options::default());
        let _av1 = VideoCodecOptions::Av1(Av1Options::default());
        let _av1svt = VideoCodecOptions::Av1Svt(SvtAv1Options::default());
        let _vp9 = VideoCodecOptions::Vp9(Vp9Options::default());
        let _prores = VideoCodecOptions::ProRes(ProResOptions::default());
        let _dnxhd = VideoCodecOptions::Dnxhd(DnxhdOptions::default());
    }
}
