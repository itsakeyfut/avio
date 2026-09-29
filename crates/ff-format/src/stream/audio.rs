//! Audio stream info and builder.

use std::time::Duration;

use crate::channel::ChannelLayout;
use crate::codec::AudioCodec;
use crate::sample::SampleFormat;

/// Information about an audio stream within a media file.
///
/// This struct contains all metadata needed to understand and process
/// an audio stream, including sample rate, channel layout, and codec
/// information.
///
/// # Construction
///
/// Use [`AudioStreamInfo::builder()`] for fluent construction:
///
/// ```
/// use ff_format::stream::AudioStreamInfo;
/// use ff_format::SampleFormat;
/// use ff_format::codec::AudioCodec;
///
/// let info = AudioStreamInfo::builder()
///     .index(1)
///     .codec(AudioCodec::Aac)
///     .sample_rate(48000)
///     .channels(2)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct AudioStreamInfo {
    /// Stream index within the container
    index: u32,
    /// Audio codec
    codec: AudioCodec,
    /// Codec name as reported by the demuxer
    codec_name: String,
    /// Sample rate in Hz
    sample_rate: u32,
    /// Number of channels
    channels: u32,
    /// Channel layout
    channel_layout: ChannelLayout,
    /// Sample format
    sample_format: SampleFormat,
    /// Stream duration (if known)
    duration: Option<Duration>,
    /// Bitrate in bits per second (if known)
    bitrate: Option<u64>,
    /// Language code (e.g., "eng", "jpn")
    language: Option<String>,
    /// Whether the linked build has a decoder for this stream.
    ///
    /// `None` means nobody asked, which is what a hand-built value carries: only a
    /// probe against a real file can answer it, and the answer is a property of the
    /// linked build rather than of the file. A consumer must treat `None` as
    /// acceptable, so a stream whose decodability was never established is not
    /// mistaken for one that cannot be decoded.
    decoder_available: Option<bool>,
}

impl AudioStreamInfo {
    /// Creates a new builder for constructing `AudioStreamInfo`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::stream::AudioStreamInfo;
    /// use ff_format::codec::AudioCodec;
    /// use ff_format::SampleFormat;
    ///
    /// let info = AudioStreamInfo::builder()
    ///     .index(1)
    ///     .codec(AudioCodec::Aac)
    ///     .sample_rate(48000)
    ///     .channels(2)
    ///     .build();
    /// ```
    #[must_use]
    pub fn builder() -> AudioStreamInfoBuilder {
        AudioStreamInfoBuilder::default()
    }

    /// Returns the stream index within the container.
    #[must_use]
    #[inline]
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// Returns the audio codec.
    #[must_use]
    #[inline]
    pub const fn codec(&self) -> AudioCodec {
        self.codec
    }

    /// Returns the codec name as reported by the demuxer.
    #[must_use]
    #[inline]
    pub fn codec_name(&self) -> &str {
        &self.codec_name
    }

    /// Returns the sample rate in Hz.
    #[must_use]
    #[inline]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the number of audio channels.
    ///
    /// The type is `u32` to match `FFmpeg`'s `AVCodecParameters::ch_layout.nb_channels`
    /// and professional audio APIs. When passing to `rodio` or `cpal` (which require
    /// `u16`), cast with `info.channels() as u16` — channel counts never exceed
    /// `u16::MAX` in practice.
    #[must_use]
    #[inline]
    pub const fn channels(&self) -> u32 {
        self.channels
    }

    /// Returns the channel layout.
    #[must_use]
    #[inline]
    pub const fn channel_layout(&self) -> ChannelLayout {
        self.channel_layout
    }

    /// Returns the sample format.
    #[must_use]
    #[inline]
    pub const fn sample_format(&self) -> SampleFormat {
        self.sample_format
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

    /// Returns the language code, if specified.
    #[must_use]
    #[inline]
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Returns whether the linked build has a decoder for this stream, or `None`
    /// when nobody asked (see the field's documentation).
    #[must_use]
    #[inline]
    pub const fn decoder_available(&self) -> Option<bool> {
        self.decoder_available
    }

    /// Returns `true` if this is a mono stream.
    #[must_use]
    #[inline]
    pub const fn is_mono(&self) -> bool {
        self.channels == 1
    }

    /// Returns `true` if this is a stereo stream.
    #[must_use]
    #[inline]
    pub const fn is_stereo(&self) -> bool {
        self.channels == 2
    }

    /// Returns `true` if this is a surround sound stream (more than 2 channels).
    #[must_use]
    #[inline]
    pub const fn is_surround(&self) -> bool {
        self.channels > 2
    }
}

impl Default for AudioStreamInfo {
    fn default() -> Self {
        Self {
            index: 0,
            codec: AudioCodec::default(),
            codec_name: String::new(),
            sample_rate: 48000,
            channels: 2,
            channel_layout: ChannelLayout::default(),
            sample_format: SampleFormat::default(),
            duration: None,
            bitrate: None,
            language: None,
            decoder_available: None,
        }
    }
}

/// Builder for constructing `AudioStreamInfo`.
#[derive(Debug, Clone, Default)]
pub struct AudioStreamInfoBuilder {
    index: u32,
    codec: AudioCodec,
    codec_name: String,
    sample_rate: u32,
    channels: u32,
    channel_layout: Option<ChannelLayout>,
    sample_format: SampleFormat,
    duration: Option<Duration>,
    bitrate: Option<u64>,
    language: Option<String>,
    decoder_available: Option<bool>,
}

impl AudioStreamInfoBuilder {
    /// Sets the stream index.
    #[must_use]
    pub fn index(mut self, index: u32) -> Self {
        self.index = index;
        self
    }

    /// Sets the audio codec.
    #[must_use]
    pub fn codec(mut self, codec: AudioCodec) -> Self {
        self.codec = codec;
        self
    }

    /// Sets the codec name string.
    #[must_use]
    pub fn codec_name(mut self, name: impl Into<String>) -> Self {
        self.codec_name = name.into();
        self
    }

    /// Sets the sample rate in Hz.
    #[must_use]
    pub fn sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = rate;
        self
    }

    /// Sets the number of channels.
    ///
    /// This also updates the channel layout if not explicitly set.
    #[must_use]
    pub fn channels(mut self, channels: u32) -> Self {
        self.channels = channels;
        self
    }

    /// Sets the channel layout explicitly.
    #[must_use]
    pub fn channel_layout(mut self, layout: ChannelLayout) -> Self {
        self.channel_layout = Some(layout);
        self
    }

    /// Sets the sample format.
    #[must_use]
    pub fn sample_format(mut self, format: SampleFormat) -> Self {
        self.sample_format = format;
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

    /// Sets the language code.
    #[must_use]
    pub fn language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }

    /// Records whether the linked build has a decoder for this stream.
    #[must_use]
    pub const fn decoder_available(mut self, available: bool) -> Self {
        self.decoder_available = Some(available);
        self
    }

    /// Builds the `AudioStreamInfo`.
    #[must_use]
    pub fn build(self) -> AudioStreamInfo {
        let channel_layout = self.channel_layout.unwrap_or_else(|| {
            log::warn!(
                "channel_layout not set, deriving from channel count \
                 channels={} fallback=from_channels",
                self.channels
            );
            ChannelLayout::from_channels(self.channels)
        });

        AudioStreamInfo {
            index: self.index,
            codec: self.codec,
            codec_name: self.codec_name,
            sample_rate: self.sample_rate,
            channels: self.channels,
            channel_layout,
            sample_format: self.sample_format,
            duration: self.duration,
            bitrate: self.bitrate,
            language: self.language,
            decoder_available: self.decoder_available,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let info = AudioStreamInfo::builder()
            .index(1)
            .codec(AudioCodec::Aac)
            .codec_name("aac")
            .sample_rate(48000)
            .channels(2)
            .sample_format(SampleFormat::F32)
            .build();

        assert_eq!(info.index(), 1);
        assert_eq!(info.codec(), AudioCodec::Aac);
        assert_eq!(info.codec_name(), "aac");
        assert_eq!(info.sample_rate(), 48000);
        assert_eq!(info.channels(), 2);
        assert_eq!(info.sample_format(), SampleFormat::F32);
        assert_eq!(info.channel_layout(), ChannelLayout::Stereo);
    }

    #[test]
    fn test_builder_full() {
        let info = AudioStreamInfo::builder()
            .index(2)
            .codec(AudioCodec::Flac)
            .codec_name("flac")
            .sample_rate(96000)
            .channels(6)
            .channel_layout(ChannelLayout::Surround5_1)
            .sample_format(SampleFormat::I32)
            .duration(Duration::from_secs(300))
            .bitrate(1_411_200)
            .language("jpn")
            .build();

        assert_eq!(info.codec(), AudioCodec::Flac);
        assert_eq!(info.sample_rate(), 96000);
        assert_eq!(info.channels(), 6);
        assert_eq!(info.channel_layout(), ChannelLayout::Surround5_1);
        assert_eq!(info.duration(), Some(Duration::from_secs(300)));
        assert_eq!(info.bitrate(), Some(1_411_200));
        assert_eq!(info.language(), Some("jpn"));
    }

    #[test]
    fn test_default() {
        let info = AudioStreamInfo::default();
        assert_eq!(info.index(), 0);
        assert_eq!(info.codec(), AudioCodec::default());
        assert_eq!(info.sample_rate(), 48000);
        assert_eq!(info.channels(), 2);
        assert!(info.duration().is_none());
    }

    #[test]
    fn test_auto_channel_layout() {
        // Should auto-detect layout from channel count
        let mono = AudioStreamInfo::builder().channels(1).build();
        assert_eq!(mono.channel_layout(), ChannelLayout::Mono);

        let stereo = AudioStreamInfo::builder().channels(2).build();
        assert_eq!(stereo.channel_layout(), ChannelLayout::Stereo);

        let surround = AudioStreamInfo::builder().channels(6).build();
        assert_eq!(surround.channel_layout(), ChannelLayout::Surround5_1);

        // Explicit layout should override
        let custom = AudioStreamInfo::builder()
            .channels(6)
            .channel_layout(ChannelLayout::Other(6))
            .build();
        assert_eq!(custom.channel_layout(), ChannelLayout::Other(6));
    }

    #[test]
    fn test_channel_checks() {
        let mono = AudioStreamInfo::builder().channels(1).build();
        assert!(mono.is_mono());
        assert!(!mono.is_stereo());
        assert!(!mono.is_surround());

        let stereo = AudioStreamInfo::builder().channels(2).build();
        assert!(!stereo.is_mono());
        assert!(stereo.is_stereo());
        assert!(!stereo.is_surround());

        let surround = AudioStreamInfo::builder().channels(6).build();
        assert!(!surround.is_mono());
        assert!(!surround.is_stereo());
        assert!(surround.is_surround());
    }

    #[test]
    fn test_debug() {
        let info = AudioStreamInfo::builder()
            .index(1)
            .codec(AudioCodec::Aac)
            .sample_rate(48000)
            .channels(2)
            .build();
        let debug = format!("{info:?}");
        assert!(debug.contains("AudioStreamInfo"));
        assert!(debug.contains("48000"));
    }

    #[test]
    fn test_clone() {
        let info = AudioStreamInfo::builder()
            .index(1)
            .codec(AudioCodec::Aac)
            .codec_name("aac")
            .sample_rate(48000)
            .channels(2)
            .language("eng")
            .build();
        let cloned = info.clone();
        assert_eq!(info.sample_rate(), cloned.sample_rate());
        assert_eq!(info.channels(), cloned.channels());
        assert_eq!(info.language(), cloned.language());
        assert_eq!(info.codec_name(), cloned.codec_name());
    }
}
