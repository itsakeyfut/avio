//! Hardware encoder definitions.

use std::ffi::CString;
use std::sync::OnceLock;

/// Hardware encoder type.
///
/// Specifies which hardware acceleration to use for encoding.
/// Hardware encoding is generally faster and more power-efficient than software encoding,
/// but may have slightly lower quality at the same bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HardwareEncoder {
    /// Auto-detect available hardware encoder
    #[default]
    Auto,

    /// Software encoding only (no hardware acceleration)
    None,

    /// NVIDIA NVENC
    Nvenc,

    /// Intel Quick Sync Video
    Qsv,

    /// AMD Advanced Media Framework (AMF, formerly VCE)
    Amf,

    /// Apple `VideoToolbox`
    VideoToolbox,

    /// VA-API (Linux)
    Vaapi,
}

impl HardwareEncoder {
    /// Get the list of concrete hardware encoder backends available on this system.
    ///
    /// Returns only actual hardware backends (NVENC, QSV, AMF, `VideoToolbox`,
    /// VA-API). The control variants [`Auto`](Self::Auto) and [`None`](Self::None)
    /// are intentionally excluded — they are configuration options, not hardware
    /// backends.
    ///
    /// The result is cached on first call for performance.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ff_encode::HardwareEncoder;
    ///
    /// let backends = HardwareEncoder::available();
    /// if backends.is_empty() {
    ///     println!("No hardware encoders detected");
    /// } else {
    ///     for hw in backends {
    ///         println!("Available: {:?}", hw);
    ///     }
    /// }
    /// ```
    #[must_use]
    pub fn available() -> &'static [Self] {
        static AVAILABLE: OnceLock<Vec<HardwareEncoder>> = OnceLock::new();

        AVAILABLE.get_or_init(|| {
            let mut result = Vec::new();

            // Check each concrete hardware encoder type.
            // Auto and None are control variants, not hardware backends — excluded.
            if Self::Nvenc.is_available() {
                result.push(Self::Nvenc);
            }
            if Self::Qsv.is_available() {
                result.push(Self::Qsv);
            }
            if Self::Amf.is_available() {
                result.push(Self::Amf);
            }
            if Self::VideoToolbox.is_available() {
                result.push(Self::VideoToolbox);
            }
            if Self::Vaapi.is_available() {
                result.push(Self::Vaapi);
            }

            result
        })
    }

    /// Check if this hardware encoder is available.
    ///
    /// Queries `FFmpeg` to determine if the hardware encoder is available
    /// on the current system. This checks for both H.264 and H.265 support.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ff_encode::HardwareEncoder;
    ///
    /// if HardwareEncoder::Nvenc.is_available() {
    ///     println!("NVENC is available on this system");
    /// }
    /// ```
    #[must_use]
    pub fn is_available(self) -> bool {
        match self {
            // Auto and None are always available
            Self::Auto | Self::None => true,

            // Check hardware encoder availability
            Self::Nvenc => is_encoder_available("h264_nvenc") || is_encoder_available("hevc_nvenc"),
            Self::Qsv => is_encoder_available("h264_qsv") || is_encoder_available("hevc_qsv"),
            Self::Amf => is_encoder_available("h264_amf") || is_encoder_available("hevc_amf"),
            Self::VideoToolbox => {
                is_encoder_available("h264_videotoolbox")
                    || is_encoder_available("hevc_videotoolbox")
            }
            Self::Vaapi => is_encoder_available("h264_vaapi") || is_encoder_available("hevc_vaapi"),
        }
    }
}

/// Helper function to check if an encoder is available.
///
/// # Arguments
///
/// * `name` - The encoder name to check (e.g., "`h264_nvenc`", "`hevc_qsv`")
///
/// # Returns
///
/// Returns `true` if the encoder is available, `false` otherwise.
fn is_encoder_available(name: &str) -> bool {
    unsafe {
        ff_sys::ensure_initialized();

        let Ok(c_name) = CString::new(name) else {
            return false;
        };

        ff_sys::avcodec::find_encoder_by_name(c_name.as_ptr()).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_hardware_encoder() {
        assert_eq!(HardwareEncoder::default(), HardwareEncoder::Auto);
    }

    #[test]
    fn test_auto_and_none_always_available() {
        // Auto and None should always be available
        assert!(HardwareEncoder::Auto.is_available());
        assert!(HardwareEncoder::None.is_available());
    }

    #[test]
    fn available_should_contain_only_hardware_backends() {
        let available = HardwareEncoder::available();
        assert!(
            !available.contains(&HardwareEncoder::Auto),
            "Auto is a control variant, not a hardware backend"
        );
        assert!(
            !available.contains(&HardwareEncoder::None),
            "None is a control variant, not a hardware backend"
        );
        // May be empty on systems with no hardware encoders — that is correct.
    }

    #[test]
    fn test_hardware_encoder_availability() {
        // This test just checks that the functions don't panic
        // Actual availability depends on system hardware
        let _nvenc = HardwareEncoder::Nvenc.is_available();
        let _qsv = HardwareEncoder::Qsv.is_available();
        let _amf = HardwareEncoder::Amf.is_available();
        let _videotoolbox = HardwareEncoder::VideoToolbox.is_available();
        let _vaapi = HardwareEncoder::Vaapi.is_available();
    }
}
