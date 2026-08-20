//! Image encoder builder and public API.

use std::path::{Path, PathBuf};

use ff_format::{PixelFormat, VideoFrame};

use crate::EncodeError;

use super::encoder_inner;

/// Builder for [`ImageEncoder`].
///
/// Created via [`ImageEncoder::create`] or [`ImageEncoder::new`]. Extension
/// validation and zero-dimension checks happen in [`build`](ImageEncoderBuilder::build).
#[derive(Debug)]
pub struct ImageEncoderBuilder {
    path: PathBuf,
    width: Option<u32>,
    height: Option<u32>,
    quality: Option<u32>,
    pixel_format: Option<PixelFormat>,
}

impl ImageEncoderBuilder {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            width: None,
            height: None,
            quality: None,
            pixel_format: None,
        }
    }

    /// Override the output width in pixels.
    ///
    /// If not set, the source frame's width is used. If only `width` is set
    /// (without `height`), the source frame's height is preserved unchanged.
    #[must_use]
    pub fn width(mut self, w: u32) -> Self {
        self.width = Some(w);
        self
    }

    /// Override the output height in pixels.
    ///
    /// If not set, the source frame's height is used. If only `height` is set
    /// (without `width`), the source frame's width is preserved unchanged.
    #[must_use]
    pub fn height(mut self, h: u32) -> Self {
        self.height = Some(h);
        self
    }

    /// Set encoder quality on a 0–100 scale (100 = best quality).
    ///
    /// The value is mapped per codec:
    /// - **JPEG**: qscale 1–31 (100 → 1 = best, 0 → 31 = worst)
    /// - **PNG**: compression level 0–9 (100 → 9 = maximum compression)
    /// - **WebP**: quality 0–100 (direct mapping)
    /// - **BMP / TIFF**: no quality concept; the value is ignored with a warning
    #[must_use]
    pub fn quality(mut self, q: u32) -> Self {
        self.quality = Some(q);
        self
    }

    /// Override the output pixel format.
    ///
    /// If not set, a codec-native default is used (e.g. `YUVJ420P` for JPEG,
    /// `RGB24` for PNG). Setting an incompatible format may cause encoding to
    /// fail with an `FFmpeg` error.
    #[must_use]
    pub fn pixel_format(mut self, fmt: PixelFormat) -> Self {
        self.pixel_format = Some(fmt);
        self
    }

    /// Validate settings and return an [`ImageEncoder`].
    ///
    /// # Errors
    ///
    /// - [`EncodeError::InvalidConfig`] — path has no extension, or width/height is zero
    /// - [`EncodeError::UnsupportedCodec`] — extension is not a supported image format
    pub fn build(self) -> Result<ImageEncoder, EncodeError> {
        encoder_inner::codec_from_extension(&self.path)?;
        if let Some(0) = self.width {
            return Err(EncodeError::InvalidConfig {
                reason: "width must be non-zero".to_string(),
            });
        }
        if let Some(0) = self.height {
            return Err(EncodeError::InvalidConfig {
                reason: "height must be non-zero".to_string(),
            });
        }
        Ok(ImageEncoder {
            path: self.path,
            width: self.width,
            height: self.height,
            quality: self.quality,
            pixel_format: self.pixel_format,
        })
    }
}

/// Encodes a single [`VideoFrame`] to a still image file.
///
/// The output format is inferred from the file extension: `.jpg`/`.jpeg`,
/// `.png`, `.bmp`, `.tif`/`.tiff`, or `.webp`.
///
/// # Example
///
/// ```ignore
/// use ff_encode::ImageEncoder;
/// use ff_format::PixelFormat;
///
/// let encoder = ImageEncoder::create("thumbnail.jpg")
///     .width(320)
///     .height(240)
///     .quality(85)
///     .build()?;
/// encoder.encode(&frame)?;
/// ```
#[derive(Debug)]
pub struct ImageEncoder {
    path: PathBuf,
    width: Option<u32>,
    height: Option<u32>,
    quality: Option<u32>,
    pixel_format: Option<PixelFormat>,
}

impl ImageEncoder {
    /// Start building an image encoder that writes to `path`.
    ///
    /// This is infallible; extension validation happens in
    /// [`ImageEncoderBuilder::build`].
    pub fn create(path: impl AsRef<Path>) -> ImageEncoderBuilder {
        ImageEncoderBuilder::new(path.as_ref().to_path_buf())
    }

    /// Alias for [`create`](ImageEncoder::create).
    #[allow(clippy::new_ret_no_self)]
    pub fn new(path: impl AsRef<Path>) -> ImageEncoderBuilder {
        ImageEncoderBuilder::new(path.as_ref().to_path_buf())
    }

    /// Encode `frame` and write it to the output file.
    ///
    /// If `width` or `height` were set on the builder and differ from the
    /// source frame dimensions, swscale is used to resize. If `pixel_format`
    /// was set and differs from the frame format, swscale performs conversion.
    ///
    /// # Errors
    ///
    /// Returns an error if the `FFmpeg` encoder is unavailable, the output file
    /// cannot be created, or encoding fails.
    pub fn encode(self, frame: &VideoFrame) -> Result<(), EncodeError> {
        let opts = encoder_inner::ImageEncodeOptions {
            width: self.width,
            height: self.height,
            quality: self.quality,
            pixel_format: self.pixel_format,
        };
        encoder_inner::encode_image(&self.path, frame, &opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_should_return_builder() {
        let _builder = ImageEncoder::create("out.png");
    }

    #[test]
    fn new_should_return_builder() {
        let _builder = ImageEncoder::new("out.png");
    }

    #[test]
    fn build_with_unsupported_extension_should_return_error() {
        let result = ImageEncoder::create("out.avi").build();
        assert!(
            matches!(result, Err(EncodeError::UnsupportedCodec { .. })),
            "expected UnsupportedCodec, got {result:?}"
        );
    }

    #[test]
    fn build_with_no_extension_should_return_error() {
        let result = ImageEncoder::create("out_no_ext").build();
        assert!(
            matches!(result, Err(EncodeError::InvalidConfig { .. })),
            "expected InvalidConfig, got {result:?}"
        );
    }

    #[test]
    fn build_with_zero_width_should_return_error() {
        let result = ImageEncoder::create("out.png").width(0).build();
        assert!(
            matches!(result, Err(EncodeError::InvalidConfig { .. })),
            "expected InvalidConfig for zero width, got {result:?}"
        );
    }

    #[test]
    fn build_with_zero_height_should_return_error() {
        let result = ImageEncoder::create("out.png").height(0).build();
        assert!(
            matches!(result, Err(EncodeError::InvalidConfig { .. })),
            "expected InvalidConfig for zero height, got {result:?}"
        );
    }

    #[test]
    fn width_setter_should_store_value() {
        let encoder = ImageEncoder::create("out.png").width(320).build().unwrap();
        assert_eq!(encoder.width, Some(320));
    }

    #[test]
    fn height_setter_should_store_value() {
        let encoder = ImageEncoder::create("out.png").height(240).build().unwrap();
        assert_eq!(encoder.height, Some(240));
    }

    #[test]
    fn quality_setter_should_store_value() {
        let encoder = ImageEncoder::create("out.png").quality(75).build().unwrap();
        assert_eq!(encoder.quality, Some(75));
    }

    #[test]
    fn pixel_format_setter_should_store_value() {
        let encoder = ImageEncoder::create("out.png")
            .pixel_format(PixelFormat::Rgb24)
            .build()
            .unwrap();
        assert_eq!(encoder.pixel_format, Some(PixelFormat::Rgb24));
    }

    #[test]
    fn build_with_only_width_should_succeed() {
        // Partial dimensions are valid — height falls back to frame dimension.
        let result = ImageEncoder::create("out.png").width(128).build();
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn build_with_all_options_should_succeed() {
        let result = ImageEncoder::create("out.jpg")
            .width(320)
            .height(240)
            .quality(80)
            .pixel_format(PixelFormat::Yuv420p)
            .build();
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }
}
