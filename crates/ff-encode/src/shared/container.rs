//! Container format definitions.

/// Output container format for encoding.
///
/// The container format is usually auto-detected from the file extension,
/// but can be explicitly specified if needed.
///
/// Named `OutputContainer` (rather than `Container`) to avoid confusion with
/// `ff_format::ContainerInfo`, which describes a container *read* from a probed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputContainer {
    /// MP4 / `QuickTime`
    Mp4,

    /// Fragmented MP4 — CMAF-compatible streaming container.
    ///
    /// Uses the same `mp4` `FFmpeg` muxer as [`OutputContainer::Mp4`] but with
    /// `movflags=+frag_keyframe+empty_moov+default_base_moof` applied before
    /// writing the header. Required for HTTP Live Streaming fMP4 segments
    /// (CMAF) and MPEG-DASH.
    FMp4,

    /// `WebM`
    WebM,

    /// Matroska
    Mkv,

    /// AVI
    Avi,

    /// MOV
    Mov,

    /// FLAC (lossless audio container)
    Flac,

    /// OGG (audio container for Vorbis/Opus)
    Ogg,
}

impl OutputContainer {
    /// Get `FFmpeg` format name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mp4 | Self::FMp4 => "mp4",
            Self::WebM => "webm",
            Self::Mkv => "matroska",
            Self::Avi => "avi",
            Self::Mov => "mov",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
        }
    }

    /// Get default file extension.
    #[must_use]
    pub const fn default_extension(self) -> &'static str {
        match self {
            Self::Mp4 | Self::FMp4 => "mp4",
            Self::WebM => "webm",
            Self::Mkv => "mkv",
            Self::Avi => "avi",
            Self::Mov => "mov",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
        }
    }

    /// Guess the container from a file path's extension.
    ///
    /// Mirrors [`default_extension`](Self::default_extension) in reverse and is
    /// case-insensitive. Returns `None` for an unknown or missing extension. The
    /// `mp4` extension maps to progressive [`Mp4`](Self::Mp4), never fragmented
    /// [`FMp4`](Self::FMp4), which must be selected explicitly.
    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "mp4" | "m4v" => Some(Self::Mp4),
            "webm" => Some(Self::WebM),
            "mkv" => Some(Self::Mkv),
            "avi" => Some(Self::Avi),
            "mov" => Some(Self::Mov),
            "flac" => Some(Self::Flac),
            "ogg" => Some(Self::Ogg),
            _ => None,
        }
    }

    /// Returns `true` if this container is fragmented MP4.
    ///
    /// When `true`, the encoder applies
    /// `movflags=+frag_keyframe+empty_moov+default_base_moof` before writing
    /// the file header, enabling CMAF-compatible streaming output.
    #[must_use]
    pub const fn is_fragmented(self) -> bool {
        matches!(self, Self::FMp4)
    }

    /// Returns `true` if this container supports the `+faststart` movflag.
    ///
    /// `faststart` (moving the `moov` atom to the front for progressive
    /// download) is specific to the `mp4`/`mov` muxers, so it applies only to
    /// [`Mp4`](Self::Mp4) and [`Mov`](Self::Mov). Fragmented MP4
    /// ([`FMp4`](Self::FMp4)) already streams and uses its own movflags, so it
    /// is excluded.
    #[must_use]
    pub const fn supports_faststart(self) -> bool {
        matches!(self, Self::Mp4 | Self::Mov)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_as_str() {
        assert_eq!(OutputContainer::Mp4.as_str(), "mp4");
        assert_eq!(OutputContainer::WebM.as_str(), "webm");
        assert_eq!(OutputContainer::Mkv.as_str(), "matroska");
    }

    #[test]
    fn test_container_extension() {
        assert_eq!(OutputContainer::Mp4.default_extension(), "mp4");
        assert_eq!(OutputContainer::WebM.default_extension(), "webm");
        assert_eq!(OutputContainer::Mkv.default_extension(), "mkv");
        assert_eq!(OutputContainer::Flac.default_extension(), "flac");
        assert_eq!(OutputContainer::Ogg.default_extension(), "ogg");
    }

    #[test]
    fn flac_as_str_should_return_flac() {
        assert_eq!(OutputContainer::Flac.as_str(), "flac");
    }

    #[test]
    fn ogg_as_str_should_return_ogg() {
        assert_eq!(OutputContainer::Ogg.as_str(), "ogg");
    }

    #[test]
    fn fmp4_as_str_should_return_mp4() {
        assert_eq!(OutputContainer::FMp4.as_str(), "mp4");
    }

    #[test]
    fn fmp4_extension_should_return_mp4() {
        assert_eq!(OutputContainer::FMp4.default_extension(), "mp4");
    }

    #[test]
    fn fmp4_is_fragmented_should_return_true() {
        assert!(OutputContainer::FMp4.is_fragmented());
        assert!(!OutputContainer::Mp4.is_fragmented());
        assert!(!OutputContainer::Mkv.is_fragmented());
    }

    #[test]
    fn from_path_should_map_known_extensions_case_insensitively() {
        use std::path::Path;
        assert_eq!(
            OutputContainer::from_path(Path::new("out.mp4")),
            Some(OutputContainer::Mp4)
        );
        // Case-insensitive, and `mp4` is progressive Mp4 (never FMp4).
        assert_eq!(
            OutputContainer::from_path(Path::new("OUT.MP4")),
            Some(OutputContainer::Mp4)
        );
        assert_eq!(
            OutputContainer::from_path(Path::new("clip.mov")),
            Some(OutputContainer::Mov)
        );
        assert_eq!(
            OutputContainer::from_path(Path::new("clip.mkv")),
            Some(OutputContainer::Mkv)
        );
        assert_eq!(OutputContainer::from_path(Path::new("clip.xyz")), None);
        assert_eq!(OutputContainer::from_path(Path::new("noext")), None);
    }

    #[test]
    fn supports_faststart_should_be_true_for_mp4_and_mov() {
        assert!(OutputContainer::Mp4.supports_faststart());
        assert!(OutputContainer::Mov.supports_faststart());
        // Fragmented MP4 streams already and uses its own movflags.
        assert!(!OutputContainer::FMp4.supports_faststart());
        assert!(!OutputContainer::WebM.supports_faststart());
        assert!(!OutputContainer::Mkv.supports_faststart());
    }
}
