//! Video stream info and builder.

use std::time::Duration;

use crate::codec::VideoCodec;
use crate::color::{ColorPrimaries, ColorRange, ColorSpace};
use crate::pixel::PixelFormat;
use crate::time::Rational;

/// Information about a video stream within a media file.
///
/// This struct contains all metadata needed to understand and process
/// a video stream, including resolution, codec, frame rate, and color
/// characteristics.
///
/// # Construction
///
/// Use [`VideoStreamInfo::builder()`] for fluent construction:
///
/// ```
/// use ff_format::stream::VideoStreamInfo;
/// use ff_format::{PixelFormat, Rational};
/// use ff_format::codec::VideoCodec;
///
/// let info = VideoStreamInfo::builder()
///     .index(0)
///     .codec(VideoCodec::H264)
///     .width(1920)
///     .height(1080)
///     .frame_rate(Rational::new(30, 1))
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct VideoStreamInfo {
    /// Stream index within the container
    index: u32,
    /// Video codec
    codec: VideoCodec,
    /// Codec name as reported by the demuxer
    codec_name: String,
    /// Frame width in pixels
    width: u32,
    /// Frame height in pixels
    height: u32,
    /// Pixel format
    pixel_format: PixelFormat,
    /// Frame rate (frames per second)
    frame_rate: Rational,
    /// Stream duration (if known)
    duration: Option<Duration>,
    /// Bitrate in bits per second (if known)
    bitrate: Option<u64>,
    /// Total number of frames (if known)
    frame_count: Option<u64>,
    /// Color space (matrix coefficients)
    color_space: ColorSpace,
    /// Color range (limited/full)
    color_range: ColorRange,
    /// Color primaries
    color_primaries: ColorPrimaries,
}

impl VideoStreamInfo {
    /// Creates a new builder for constructing `VideoStreamInfo`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::stream::VideoStreamInfo;
    /// use ff_format::codec::VideoCodec;
    /// use ff_format::{PixelFormat, Rational};
    ///
    /// let info = VideoStreamInfo::builder()
    ///     .index(0)
    ///     .codec(VideoCodec::H264)
    ///     .width(1920)
    ///     .height(1080)
    ///     .frame_rate(Rational::new(30, 1))
    ///     .build();
    /// ```
    #[must_use]
    pub fn builder() -> VideoStreamInfoBuilder {
        VideoStreamInfoBuilder::default()
    }

    /// Returns the stream index within the container.
    #[must_use]
    #[inline]
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// Returns the video codec.
    #[must_use]
    #[inline]
    pub const fn codec(&self) -> VideoCodec {
        self.codec
    }

    /// Returns the codec name as reported by the demuxer.
    #[must_use]
    #[inline]
    pub fn codec_name(&self) -> &str {
        &self.codec_name
    }

    /// Returns the frame width in pixels.
    #[must_use]
    #[inline]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the frame height in pixels.
    #[must_use]
    #[inline]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the pixel format.
    #[must_use]
    #[inline]
    pub const fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// Returns the frame rate as a rational number.
    #[must_use]
    #[inline]
    pub const fn frame_rate(&self) -> Rational {
        self.frame_rate
    }

    /// Returns the frame rate as frames per second (f64).
    #[must_use]
    #[inline]
    pub fn fps(&self) -> f64 {
        self.frame_rate.as_f64()
    }

    /// Returns the stream duration, if known.
    #[must_use]
    #[inline]
    pub const fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Returns the bitrate in bits per second, if known.
    #[must_use]
    #[inline]
    pub const fn bitrate(&self) -> Option<u64> {
        self.bitrate
    }

    /// Returns the total number of frames, if known.
    #[must_use]
    #[inline]
    pub const fn frame_count(&self) -> Option<u64> {
        self.frame_count
    }

    /// Returns the color space (matrix coefficients).
    #[must_use]
    #[inline]
    pub const fn color_space(&self) -> ColorSpace {
        self.color_space
    }

    /// Returns the color range (limited/full).
    #[must_use]
    #[inline]
    pub const fn color_range(&self) -> ColorRange {
        self.color_range
    }

    /// Returns the color primaries.
    #[must_use]
    #[inline]
    pub const fn color_primaries(&self) -> ColorPrimaries {
        self.color_primaries
    }

    /// Returns the aspect ratio as width/height.
    #[must_use]
    #[inline]
    pub fn aspect_ratio(&self) -> f64 {
        if self.height == 0 {
            log::warn!(
                "aspect_ratio unavailable, height is 0, returning 0.0 \
                 width={} height=0 fallback=0.0",
                self.width
            );
            0.0
        } else {
            f64::from(self.width) / f64::from(self.height)
        }
    }

    /// Returns `true` if the video is HD (720p or higher).
    #[must_use]
    #[inline]
    pub const fn is_hd(&self) -> bool {
        self.height >= 720
    }

    /// Returns `true` if the video is Full HD (1080p or higher).
    #[must_use]
    #[inline]
    pub const fn is_full_hd(&self) -> bool {
        self.height >= 1080
    }

    /// Returns `true` if the video is 4K UHD (2160p or higher).
    #[must_use]
    #[inline]
    pub const fn is_4k(&self) -> bool {
        self.height >= 2160
    }

    /// Returns `true` if this video stream appears to be HDR (High Dynamic Range).
    ///
    /// HDR detection is based on two primary indicators:
    /// 1. **Wide color gamut**: BT.2020 color primaries
    /// 2. **High bit depth**: 10-bit or higher pixel format
    ///
    /// Both conditions must be met for a stream to be considered HDR.
    /// This is a heuristic detection - for definitive HDR identification,
    /// additional metadata like transfer characteristics (PQ/HLG) should be checked.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::stream::VideoStreamInfo;
    /// use ff_format::color::ColorPrimaries;
    /// use ff_format::PixelFormat;
    ///
    /// let hdr_video = VideoStreamInfo::builder()
    ///     .width(3840)
    ///     .height(2160)
    ///     .color_primaries(ColorPrimaries::Bt2020)
    ///     .pixel_format(PixelFormat::Yuv420p10le)
    ///     .build();
    ///
    /// assert!(hdr_video.is_hdr());
    ///
    /// // Standard HD video with BT.709 is not HDR
    /// let sdr_video = VideoStreamInfo::builder()
    ///     .width(1920)
    ///     .height(1080)
    ///     .color_primaries(ColorPrimaries::Bt709)
    ///     .pixel_format(PixelFormat::Yuv420p)
    ///     .build();
    ///
    /// assert!(!sdr_video.is_hdr());
    /// ```
    #[must_use]
    #[inline]
    pub fn is_hdr(&self) -> bool {
        // HDR requires wide color gamut (BT.2020) and high bit depth (10-bit or higher)
        self.color_primaries.is_wide_gamut() && self.pixel_format.is_high_bit_depth()
    }
}

impl Default for VideoStreamInfo {
    fn default() -> Self {
        Self {
            index: 0,
            codec: VideoCodec::default(),
            codec_name: String::new(),
            width: 0,
            height: 0,
            pixel_format: PixelFormat::default(),
            frame_rate: Rational::new(30, 1),
            duration: None,
            bitrate: None,
            frame_count: None,
            color_space: ColorSpace::default(),
            color_range: ColorRange::default(),
            color_primaries: ColorPrimaries::default(),
        }
    }
}

/// Builder for constructing `VideoStreamInfo`.
#[derive(Debug, Clone, Default)]
pub struct VideoStreamInfoBuilder {
    index: u32,
    codec: VideoCodec,
    codec_name: String,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    frame_rate: Rational,
    duration: Option<Duration>,
    bitrate: Option<u64>,
    frame_count: Option<u64>,
    color_space: ColorSpace,
    color_range: ColorRange,
    color_primaries: ColorPrimaries,
}

impl VideoStreamInfoBuilder {
    /// Sets the stream index.
    #[must_use]
    pub fn index(mut self, index: u32) -> Self {
        self.index = index;
        self
    }

    /// Sets the video codec.
    #[must_use]
    pub fn codec(mut self, codec: VideoCodec) -> Self {
        self.codec = codec;
        self
    }

    /// Sets the codec name string.
    #[must_use]
    pub fn codec_name(mut self, name: impl Into<String>) -> Self {
        self.codec_name = name.into();
        self
    }

    /// Sets the frame width in pixels.
    #[must_use]
    pub fn width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    /// Sets the frame height in pixels.
    #[must_use]
    pub fn height(mut self, height: u32) -> Self {
        self.height = height;
        self
    }

    /// Sets the pixel format.
    #[must_use]
    pub fn pixel_format(mut self, format: PixelFormat) -> Self {
        self.pixel_format = format;
        self
    }

    /// Sets the frame rate.
    #[must_use]
    pub fn frame_rate(mut self, rate: Rational) -> Self {
        self.frame_rate = rate;
        self
    }

    /// Sets the stream duration.
    #[must_use]
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Sets the bitrate in bits per second.
    #[must_use]
    pub fn bitrate(mut self, bitrate: u64) -> Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// Sets the total frame count.
    #[must_use]
    pub fn frame_count(mut self, count: u64) -> Self {
        self.frame_count = Some(count);
        self
    }

    /// Sets the color space.
    #[must_use]
    pub fn color_space(mut self, space: ColorSpace) -> Self {
        self.color_space = space;
        self
    }

    /// Sets the color range.
    #[must_use]
    pub fn color_range(mut self, range: ColorRange) -> Self {
        self.color_range = range;
        self
    }

    /// Sets the color primaries.
    #[must_use]
    pub fn color_primaries(mut self, primaries: ColorPrimaries) -> Self {
        self.color_primaries = primaries;
        self
    }

    /// Builds the `VideoStreamInfo`.
    #[must_use]
    pub fn build(self) -> VideoStreamInfo {
        VideoStreamInfo {
            index: self.index,
            codec: self.codec,
            codec_name: self.codec_name,
            width: self.width,
            height: self.height,
            pixel_format: self.pixel_format,
            frame_rate: self.frame_rate,
            duration: self.duration,
            bitrate: self.bitrate,
            frame_count: self.frame_count,
            color_space: self.color_space,
            color_range: self.color_range,
            color_primaries: self.color_primaries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let info = VideoStreamInfo::builder()
            .index(0)
            .codec(VideoCodec::H264)
            .codec_name("h264")
            .width(1920)
            .height(1080)
            .frame_rate(Rational::new(30, 1))
            .pixel_format(PixelFormat::Yuv420p)
            .build();

        assert_eq!(info.index(), 0);
        assert_eq!(info.codec(), VideoCodec::H264);
        assert_eq!(info.codec_name(), "h264");
        assert_eq!(info.width(), 1920);
        assert_eq!(info.height(), 1080);
        assert!((info.fps() - 30.0).abs() < 0.001);
        assert_eq!(info.pixel_format(), PixelFormat::Yuv420p);
    }

    #[test]
    fn test_builder_full() {
        let info = VideoStreamInfo::builder()
            .index(0)
            .codec(VideoCodec::H265)
            .codec_name("hevc")
            .width(3840)
            .height(2160)
            .frame_rate(Rational::new(60, 1))
            .pixel_format(PixelFormat::Yuv420p10le)
            .duration(Duration::from_secs(120))
            .bitrate(50_000_000)
            .frame_count(7200)
            .color_space(ColorSpace::Bt2020Ncl)
            .color_range(ColorRange::Full)
            .color_primaries(ColorPrimaries::Bt2020)
            .build();

        assert_eq!(info.codec(), VideoCodec::H265);
        assert_eq!(info.width(), 3840);
        assert_eq!(info.height(), 2160);
        assert_eq!(info.duration(), Some(Duration::from_secs(120)));
        assert_eq!(info.bitrate(), Some(50_000_000));
        assert_eq!(info.frame_count(), Some(7200));
        assert_eq!(info.color_space(), ColorSpace::Bt2020Ncl);
        assert_eq!(info.color_range(), ColorRange::Full);
        assert_eq!(info.color_primaries(), ColorPrimaries::Bt2020);
    }

    #[test]
    fn test_default() {
        let info = VideoStreamInfo::default();
        assert_eq!(info.index(), 0);
        assert_eq!(info.codec(), VideoCodec::default());
        assert_eq!(info.width(), 0);
        assert_eq!(info.height(), 0);
        assert!(info.duration().is_none());
    }

    #[test]
    fn test_aspect_ratio() {
        let info = VideoStreamInfo::builder().width(1920).height(1080).build();
        assert!((info.aspect_ratio() - (16.0 / 9.0)).abs() < 0.01);

        let info = VideoStreamInfo::builder().width(1280).height(720).build();
        assert!((info.aspect_ratio() - (16.0 / 9.0)).abs() < 0.01);

        // Zero height
        let info = VideoStreamInfo::builder().width(1920).height(0).build();
        assert_eq!(info.aspect_ratio(), 0.0);
    }

    #[test]
    fn test_resolution_checks() {
        // SD
        let sd = VideoStreamInfo::builder().width(720).height(480).build();
        assert!(!sd.is_hd());
        assert!(!sd.is_full_hd());
        assert!(!sd.is_4k());

        // HD
        let hd = VideoStreamInfo::builder().width(1280).height(720).build();
        assert!(hd.is_hd());
        assert!(!hd.is_full_hd());
        assert!(!hd.is_4k());

        // Full HD
        let fhd = VideoStreamInfo::builder().width(1920).height(1080).build();
        assert!(fhd.is_hd());
        assert!(fhd.is_full_hd());
        assert!(!fhd.is_4k());

        // 4K
        let uhd = VideoStreamInfo::builder().width(3840).height(2160).build();
        assert!(uhd.is_hd());
        assert!(uhd.is_full_hd());
        assert!(uhd.is_4k());
    }

    #[test]
    fn test_is_hdr() {
        // HDR video: BT.2020 color primaries + 10-bit pixel format
        let hdr = VideoStreamInfo::builder()
            .width(3840)
            .height(2160)
            .color_primaries(ColorPrimaries::Bt2020)
            .pixel_format(PixelFormat::Yuv420p10le)
            .build();
        assert!(hdr.is_hdr());

        // HDR video with P010le format
        let hdr_p010 = VideoStreamInfo::builder()
            .width(3840)
            .height(2160)
            .color_primaries(ColorPrimaries::Bt2020)
            .pixel_format(PixelFormat::P010le)
            .build();
        assert!(hdr_p010.is_hdr());

        // SDR video: BT.709 color primaries (standard HD)
        let sdr_hd = VideoStreamInfo::builder()
            .width(1920)
            .height(1080)
            .color_primaries(ColorPrimaries::Bt709)
            .pixel_format(PixelFormat::Yuv420p)
            .build();
        assert!(!sdr_hd.is_hdr());

        // BT.2020 but 8-bit (not HDR - missing high bit depth)
        let wide_gamut_8bit = VideoStreamInfo::builder()
            .width(3840)
            .height(2160)
            .color_primaries(ColorPrimaries::Bt2020)
            .pixel_format(PixelFormat::Yuv420p) // 8-bit
            .build();
        assert!(!wide_gamut_8bit.is_hdr());

        // 10-bit but BT.709 (not HDR - missing wide gamut)
        let hd_10bit = VideoStreamInfo::builder()
            .width(1920)
            .height(1080)
            .color_primaries(ColorPrimaries::Bt709)
            .pixel_format(PixelFormat::Yuv420p10le)
            .build();
        assert!(!hd_10bit.is_hdr());

        // Default video stream is not HDR
        let default = VideoStreamInfo::default();
        assert!(!default.is_hdr());
    }

    #[test]
    fn test_debug() {
        let info = VideoStreamInfo::builder()
            .index(0)
            .codec(VideoCodec::H264)
            .width(1920)
            .height(1080)
            .build();
        let debug = format!("{info:?}");
        assert!(debug.contains("VideoStreamInfo"));
        assert!(debug.contains("1920"));
        assert!(debug.contains("1080"));
    }

    #[test]
    fn test_clone() {
        let info = VideoStreamInfo::builder()
            .index(0)
            .codec(VideoCodec::H264)
            .codec_name("h264")
            .width(1920)
            .height(1080)
            .build();
        let cloned = info.clone();
        assert_eq!(info.width(), cloned.width());
        assert_eq!(info.height(), cloned.height());
        assert_eq!(info.codec_name(), cloned.codec_name());
    }
}
