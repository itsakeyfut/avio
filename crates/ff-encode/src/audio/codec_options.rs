//! Per-codec encoding options for [`AudioEncoderBuilder`](super::builder::AudioEncoderBuilder).
//!
//! Pass an [`AudioCodecOptions`] value to
//! `AudioEncoderBuilder::codec_options()` to control codec-specific behaviour.
//! Options are applied via `av_opt_set` / direct field assignment **before**
//! `avcodec_open2`.  Any option that the chosen encoder does not support is
//! logged as a warning and skipped — it never causes `build()` to return an
//! error.

/// Per-codec encoding options for audio.
///
/// The variant must match the codec passed to
/// `AudioEncoderBuilder::audio_codec()`.  A mismatch is silently ignored
/// (the options are not applied).
#[derive(Debug, Clone)]
pub enum AudioCodecOptions {
    /// Opus (libopus) encoding options.
    Opus(OpusOptions),
    /// AAC encoding options.
    Aac(AacOptions),
    /// MP3 (libmp3lame) encoding options.
    Mp3(Mp3Options),
    /// FLAC encoding options.
    Flac(FlacOptions),
}

// Opus

/// Opus (libopus) per-codec options.
#[derive(Debug, Clone)]
pub struct OpusOptions {
    /// Encoder application mode, optimised for the content type.
    pub application: OpusApplication,
    /// Frame duration in milliseconds.
    ///
    /// Must be one of `2`, `5`, `10`, `20`, `40`, or `60`.
    /// `None` uses the libopus default (20 ms).
    /// `build()` returns [`EncodeError::InvalidOption`](crate::EncodeError::InvalidOption)
    /// if the value is not in the allowed set.
    pub frame_duration_ms: Option<u32>,
}

impl Default for OpusOptions {
    fn default() -> Self {
        Self {
            application: OpusApplication::Audio,
            frame_duration_ms: None,
        }
    }
}

/// Opus encoder application mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpusApplication {
    /// Optimised for general audio (music, speech mix). Default.
    #[default]
    Audio,
    /// Optimised for `VoIP` / speech clarity at low bitrates.
    Voip,
    /// Minimum latency mode — disables lookahead.
    LowDelay,
}

impl OpusApplication {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Voip => "voip",
            Self::LowDelay => "lowdelay",
        }
    }
}

// AAC

/// AAC per-codec options.
#[derive(Debug, Clone)]
pub struct AacOptions {
    /// AAC encoding profile.
    ///
    /// `He` and `Hev2` typically require `libfdk_aac` (non-free). The built-in
    /// `FFmpeg` `aac` encoder may not support them — the failure is logged as a
    /// warning and encoding continues with the encoder's default profile.
    pub profile: AacProfile,
    /// VBR quality mode (1–5). `Some(q)` enables VBR; `None` uses CBR.
    ///
    /// Only supported by `libfdk_aac`. The built-in `aac` encoder ignores this
    /// option (logged as a warning). `build()` returns
    /// [`EncodeError::InvalidOption`](crate::EncodeError::InvalidOption) if the
    /// value is outside 1–5.
    pub vbr_quality: Option<u8>,
}

impl Default for AacOptions {
    fn default() -> Self {
        Self {
            profile: AacProfile::Lc,
            vbr_quality: None,
        }
    }
}

/// AAC encoding profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AacProfile {
    /// AAC-LC (Low Complexity) — widest compatibility. Default.
    #[default]
    Lc,
    /// HE-AAC v1 — with Spectral Band Replication (SBR). Typically requires `libfdk_aac`.
    He,
    /// HE-AAC v2 — with SBR + Parametric Stereo (PS). Typically requires `libfdk_aac`.
    Hev2,
}

impl AacProfile {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Lc => "aac_low",
            Self::He => "aac_he",
            Self::Hev2 => "aac_he_v2",
        }
    }
}

// MP3

/// MP3 quality mode: VBR quality scale or CBR fixed bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Quality {
    /// Variable bitrate — libmp3lame `q` scale (0 = best / V0, 9 = worst / V9).
    ///
    /// `build()` returns [`EncodeError::InvalidOption`](crate::EncodeError::InvalidOption)
    /// if the value exceeds 9.
    Vbr(u8),
    /// Constant bitrate in bits/sec (e.g. `128_000` for 128 kbps).
    Cbr(u32),
}

/// MP3 (libmp3lame) per-codec options.
#[derive(Debug, Clone)]
pub struct Mp3Options {
    /// VBR quality or CBR bitrate selection.
    pub quality: Mp3Quality,
}

impl Default for Mp3Options {
    fn default() -> Self {
        Self {
            quality: Mp3Quality::Vbr(4),
        }
    }
}

// FLAC

/// FLAC per-codec options.
#[derive(Debug, Clone)]
pub struct FlacOptions {
    /// Compression level (0–12). `0` = fastest / largest, `12` = slowest / smallest.
    pub compression_level: u8,
}

impl Default for FlacOptions {
    fn default() -> Self {
        Self {
            compression_level: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_application_should_return_correct_str() {
        assert_eq!(OpusApplication::Audio.as_str(), "audio");
        assert_eq!(OpusApplication::Voip.as_str(), "voip");
        assert_eq!(OpusApplication::LowDelay.as_str(), "lowdelay");
    }

    #[test]
    fn opus_options_default_should_have_audio_application_and_no_frame_duration() {
        let opts = OpusOptions::default();
        assert_eq!(opts.application, OpusApplication::Audio);
        assert!(opts.frame_duration_ms.is_none());
    }

    #[test]
    fn aac_profile_should_return_correct_str() {
        assert_eq!(AacProfile::Lc.as_str(), "aac_low");
        assert_eq!(AacProfile::He.as_str(), "aac_he");
        assert_eq!(AacProfile::Hev2.as_str(), "aac_he_v2");
    }

    #[test]
    fn aac_options_default_should_have_lc_profile_and_no_vbr() {
        let opts = AacOptions::default();
        assert_eq!(opts.profile, AacProfile::Lc);
        assert!(opts.vbr_quality.is_none());
    }

    #[test]
    fn mp3_options_default_should_have_vbr_quality_4() {
        let opts = Mp3Options::default();
        assert_eq!(opts.quality, Mp3Quality::Vbr(4));
    }

    #[test]
    fn mp3_quality_enum_variants_are_accessible() {
        let _vbr = Mp3Quality::Vbr(0);
        let _cbr = Mp3Quality::Cbr(192_000);
    }

    #[test]
    fn flac_options_default_should_have_compression_level_5() {
        let opts = FlacOptions::default();
        assert_eq!(opts.compression_level, 5);
    }

    #[test]
    fn audio_codec_options_enum_variants_are_accessible() {
        let _opus = AudioCodecOptions::Opus(OpusOptions::default());
        let _aac = AudioCodecOptions::Aac(AacOptions::default());
        let _mp3 = AudioCodecOptions::Mp3(Mp3Options::default());
        let _flac = AudioCodecOptions::Flac(FlacOptions::default());
    }
}
