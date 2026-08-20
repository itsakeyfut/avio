//! SVT-AV1 (libsvtav1) per-codec encoding options.

/// SVT-AV1 (libsvtav1) per-codec options.
///
/// **Note**: Requires an `FFmpeg` build with `--enable-libsvtav1` (LGPL).
/// `build()` returns [`EncodeError::EncoderUnavailable`](crate::EncodeError::EncoderUnavailable)
/// when libsvtav1 is absent from the `FFmpeg` build.
#[derive(Debug, Clone)]
pub struct SvtAv1Options {
    /// Encoder preset: 0 = best quality / slowest, 13 = fastest / lowest quality.
    pub preset: u8,
    /// Log2 number of tile rows (0–6).
    pub tile_rows: u8,
    /// Log2 number of tile columns (0–6).
    pub tile_cols: u8,
    /// Raw SVT-AV1 params string passed verbatim (e.g. `"fast-decode=1:hdr=0"`).
    ///
    /// `None` leaves the encoder default. Invalid values are logged as a warning
    /// and skipped — `build()` never fails due to an unsupported parameter.
    pub svtav1_params: Option<String>,
}

impl Default for SvtAv1Options {
    fn default() -> Self {
        Self {
            preset: 8,
            tile_rows: 0,
            tile_cols: 0,
            svtav1_params: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svtav1_options_default_should_have_preset_8() {
        let opts = SvtAv1Options::default();
        assert_eq!(opts.preset, 8);
        assert_eq!(opts.tile_rows, 0);
        assert_eq!(opts.tile_cols, 0);
        assert!(opts.svtav1_params.is_none());
    }
}
