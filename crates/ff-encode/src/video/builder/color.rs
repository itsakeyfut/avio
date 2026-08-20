//! Color, HDR, and pixel format settings for [`VideoEncoderBuilder`].

use super::VideoEncoderBuilder;

impl VideoEncoderBuilder {
    /// Override the pixel format for video encoding.
    ///
    /// When omitted the encoder uses `yuv420p` by default, except that
    /// H.265 `Main10` automatically selects `yuv420p10le`.
    #[must_use]
    pub fn pixel_format(mut self, fmt: ff_format::PixelFormat) -> Self {
        self.pixel_format = Some(fmt);
        self
    }

    /// Embed HDR10 static metadata in the output.
    ///
    /// Sets `color_primaries = BT.2020`, `color_trc = SMPTE ST 2084 (PQ)`,
    /// and `colorspace = BT.2020 NCL` on the codec context, then attaches
    /// `AV_PKT_DATA_CONTENT_LIGHT_LEVEL` and
    /// `AV_PKT_DATA_MASTERING_DISPLAY_METADATA` packet side data to every
    /// keyframe.
    ///
    /// Pair with [`codec_options`](Self::codec_options) using
    /// `H265Options { profile: H265Profile::Main10, .. }`
    /// and [`pixel_format(PixelFormat::Yuv420p10le)`](Self::pixel_format) for a
    /// complete HDR10 pipeline.
    #[must_use]
    pub fn hdr10_metadata(mut self, meta: ff_format::Hdr10Metadata) -> Self {
        self.hdr10_metadata = Some(meta);
        self
    }

    /// Override the color space (matrix coefficients) written to the codec context.
    ///
    /// When omitted the encoder uses the `FFmpeg` default. HDR10 metadata, if set
    /// via [`hdr10_metadata()`](Self::hdr10_metadata), automatically selects
    /// BT.2020 NCL — this setter takes priority over that automatic choice.
    #[must_use]
    pub fn color_space(mut self, cs: ff_format::ColorSpace) -> Self {
        self.color_space = Some(cs);
        self
    }

    /// Override the color transfer characteristic (gamma curve) written to the codec context.
    ///
    /// When omitted the encoder uses the `FFmpeg` default. HDR10 metadata
    /// automatically selects PQ (SMPTE ST 2084) — this setter takes priority.
    /// Use [`ColorTransfer::Hlg`](ff_format::ColorTransfer::Hlg) for HLG broadcast HDR.
    #[must_use]
    pub fn color_transfer(mut self, trc: ff_format::ColorTransfer) -> Self {
        self.color_transfer = Some(trc);
        self
    }

    /// Override the color primaries written to the codec context.
    ///
    /// When omitted the encoder uses the `FFmpeg` default. HDR10 metadata
    /// automatically selects BT.2020 — this setter takes priority.
    #[must_use]
    pub fn color_primaries(mut self, cp: ff_format::ColorPrimaries) -> Self {
        self.color_primaries = Some(cp);
        self
    }
}
