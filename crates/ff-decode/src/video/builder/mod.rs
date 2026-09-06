//! Video decoder builder for constructing video decoders with custom configuration.
//!
//! This module provides the [`VideoDecoderBuilder`] type which enables fluent
//! configuration of video decoders. Use [`VideoDecoder::open()`] to start building.
//!
//! # Examples
//!
//! ```ignore
//! use ff_decode::{VideoDecoder, HardwareAccel};
//! use ff_format::PixelFormat;
//!
//! let decoder = VideoDecoder::open("video.mp4")?
//!     .output_format(PixelFormat::Rgba)
//!     .hardware_accel(HardwareAccel::Auto)
//!     .thread_count(4)
//!     .build()?;
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ff_format::{ContainerInfo, NetworkOptions, PixelFormat, VideoStreamInfo};

use crate::HardwareAccel;
use crate::error::DecodeError;
use crate::video::decoder_inner::VideoDecoderInner;
use ff_common::FramePool;

mod decode;
mod format;
mod hw;
mod network;
mod scale;

/// Requested output scale for decoded frames.
///
/// Controls how `libswscale` resizes the frame in the same pass as pixel-format
/// conversion. The last setter wins — calling `output_width()` after
/// `output_size()` replaces the earlier setting.
///
/// Both width and height are rounded up to the nearest even number if needed,
/// because most pixel formats (e.g. `yuv420p`) require even dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputScale {
    /// Scale to an exact width × height.
    Exact {
        /// Target width in pixels.
        width: u32,
        /// Target height in pixels.
        height: u32,
    },
    /// Scale to the given width; compute height to preserve aspect ratio.
    FitWidth(u32),
    /// Scale to the given height; compute width to preserve aspect ratio.
    FitHeight(u32),
}

/// Builder for configuring and constructing a [`VideoDecoder`].
///
/// This struct provides a fluent interface for setting up decoder options
/// before opening a video file. It is created by calling [`VideoDecoder::open()`].
///
/// # Examples
///
/// ## Basic Usage
///
/// ```ignore
/// use ff_decode::VideoDecoder;
///
/// let decoder = VideoDecoder::open("video.mp4")?
///     .build()?;
/// ```
///
/// ## With Custom Format
///
/// ```ignore
/// use ff_decode::VideoDecoder;
/// use ff_format::PixelFormat;
///
/// let decoder = VideoDecoder::open("video.mp4")?
///     .output_format(PixelFormat::Rgba)
///     .build()?;
/// ```
///
/// ## With Hardware Acceleration
///
/// ```ignore
/// use ff_decode::{VideoDecoder, HardwareAccel};
///
/// let decoder = VideoDecoder::open("video.mp4")?
///     .hardware_accel(HardwareAccel::Nvdec)
///     .build()?;
/// ```
///
/// ## With Frame Pool
///
/// ```ignore
/// use ff_decode::{VideoDecoder, FramePool};
/// use std::sync::Arc;
///
/// let pool: Arc<dyn FramePool> = create_frame_pool();
/// let decoder = VideoDecoder::open("video.mp4")?
///     .frame_pool(pool)
///     .build()?;
/// ```
#[derive(Debug)]
pub struct VideoDecoderBuilder {
    /// Path to the media file
    path: PathBuf,
    /// Output pixel format (None = use source format)
    output_format: Option<PixelFormat>,
    /// Output scale (None = use source dimensions)
    output_scale: Option<OutputScale>,
    /// Hardware acceleration setting
    hardware_accel: HardwareAccel,
    /// Number of decoding threads (0 = auto)
    thread_count: usize,
    /// Optional frame pool for memory reuse
    frame_pool: Option<Arc<dyn FramePool>>,
    /// Frame rate override for image sequences (default 25 fps when path contains `%`).
    frame_rate: Option<u32>,
    /// Network options for URL-based sources (RTMP, RTSP, HTTP, etc.).
    network_opts: Option<NetworkOptions>,
    /// A caller-supplied byte source to demux from instead of `path`.
    ///
    /// Set by [`VideoDecoder::from_reader`]; when present it wins over `path`,
    /// which is then only a label for diagnostics.
    source: Option<SourceHandle>,
}

/// A boxed byte source that can sit in a `#[derive(Debug)]` struct.
///
/// A caller's reader carries no `Debug` bound -- requiring one would rule out
/// most of what this feature exists to accept -- so the handle prints a label
/// instead of its contents.
struct SourceHandle(Box<dyn ff_sys::IoSource>);

impl std::fmt::Debug for SourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<reader>")
    }
}

impl VideoDecoderBuilder {
    /// Creates a new builder for the specified file path.
    ///
    /// This is an internal constructor; use [`VideoDecoder::open()`] instead.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            output_format: None,
            output_scale: None,
            hardware_accel: HardwareAccel::Auto,
            thread_count: 0,
            frame_pool: None,
            frame_rate: None,
            network_opts: None,
            source: None,
        }
    }

    /// Creates a builder that demuxes `source` instead of a file.
    ///
    /// This is an internal constructor; use [`VideoDecoder::from_reader()`].
    pub(crate) fn from_source(source: Box<dyn ff_sys::IoSource>) -> Self {
        let mut builder = Self::new(PathBuf::from("<reader>"));
        builder.source = Some(SourceHandle(source));
        builder
    }

    /// Returns the configured file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the configured output format, if any.
    #[must_use]
    pub fn get_output_format(&self) -> Option<PixelFormat> {
        self.output_format
    }

    /// Returns the configured hardware acceleration mode.
    #[must_use]
    pub fn get_hardware_accel(&self) -> HardwareAccel {
        self.hardware_accel
    }

    /// Returns the configured thread count.
    #[must_use]
    pub fn get_thread_count(&self) -> usize {
        self.thread_count
    }

    /// Builds the decoder with the configured options.
    ///
    /// This method opens the media file, initializes the decoder context,
    /// and prepares for frame decoding.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be found ([`DecodeError::FileNotFound`])
    /// - The file contains no video stream ([`DecodeError::NoVideoStream`])
    /// - The codec is not supported ([`DecodeError::UnsupportedCodec`])
    /// - Hardware acceleration is unavailable ([`DecodeError::HwAccelUnavailable`])
    /// - Other `FFmpeg` errors occur ([`DecodeError::Ffmpeg`])
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::VideoDecoder;
    ///
    /// let decoder = VideoDecoder::open("video.mp4")?
    ///     .build()?;
    ///
    /// // Start decoding
    /// for result in &mut decoder {
    ///     let frame = result?;
    ///     // Process frame...
    /// }
    /// ```
    pub fn build(self) -> Result<VideoDecoder, DecodeError> {
        // Validate output scale dimensions before opening the file.
        // FitWidth / FitHeight aspect-ratio dimensions are resolved at decode time
        // from the actual source dimensions, so we only reject an explicit zero here.
        if let Some(scale) = self.output_scale {
            let (w, h) = match scale {
                OutputScale::Exact { width, height } => (width, height),
                OutputScale::FitWidth(w) => (w, 1), // height will be derived
                OutputScale::FitHeight(h) => (1, h), // width will be derived
            };
            if w == 0 || h == 0 {
                return Err(DecodeError::InvalidOutputDimensions {
                    width: w,
                    height: h,
                });
            }
        }

        // Image-sequence patterns contain '%' — the literal path does not exist.
        // Network URLs must also skip the file-existence check, and so must a
        // caller-supplied source, whose `path` is a label rather than a location.
        let path_str = self.path.to_str().unwrap_or("");
        let is_image_sequence = path_str.contains('%');
        let is_network_url = crate::network::is_url(path_str);
        if self.source.is_none() && !is_image_sequence && !is_network_url && !self.path.exists() {
            return Err(DecodeError::FileNotFound {
                path: self.path.clone(),
            });
        }

        // Create the decoder inner
        let (inner, stream_info, container_info) = VideoDecoderInner::new(
            &self.path,
            self.output_format,
            self.output_scale,
            self.hardware_accel,
            self.thread_count,
            self.frame_rate,
            self.frame_pool.clone(),
            self.network_opts,
            self.source.map(|s| s.0),
        )?;

        Ok(VideoDecoder {
            path: self.path,
            frame_pool: self.frame_pool,
            inner,
            stream_info,
            container_info,
            fused: false,
        })
    }
}

/// A video decoder for extracting frames from media files.
///
/// The decoder provides frame-by-frame access to video content with support
/// for seeking, hardware acceleration, and format conversion.
///
/// # Construction
///
/// Use [`VideoDecoder::open()`] to create a builder, then call [`VideoDecoderBuilder::build()`]:
///
/// ```ignore
/// use ff_decode::VideoDecoder;
/// use ff_format::PixelFormat;
///
/// let decoder = VideoDecoder::open("video.mp4")?
///     .output_format(PixelFormat::Rgba)
///     .build()?;
/// ```
///
/// # Frame Decoding
///
/// Frames can be decoded one at a time or using the built-in iterator:
///
/// ```ignore
/// // Decode one frame
/// if let Some(frame) = decoder.decode_one()? {
///     println!("Frame at {:?}", frame.timestamp().as_duration());
/// }
///
/// // Iterator form — VideoDecoder implements Iterator directly
/// for result in &mut decoder {
///     let frame = result?;
///     // Process frame...
/// }
/// ```
///
/// # Seeking
///
/// The decoder supports efficient seeking:
///
/// ```ignore
/// use ff_decode::SeekMode;
/// use std::time::Duration;
///
/// // Seek to 30 seconds (keyframe)
/// decoder.seek(Duration::from_secs(30), SeekMode::Keyframe)?;
///
/// // Seek to exact frame
/// decoder.seek(Duration::from_secs(30), SeekMode::Exact)?;
/// ```
pub struct VideoDecoder {
    /// Path to the media file
    path: PathBuf,
    /// Optional frame pool for memory reuse
    frame_pool: Option<Arc<dyn FramePool>>,
    /// Internal decoder state
    inner: VideoDecoderInner,
    /// Video stream information
    stream_info: VideoStreamInfo,
    /// Container-level metadata
    container_info: ContainerInfo,
    /// Set to `true` after a decoding error; causes [`Iterator::next`] to return `None`.
    fused: bool,
}

impl VideoDecoder {
    /// Opens a media file and returns a builder for configuring the decoder.
    ///
    /// This is the entry point for creating a decoder. The returned builder
    /// allows setting options before the decoder is fully initialized.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the media file to decode.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::VideoDecoder;
    ///
    /// // Simple usage
    /// let decoder = VideoDecoder::open("video.mp4")?
    ///     .build()?;
    ///
    /// // With options
    /// let decoder = VideoDecoder::open("video.mp4")?
    ///     .output_format(PixelFormat::Rgba)
    ///     .hardware_accel(HardwareAccel::Auto)
    ///     .build()?;
    /// ```
    ///
    /// # Note
    ///
    /// This method does not validate that the file exists or is a valid
    /// media file. Validation occurs when [`VideoDecoderBuilder::build()`] is called.
    pub fn open(path: impl AsRef<Path>) -> VideoDecoderBuilder {
        VideoDecoderBuilder::new(path.as_ref().to_path_buf())
    }

    /// Creates a builder that decodes from `source` instead of a file.
    ///
    /// `source` is anything that reads and seeks and can move to the decoder's
    /// thread -- an in-memory `Cursor<Vec<u8>>`, a `File`, a custom byte store.
    /// The container format is probed from the bytes, so nothing has to be named.
    ///
    /// `Seek` is required: `FFmpeg` can demux without it, but the containers that
    /// survive a non-seekable input are a narrower set with different failure
    /// modes, and supporting it is separate work.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::io::Cursor;
    /// use ff_decode::VideoDecoder;
    ///
    /// let bytes: Vec<u8> = std::fs::read("input.mp4")?;
    /// let mut decoder = VideoDecoder::from_reader(Cursor::new(bytes)).build()?;
    /// let frame = decoder.decode_one()?;
    /// # Ok::<(), ff_decode::DecodeError>(())
    /// ```
    ///
    /// # Note
    ///
    /// As with [`open`](Self::open), nothing is read until
    /// [`VideoDecoderBuilder::build()`] is called.
    pub fn from_reader(source: impl ff_sys::IoSource + 'static) -> VideoDecoderBuilder {
        VideoDecoderBuilder::from_source(Box::new(source))
    }

    // =========================================================================
    // Information Methods
    // =========================================================================

    /// Returns the video stream information.
    ///
    /// This contains metadata about the video stream including resolution,
    /// frame rate, codec, and color characteristics.
    #[must_use]
    pub fn stream_info(&self) -> &VideoStreamInfo {
        &self.stream_info
    }

    /// Returns the video width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.stream_info.width()
    }

    /// Returns the video height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.stream_info.height()
    }

    /// Returns the frame rate in frames per second.
    #[must_use]
    pub fn frame_rate(&self) -> f64 {
        self.stream_info.fps()
    }

    /// Returns the total duration of the video.
    ///
    /// Returns [`Duration::ZERO`] if duration is unknown.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.stream_info.duration().unwrap_or(Duration::ZERO)
    }

    /// Returns the total duration of the video, or `None` for live streams
    /// or formats that do not carry duration information.
    #[must_use]
    pub fn duration_opt(&self) -> Option<Duration> {
        self.stream_info.duration()
    }

    /// Returns container-level metadata (format name, bitrate, stream count).
    #[must_use]
    pub fn container_info(&self) -> &ContainerInfo {
        &self.container_info
    }

    /// Returns the current playback position.
    #[must_use]
    pub fn position(&self) -> Duration {
        self.inner.position()
    }

    /// Returns `true` if the end of stream has been reached.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.inner.is_eof()
    }

    /// Returns the file path being decoded.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns a reference to the frame pool, if configured.
    #[must_use]
    pub fn frame_pool(&self) -> Option<&Arc<dyn FramePool>> {
        self.frame_pool.as_ref()
    }

    /// Returns the currently active hardware acceleration mode.
    ///
    /// This method returns the actual hardware acceleration being used,
    /// which may differ from what was requested:
    ///
    /// - If [`HardwareAccel::Auto`] was requested, this returns the specific
    ///   accelerator that was successfully initialized (e.g., [`HardwareAccel::Nvdec`]),
    ///   or [`HardwareAccel::None`] if no hardware acceleration is available.
    /// - If a specific accelerator was requested and initialization failed,
    ///   the decoder creation would have returned an error.
    /// - If [`HardwareAccel::None`] was requested, this always returns [`HardwareAccel::None`].
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::{VideoDecoder, HardwareAccel};
    ///
    /// // Request automatic hardware acceleration
    /// let decoder = VideoDecoder::open("video.mp4")?
    ///     .hardware_accel(HardwareAccel::Auto)
    ///     .build()?;
    ///
    /// // Check which accelerator was selected
    /// match decoder.hardware_accel() {
    ///     HardwareAccel::None => println!("Using software decoding"),
    ///     HardwareAccel::Nvdec => println!("Using NVIDIA NVDEC"),
    ///     HardwareAccel::Qsv => println!("Using Intel Quick Sync"),
    ///     HardwareAccel::VideoToolbox => println!("Using Apple VideoToolbox"),
    ///     HardwareAccel::Vaapi => println!("Using VA-API"),
    ///     HardwareAccel::Amf => println!("Using AMD AMF"),
    ///     _ => unreachable!(),
    /// }
    /// ```
    #[must_use]
    pub fn hardware_accel(&self) -> HardwareAccel {
        self.inner.hardware_accel()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builder_default_values_should_have_auto_hw_and_zero_threads() {
        let builder = VideoDecoderBuilder::new(PathBuf::from("test.mp4"));

        assert_eq!(builder.path(), Path::new("test.mp4"));
        assert!(builder.get_output_format().is_none());
        assert_eq!(builder.get_hardware_accel(), HardwareAccel::Auto);
        assert_eq!(builder.get_thread_count(), 0);
    }

    #[test]
    fn builder_chaining_should_set_all_fields() {
        let builder = VideoDecoderBuilder::new(PathBuf::from("test.mp4"))
            .output_format(PixelFormat::Bgra)
            .hardware_accel(HardwareAccel::Qsv)
            .thread_count(4);

        assert_eq!(builder.get_output_format(), Some(PixelFormat::Bgra));
        assert_eq!(builder.get_hardware_accel(), HardwareAccel::Qsv);
        assert_eq!(builder.get_thread_count(), 4);
    }

    #[test]
    fn decoder_open_should_return_builder_with_path() {
        let builder = VideoDecoder::open("video.mp4");
        assert_eq!(builder.path(), Path::new("video.mp4"));
    }

    #[test]
    fn decoder_open_pathbuf_should_preserve_path() {
        let path = PathBuf::from("/path/to/video.mp4");
        let builder = VideoDecoder::open(&path);
        assert_eq!(builder.path(), path.as_path());
    }

    #[test]
    fn build_nonexistent_file_should_return_file_not_found() {
        let result = VideoDecoder::open("nonexistent_file_12345.mp4").build();

        assert!(result.is_err());
        match result {
            Err(DecodeError::FileNotFound { path }) => {
                assert!(
                    path.to_string_lossy()
                        .contains("nonexistent_file_12345.mp4")
                );
            }
            Err(e) => panic!("Expected FileNotFound error, got: {e:?}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn build_invalid_video_file_should_fail() {
        // Create a temporary test file (not a valid video)
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ff_decode_test_file.txt");
        std::fs::write(&test_file, "test").expect("Failed to create test file");

        let result = VideoDecoder::open(&test_file).build();

        // Clean up
        let _ = std::fs::remove_file(&test_file);

        // The build should fail (not a valid video file)
        assert!(result.is_err());
        if let Err(e) = result {
            // Should get either NoVideoStream or Ffmpeg error
            assert!(
                matches!(e, DecodeError::NoVideoStream { .. })
                    || matches!(e, DecodeError::Ffmpeg { .. })
            );
        }
    }
}
