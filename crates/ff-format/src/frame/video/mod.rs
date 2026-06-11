//! Video frame type.
//!
//! This module provides [`VideoFrame`] for working with decoded video frames.
//!
//! # Examples
//!
//! ```
//! use ff_format::{PixelFormat, PooledBuffer, Rational, Timestamp, VideoFrame};
//!
//! // Create a simple 1920x1080 RGBA frame
//! let width = 1920u32;
//! let height = 1080u32;
//! let bytes_per_pixel = 4; // RGBA
//! let stride = width as usize * bytes_per_pixel;
//! let data = vec![0u8; stride * height as usize];
//!
//! let frame = VideoFrame::new(
//!     vec![PooledBuffer::standalone(data)],
//!     vec![stride],
//!     width,
//!     height,
//!     PixelFormat::Rgba,
//!     Timestamp::default(),
//!     true,
//! ).unwrap();
//!
//! assert_eq!(frame.width(), 1920);
//! assert_eq!(frame.height(), 1080);
//! assert!(frame.is_key_frame());
//! assert_eq!(frame.num_planes(), 1);
//! ```

use std::fmt;

use crate::error::FrameError;
use crate::{PixelFormat, PooledBuffer, Timestamp};

mod planar;

/// A decoded video frame.
///
/// This structure holds the pixel data and metadata for a single video frame.
/// It supports both packed formats (like RGBA) where all data is in a single
/// plane, and planar formats (like YUV420P) where each color component is
/// stored in a separate plane.
///
/// # Memory Layout
///
/// For packed formats (RGB, RGBA, BGR, BGRA):
/// - Single plane containing all pixel data
/// - Stride equals width × `bytes_per_pixel` (plus optional padding)
///
/// For planar YUV formats (YUV420P, YUV422P, YUV444P):
/// - Plane 0: Y (luma) - full resolution
/// - Plane 1: U (Cb) - may be subsampled
/// - Plane 2: V (Cr) - may be subsampled
///
/// For semi-planar formats (NV12, NV21):
/// - Plane 0: Y (luma) - full resolution
/// - Plane 1: UV interleaved - half height
///
/// # Strides
///
/// Each plane has an associated stride (also called line size or pitch),
/// which is the number of bytes from the start of one row to the start
/// of the next. This may be larger than the actual data width due to
/// alignment requirements.
#[derive(Clone)]
pub struct VideoFrame {
    /// Pixel data for each plane
    planes: Vec<PooledBuffer>,
    /// Stride (bytes per row) for each plane
    strides: Vec<usize>,
    /// Frame width in pixels
    width: u32,
    /// Frame height in pixels
    height: u32,
    /// Pixel format
    format: PixelFormat,
    /// Presentation timestamp
    timestamp: Timestamp,
    /// Whether this is a key frame (I-frame)
    key_frame: bool,
}

impl VideoFrame {
    /// Creates a new video frame with the specified parameters.
    ///
    /// # Arguments
    ///
    /// * `planes` - Pixel data for each plane
    /// * `strides` - Stride (bytes per row) for each plane
    /// * `width` - Frame width in pixels
    /// * `height` - Frame height in pixels
    /// * `format` - Pixel format
    /// * `timestamp` - Presentation timestamp
    /// * `key_frame` - Whether this is a key frame
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::MismatchedPlaneStride`] if `planes.len() != strides.len()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, PooledBuffer, Rational, Timestamp, VideoFrame};
    ///
    /// // Create a 640x480 YUV420P frame
    /// let width = 640u32;
    /// let height = 480u32;
    ///
    /// // Y plane: full resolution
    /// let y_stride = width as usize;
    /// let y_data = vec![128u8; y_stride * height as usize];
    ///
    /// // U/V planes: half resolution in both dimensions
    /// let uv_stride = (width / 2) as usize;
    /// let uv_height = (height / 2) as usize;
    /// let u_data = vec![128u8; uv_stride * uv_height];
    /// let v_data = vec![128u8; uv_stride * uv_height];
    ///
    /// let frame = VideoFrame::new(
    ///     vec![
    ///         PooledBuffer::standalone(y_data),
    ///         PooledBuffer::standalone(u_data),
    ///         PooledBuffer::standalone(v_data),
    ///     ],
    ///     vec![y_stride, uv_stride, uv_stride],
    ///     width,
    ///     height,
    ///     PixelFormat::Yuv420p,
    ///     Timestamp::default(),
    ///     true,
    /// ).unwrap();
    ///
    /// assert_eq!(frame.num_planes(), 3);
    /// ```
    pub fn new(
        planes: Vec<PooledBuffer>,
        strides: Vec<usize>,
        width: u32,
        height: u32,
        format: PixelFormat,
        timestamp: Timestamp,
        key_frame: bool,
    ) -> Result<Self, FrameError> {
        if planes.len() != strides.len() {
            return Err(FrameError::MismatchedPlaneStride {
                planes: planes.len(),
                strides: strides.len(),
            });
        }
        Ok(Self {
            planes,
            strides,
            width,
            height,
            format,
            timestamp,
            key_frame,
        })
    }

    /// Creates an empty video frame with the specified dimensions and format.
    ///
    /// The frame will have properly sized planes filled with zeros based
    /// on the pixel format.
    ///
    /// # Arguments
    ///
    /// * `width` - Frame width in pixels
    /// * `height` - Frame height in pixels
    /// * `format` - Pixel format
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::UnsupportedPixelFormat`] if the format is
    /// [`PixelFormat::Other`], as the memory layout cannot be determined.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// assert_eq!(frame.width(), 1920);
    /// assert_eq!(frame.height(), 1080);
    /// assert_eq!(frame.num_planes(), 1);
    /// ```
    pub fn empty(width: u32, height: u32, format: PixelFormat) -> Result<Self, FrameError> {
        let (planes, strides) = Self::allocate_planes(width, height, format)?;
        Ok(Self {
            planes,
            strides,
            width,
            height,
            format,
            timestamp: Timestamp::default(),
            key_frame: false,
        })
    }

    /// Creates a packed [`PixelFormat::Rgba`] frame from tightly-packed pixel
    /// data (`width * height * 4` bytes, no row padding).
    ///
    /// This is the inverse of [`to_rgba`](Self::to_rgba); together they let a
    /// host move frames between an RGBA surface and avio's filter graph.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::InvalidDataSize`] if `rgba.len() != width * height * 4`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::from_rgba(2, 2, vec![0u8; 2 * 2 * 4]).unwrap();
    /// assert_eq!(frame.format(), PixelFormat::Rgba);
    /// assert_eq!(frame.num_planes(), 1);
    /// ```
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, FrameError> {
        let stride = width as usize * 4;
        let expected = stride * height as usize;
        if rgba.len() != expected {
            return Err(FrameError::InvalidDataSize {
                expected,
                actual: rgba.len(),
            });
        }
        Self::new(
            vec![PooledBuffer::standalone(rgba)],
            vec![stride],
            width,
            height,
            PixelFormat::Rgba,
            Timestamp::default(),
            false,
        )
    }

    /// Returns tightly-packed RGBA bytes (`width * height * 4`, row padding
    /// stripped) when this frame is [`PixelFormat::Rgba`]; `None` otherwise.
    ///
    /// Inverse of [`from_rgba`](Self::from_rgba).
    #[must_use]
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        if self.format != PixelFormat::Rgba {
            return None;
        }
        let row_bytes = self.width as usize * 4;
        let stride = self.stride(0)?;
        let src = self.plane(0)?;
        let mut out = Vec::with_capacity(row_bytes * self.height as usize);
        for row in 0..self.height as usize {
            let start = row * stride;
            out.extend_from_slice(src.get(start..start + row_bytes)?);
        }
        Some(out)
    }

    /// Creates a black YUV420P video frame.
    ///
    /// The Y plane is filled with `0x00`; U and V planes are filled with `0x80`
    /// (neutral chroma). `pts_ms` is the presentation timestamp in milliseconds.
    ///
    /// The `format` parameter is accepted for call-site clarity; always pass
    /// [`PixelFormat::Yuv420p`].
    #[doc(hidden)]
    #[must_use]
    pub fn new_black(width: u32, height: u32, format: PixelFormat, pts_ms: i64) -> Self {
        let y_w = width as usize;
        let y_h = height as usize;
        let uv_w = (width as usize).div_ceil(2);
        let uv_h = (height as usize).div_ceil(2);
        let timestamp = Timestamp::from_millis(pts_ms, crate::Rational::new(1, 1000));
        Self {
            planes: vec![
                PooledBuffer::standalone(vec![0u8; y_w * y_h]),
                PooledBuffer::standalone(vec![0x80u8; uv_w * uv_h]),
                PooledBuffer::standalone(vec![0x80u8; uv_w * uv_h]),
            ],
            strides: vec![y_w, uv_w, uv_w],
            width,
            height,
            format,
            timestamp,
            key_frame: true,
        }
    }

    // ==========================================================================
    // Metadata Accessors
    // ==========================================================================

    /// Returns the frame width in pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// assert_eq!(frame.width(), 1920);
    /// ```
    #[must_use]
    #[inline]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the frame height in pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// assert_eq!(frame.height(), 1080);
    /// ```
    #[must_use]
    #[inline]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the pixel format of this frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Yuv420p).unwrap();
    /// assert_eq!(frame.format(), PixelFormat::Yuv420p);
    /// ```
    #[must_use]
    #[inline]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// Returns the presentation timestamp of this frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, PooledBuffer, Rational, Timestamp, VideoFrame};
    ///
    /// let ts = Timestamp::new(90000, Rational::new(1, 90000));
    /// let frame = VideoFrame::new(
    ///     vec![PooledBuffer::standalone(vec![0u8; 1920 * 1080 * 4])],
    ///     vec![1920 * 4],
    ///     1920,
    ///     1080,
    ///     PixelFormat::Rgba,
    ///     ts,
    ///     true,
    /// ).unwrap();
    /// assert_eq!(frame.timestamp(), ts);
    /// ```
    #[must_use]
    #[inline]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns whether this frame is a key frame (I-frame).
    ///
    /// Key frames are complete frames that don't depend on any other frames
    /// for decoding. They are used as reference points for seeking.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let mut frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// assert!(!frame.is_key_frame());
    ///
    /// frame.set_key_frame(true);
    /// assert!(frame.is_key_frame());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_key_frame(&self) -> bool {
        self.key_frame
    }

    /// Sets whether this frame is a key frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let mut frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
    /// frame.set_key_frame(true);
    /// assert!(frame.is_key_frame());
    /// ```
    #[inline]
    pub fn set_key_frame(&mut self, key_frame: bool) {
        self.key_frame = key_frame;
    }

    /// Sets the timestamp of this frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, Rational, Timestamp, VideoFrame};
    ///
    /// let mut frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
    /// let ts = Timestamp::new(3000, Rational::new(1, 90000));
    /// frame.set_timestamp(ts);
    /// assert_eq!(frame.timestamp(), ts);
    /// ```
    #[inline]
    pub fn set_timestamp(&mut self, timestamp: Timestamp) {
        self.timestamp = timestamp;
    }

    // ==========================================================================
    // Plane Data Access
    // ==========================================================================

    /// Returns the number of planes in this frame.
    ///
    /// - Packed formats (RGBA, RGB24, etc.): 1 plane
    /// - Planar YUV (YUV420P, YUV422P, YUV444P): 3 planes
    /// - Semi-planar (NV12, NV21): 2 planes
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let rgba = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
    /// assert_eq!(rgba.num_planes(), 1);
    ///
    /// let yuv = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
    /// assert_eq!(yuv.num_planes(), 3);
    ///
    /// let nv12 = VideoFrame::empty(640, 480, PixelFormat::Nv12).unwrap();
    /// assert_eq!(nv12.num_planes(), 2);
    /// ```
    #[must_use]
    #[inline]
    pub fn num_planes(&self) -> usize {
        self.planes.len()
    }

    /// Returns a slice of all plane buffers.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
    /// let planes = frame.planes();
    /// assert_eq!(planes.len(), 3);
    /// ```
    #[must_use]
    #[inline]
    pub fn planes(&self) -> &[PooledBuffer] {
        &self.planes
    }

    /// Returns the data for a specific plane, or `None` if the index is out of bounds.
    ///
    /// # Arguments
    ///
    /// * `index` - The plane index (0 for Y/RGB, 1 for U/UV, 2 for V)
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
    ///
    /// // Y plane exists
    /// assert!(frame.plane(0).is_some());
    ///
    /// // U and V planes exist
    /// assert!(frame.plane(1).is_some());
    /// assert!(frame.plane(2).is_some());
    ///
    /// // No fourth plane
    /// assert!(frame.plane(3).is_none());
    /// ```
    #[must_use]
    #[inline]
    pub fn plane(&self, index: usize) -> Option<&[u8]> {
        self.planes.get(index).map(std::convert::AsRef::as_ref)
    }

    /// Returns mutable access to a specific plane's data.
    ///
    /// # Arguments
    ///
    /// * `index` - The plane index
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let mut frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
    /// if let Some(data) = frame.plane_mut(0) {
    ///     // Fill with red (RGBA)
    ///     for chunk in data.chunks_exact_mut(4) {
    ///         chunk[0] = 255; // R
    ///         chunk[1] = 0;   // G
    ///         chunk[2] = 0;   // B
    ///         chunk[3] = 255; // A
    ///     }
    /// }
    /// ```
    #[must_use]
    #[inline]
    pub fn plane_mut(&mut self, index: usize) -> Option<&mut [u8]> {
        self.planes.get_mut(index).map(std::convert::AsMut::as_mut)
    }

    /// Returns a slice of all stride values.
    ///
    /// Strides indicate the number of bytes between the start of consecutive
    /// rows in each plane.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// let strides = frame.strides();
    /// assert_eq!(strides[0], 1920 * 4); // RGBA = 4 bytes per pixel
    /// ```
    #[must_use]
    #[inline]
    pub fn strides(&self) -> &[usize] {
        &self.strides
    }

    /// Returns the stride for a specific plane, or `None` if the index is out of bounds.
    ///
    /// # Arguments
    ///
    /// * `plane` - The plane index
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
    ///
    /// // Y plane stride = width
    /// assert_eq!(frame.stride(0), Some(640));
    ///
    /// // U/V plane stride = width / 2
    /// assert_eq!(frame.stride(1), Some(320));
    /// assert_eq!(frame.stride(2), Some(320));
    /// ```
    #[must_use]
    #[inline]
    pub fn stride(&self, plane: usize) -> Option<usize> {
        self.strides.get(plane).copied()
    }

    // ==========================================================================
    // Contiguous Data Access
    // ==========================================================================

    /// Returns the frame data as a contiguous byte vector.
    ///
    /// For packed formats with a single plane, this returns a copy of the plane data.
    /// For planar formats, this concatenates all planes into a single buffer.
    ///
    /// # Note
    ///
    /// This method allocates a new vector and copies the data. For zero-copy
    /// access, use [`plane()`](Self::plane) or [`planes()`](Self::planes) instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
    /// let data = frame.data();
    /// assert_eq!(data.len(), 4 * 4 * 4); // 4x4 pixels, 4 bytes each
    /// ```
    #[must_use]
    pub fn data(&self) -> Vec<u8> {
        let total_size: usize = self.planes.iter().map(PooledBuffer::len).sum();
        let mut result = Vec::with_capacity(total_size);
        for plane in &self.planes {
            result.extend_from_slice(plane.as_ref());
        }
        result
    }

    /// Returns a reference to the first plane's data as a contiguous slice.
    ///
    /// This is only meaningful for packed formats (RGBA, RGB24, etc.) where
    /// all data is in a single plane. Returns `None` if the format is planar
    /// or if no planes exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// // Packed format - returns data
    /// let rgba = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
    /// assert!(rgba.data_ref().is_some());
    ///
    /// // Planar format - returns None
    /// let yuv = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
    /// assert!(yuv.data_ref().is_none());
    /// ```
    #[must_use]
    #[inline]
    pub fn data_ref(&self) -> Option<&[u8]> {
        if self.format.is_packed() && self.planes.len() == 1 {
            Some(self.planes[0].as_ref())
        } else {
            None
        }
    }

    /// Returns a mutable reference to the first plane's data.
    ///
    /// This is only meaningful for packed formats where all data is in a
    /// single plane. Returns `None` if the format is planar.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let mut frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
    /// if let Some(data) = frame.data_mut() {
    ///     data[0] = 255; // Modify first byte
    /// }
    /// ```
    #[must_use]
    #[inline]
    pub fn data_mut(&mut self) -> Option<&mut [u8]> {
        if self.format.is_packed() && self.planes.len() == 1 {
            Some(self.planes[0].as_mut())
        } else {
            None
        }
    }

    // ==========================================================================
    // Utility Methods
    // ==========================================================================

    /// Returns the total size in bytes of all plane data.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// assert_eq!(frame.total_size(), 1920 * 1080 * 4);
    /// ```
    #[must_use]
    pub fn total_size(&self) -> usize {
        self.planes.iter().map(PooledBuffer::len).sum()
    }

    /// Returns the resolution as a (width, height) tuple.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// assert_eq!(frame.resolution(), (1920, 1080));
    /// ```
    #[must_use]
    #[inline]
    pub const fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns the aspect ratio as a floating-point value.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_format::{PixelFormat, VideoFrame};
    ///
    /// let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
    /// let aspect = frame.aspect_ratio();
    /// assert!((aspect - 16.0 / 9.0).abs() < 0.01);
    /// ```
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
}

impl fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("timestamp", &self.timestamp)
            .field("key_frame", &self.key_frame)
            .field("num_planes", &self.planes.len())
            .field(
                "plane_sizes",
                &self
                    .planes
                    .iter()
                    .map(PooledBuffer::len)
                    .collect::<Vec<_>>(),
            )
            .field("strides", &self.strides)
            .finish()
    }
}

impl fmt::Display for VideoFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "VideoFrame({}x{} {} @ {}{})",
            self.width,
            self.height,
            self.format,
            self.timestamp,
            if self.key_frame { " [KEY]" } else { "" }
        )
    }
}

impl Default for VideoFrame {
    /// Returns a default empty 1x1 YUV420P frame.
    ///
    /// This constructs a minimal valid frame directly.
    fn default() -> Self {
        // Construct a minimal 1x1 YUV420P frame directly
        // Y plane: 1 byte, U plane: 1 byte, V plane: 1 byte
        Self {
            planes: vec![
                PooledBuffer::standalone(vec![0u8; 1]),
                PooledBuffer::standalone(vec![0u8; 1]),
                PooledBuffer::standalone(vec![0u8; 1]),
            ],
            strides: vec![1, 1, 1],
            width: 1,
            height: 1,
            format: PixelFormat::Yuv420p,
            timestamp: Timestamp::default(),
            key_frame: false,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::redundant_closure_for_method_calls,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use crate::Rational;

    // ==========================================================================
    // Construction Tests
    // ==========================================================================

    #[test]
    fn test_new_rgba_frame() {
        let width = 640u32;
        let height = 480u32;
        let stride = width as usize * 4;
        let data = vec![0u8; stride * height as usize];
        let ts = Timestamp::new(1000, Rational::new(1, 1000));

        let frame = VideoFrame::new(
            vec![PooledBuffer::standalone(data)],
            vec![stride],
            width,
            height,
            PixelFormat::Rgba,
            ts,
            true,
        )
        .unwrap();

        assert_eq!(frame.width(), 640);
        assert_eq!(frame.height(), 480);
        assert_eq!(frame.format(), PixelFormat::Rgba);
        assert_eq!(frame.timestamp(), ts);
        assert!(frame.is_key_frame());
        assert_eq!(frame.num_planes(), 1);
        assert_eq!(frame.stride(0), Some(640 * 4));
    }

    #[test]
    fn test_new_yuv420p_frame() {
        let width = 640u32;
        let height = 480u32;

        let y_stride = width as usize;
        let uv_stride = (width / 2) as usize;
        let uv_height = (height / 2) as usize;

        let y_data = vec![128u8; y_stride * height as usize];
        let u_data = vec![128u8; uv_stride * uv_height];
        let v_data = vec![128u8; uv_stride * uv_height];

        let frame = VideoFrame::new(
            vec![
                PooledBuffer::standalone(y_data),
                PooledBuffer::standalone(u_data),
                PooledBuffer::standalone(v_data),
            ],
            vec![y_stride, uv_stride, uv_stride],
            width,
            height,
            PixelFormat::Yuv420p,
            Timestamp::default(),
            false,
        )
        .unwrap();

        assert_eq!(frame.width(), 640);
        assert_eq!(frame.height(), 480);
        assert_eq!(frame.format(), PixelFormat::Yuv420p);
        assert!(!frame.is_key_frame());
        assert_eq!(frame.num_planes(), 3);
        assert_eq!(frame.stride(0), Some(640));
        assert_eq!(frame.stride(1), Some(320));
        assert_eq!(frame.stride(2), Some(320));
    }

    #[test]
    fn test_new_mismatched_planes_strides() {
        let result = VideoFrame::new(
            vec![PooledBuffer::standalone(vec![0u8; 100])],
            vec![10, 10], // Mismatched length
            10,
            10,
            PixelFormat::Rgba,
            Timestamp::default(),
            false,
        );

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            FrameError::MismatchedPlaneStride {
                planes: 1,
                strides: 2
            }
        );
    }

    // ==========================================================================
    // Metadata Tests
    // ==========================================================================

    #[test]
    fn test_resolution() {
        let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
        assert_eq!(frame.resolution(), (1920, 1080));
    }

    #[test]
    fn test_aspect_ratio() {
        let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
        let aspect = frame.aspect_ratio();
        assert!((aspect - 16.0 / 9.0).abs() < 0.001);

        let frame_4_3 = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
        let aspect_4_3 = frame_4_3.aspect_ratio();
        assert!((aspect_4_3 - 4.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn test_aspect_ratio_zero_height() {
        let frame = VideoFrame::new(
            vec![PooledBuffer::standalone(vec![])],
            vec![0],
            100,
            0,
            PixelFormat::Rgba,
            Timestamp::default(),
            false,
        )
        .unwrap();
        assert_eq!(frame.aspect_ratio(), 0.0);
    }

    #[test]
    fn test_total_size_rgba() {
        let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
        assert_eq!(frame.total_size(), 1920 * 1080 * 4);
    }

    #[test]
    fn test_total_size_yuv420p() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
        // Y: 640*480, U: 320*240, V: 320*240
        let expected = 640 * 480 + 320 * 240 * 2;
        assert_eq!(frame.total_size(), expected);
    }

    #[test]
    fn test_set_key_frame() {
        let mut frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
        assert!(!frame.is_key_frame());

        frame.set_key_frame(true);
        assert!(frame.is_key_frame());

        frame.set_key_frame(false);
        assert!(!frame.is_key_frame());
    }

    #[test]
    fn test_set_timestamp() {
        let mut frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
        let ts = Timestamp::new(90000, Rational::new(1, 90000));

        frame.set_timestamp(ts);
        assert_eq!(frame.timestamp(), ts);
    }

    // ==========================================================================
    // Plane Access Tests
    // ==========================================================================

    #[test]
    fn test_plane_access() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();

        assert!(frame.plane(0).is_some());
        assert!(frame.plane(1).is_some());
        assert!(frame.plane(2).is_some());
        assert!(frame.plane(3).is_none());
    }

    #[test]
    fn test_plane_mut_access() {
        let mut frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();

        if let Some(data) = frame.plane_mut(0) {
            // Fill with red
            for chunk in data.chunks_exact_mut(4) {
                chunk[0] = 255;
                chunk[1] = 0;
                chunk[2] = 0;
                chunk[3] = 255;
            }
        }

        let plane = frame.plane(0).unwrap();
        assert_eq!(plane[0], 255); // R
        assert_eq!(plane[1], 0); // G
        assert_eq!(plane[2], 0); // B
        assert_eq!(plane[3], 255); // A
    }

    #[test]
    fn test_planes_slice() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
        let planes = frame.planes();
        assert_eq!(planes.len(), 3);
    }

    #[test]
    fn test_strides_slice() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
        let strides = frame.strides();
        assert_eq!(strides.len(), 3);
        assert_eq!(strides[0], 640);
        assert_eq!(strides[1], 320);
        assert_eq!(strides[2], 320);
    }

    #[test]
    fn test_stride_out_of_bounds() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
        assert!(frame.stride(0).is_some());
        assert!(frame.stride(1).is_none());
    }

    // ==========================================================================
    // Data Access Tests
    // ==========================================================================

    #[test]
    fn test_data_contiguous() {
        let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
        let data = frame.data();
        assert_eq!(data.len(), 4 * 4 * 4);
    }

    #[test]
    fn test_data_yuv420p_concatenation() {
        let frame = VideoFrame::empty(4, 4, PixelFormat::Yuv420p).unwrap();
        let data = frame.data();
        // Y: 4*4 + U: 2*2 + V: 2*2 = 16 + 4 + 4 = 24
        assert_eq!(data.len(), 24);
    }

    #[test]
    fn test_data_ref_packed() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
        assert!(frame.data_ref().is_some());
        assert_eq!(frame.data_ref().map(|d| d.len()), Some(640 * 480 * 4));
    }

    #[test]
    fn test_data_ref_planar() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
        assert!(frame.data_ref().is_none());
    }

    #[test]
    fn test_data_mut_packed() {
        let mut frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
        assert!(frame.data_mut().is_some());

        if let Some(data) = frame.data_mut() {
            data[0] = 123;
        }

        assert_eq!(frame.plane(0).unwrap()[0], 123);
    }

    #[test]
    fn test_data_mut_planar() {
        let mut frame = VideoFrame::empty(640, 480, PixelFormat::Yuv420p).unwrap();
        assert!(frame.data_mut().is_none());
    }

    // ==========================================================================
    // Clone Tests
    // ==========================================================================

    #[test]
    fn test_clone() {
        let mut original = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
        original.set_key_frame(true);
        original.set_timestamp(Timestamp::new(1000, Rational::new(1, 1000)));

        // Modify some data
        if let Some(data) = original.plane_mut(0) {
            data[0] = 42;
        }

        let cloned = original.clone();

        // Verify metadata matches
        assert_eq!(cloned.width(), original.width());
        assert_eq!(cloned.height(), original.height());
        assert_eq!(cloned.format(), original.format());
        assert_eq!(cloned.timestamp(), original.timestamp());
        assert_eq!(cloned.is_key_frame(), original.is_key_frame());

        // Verify data was cloned
        assert_eq!(cloned.plane(0).unwrap()[0], 42);

        // Verify it's a deep clone (modifying clone doesn't affect original)
        let mut cloned = cloned;
        if let Some(data) = cloned.plane_mut(0) {
            data[0] = 99;
        }
        assert_eq!(original.plane(0).unwrap()[0], 42);
        assert_eq!(cloned.plane(0).unwrap()[0], 99);
    }

    #[test]
    fn video_frame_clone_should_have_identical_data() {
        let width = 320u32;
        let height = 240u32;
        let stride = width as usize;
        let y_data = vec![42u8; stride * height as usize];
        let uv_stride = (width / 2) as usize;
        let uv_data = vec![128u8; uv_stride * (height / 2) as usize];
        let ts = Timestamp::new(1000, Rational::new(1, 1000));

        let original = VideoFrame::new(
            vec![
                PooledBuffer::standalone(y_data.clone()),
                PooledBuffer::standalone(uv_data.clone()),
                PooledBuffer::standalone(uv_data.clone()),
            ],
            vec![stride, uv_stride, uv_stride],
            width,
            height,
            PixelFormat::Yuv420p,
            ts,
            false,
        )
        .unwrap();

        let clone = original.clone();

        assert_eq!(clone.width(), original.width());
        assert_eq!(clone.height(), original.height());
        assert_eq!(clone.format(), original.format());
        assert_eq!(clone.timestamp(), original.timestamp());
        assert_eq!(clone.is_key_frame(), original.is_key_frame());
        assert_eq!(clone.num_planes(), original.num_planes());
        assert_eq!(clone.plane(0), original.plane(0));
    }

    // ==========================================================================
    // Display/Debug Tests
    // ==========================================================================

    #[test]
    fn test_debug() {
        let frame = VideoFrame::empty(640, 480, PixelFormat::Rgba).unwrap();
        let debug = format!("{frame:?}");
        assert!(debug.contains("VideoFrame"));
        assert!(debug.contains("640"));
        assert!(debug.contains("480"));
        assert!(debug.contains("Rgba"));
    }

    #[test]
    fn test_display() {
        let mut frame = VideoFrame::empty(1920, 1080, PixelFormat::Yuv420p).unwrap();
        frame.set_key_frame(true);

        let display = format!("{frame}");
        assert!(display.contains("1920x1080"));
        assert!(display.contains("yuv420p"));
        assert!(display.contains("[KEY]"));
    }

    #[test]
    fn test_display_non_keyframe() {
        let frame = VideoFrame::empty(1920, 1080, PixelFormat::Rgba).unwrap();
        let display = format!("{frame}");
        assert!(!display.contains("[KEY]"));
    }

    #[test]
    fn from_rgba_then_to_rgba_should_round_trip() {
        let pixels: Vec<u8> = (0..(2 * 2 * 4)).map(|i| i as u8).collect();
        let frame = VideoFrame::from_rgba(2, 2, pixels.clone()).unwrap();
        assert_eq!(frame.format(), PixelFormat::Rgba);
        assert_eq!(frame.num_planes(), 1);
        assert_eq!(frame.to_rgba(), Some(pixels));
    }

    #[test]
    fn from_rgba_wrong_size_should_error() {
        let err = VideoFrame::from_rgba(2, 2, vec![0u8; 3]);
        assert!(matches!(
            err,
            Err(crate::error::FrameError::InvalidDataSize { .. })
        ));
    }

    #[test]
    fn to_rgba_non_rgba_should_return_none() {
        let frame = VideoFrame::empty(4, 4, PixelFormat::Yuv420p).unwrap();
        assert_eq!(frame.to_rgba(), None);
    }
}
