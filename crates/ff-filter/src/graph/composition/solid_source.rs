//! Solid-color layer source primitive.
//!
//! [`SolidSource`] generates a solid-color `rgba` frame stream from a [`Color`]
//! plus a canvas size and frame rate, with no timeline/track/clip knowledge. It
//! backs the engine's solid color clips.

use ff_format::{Color, VideoFrame};

use crate::error::FilterError;
use crate::graph::graph::FilterGraph;

/// Builds the `color` filter argument string for a solid fill.
///
/// `c=#RRGGBB@{alpha}:s={w}x{h}:r={fps}` (lowercase hex, alpha as `a/255`),
/// mirroring the base-canvas color source in `build_video_composition`.
pub(super) fn solid_color_args(color: Color, width: u32, height: u32, fps: f64) -> String {
    format!(
        "c=#{:02x}{:02x}{:02x}@{:.3}:s={width}x{height}:r={fps}",
        color.r,
        color.g,
        color.b,
        f32::from(color.a) / 255.0,
    )
}

/// Generates a solid-color `rgba` frame stream.
///
/// Frames are produced sequentially via [`pull`](Self::pull); the source exposes
/// no seek, so a host that seeks rebuilds it. It is model-agnostic: it takes only
/// a color plus the target canvas size and frame rate (a clip's duration is the
/// consumer's concern, not the source's).
pub struct SolidSource {
    graph: FilterGraph,
}

impl SolidSource {
    /// Builds a solid-color source for a `width × height` canvas at `fps`.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError`] when the underlying `FFmpeg` graph cannot be built
    /// (e.g. the `color` filter is unavailable).
    pub fn new(color: Color, width: u32, height: u32, fps: f64) -> Result<Self, FilterError> {
        let args = solid_color_args(color, width, height, fps);
        let graph = super::composition_inner::build_solid_source(&args)?;
        Ok(Self { graph })
    }

    /// Pulls the next generated frame (`rgba`), or `None` when none is buffered yet
    /// or at end-of-stream.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError`] on an unexpected `FFmpeg` error.
    pub fn pull(&mut self) -> Result<Option<VideoFrame>, FilterError> {
        self.graph.pull_video()
    }
}

#[cfg(test)]
mod tests {
    use super::solid_color_args;
    use ff_format::Color;

    #[test]
    fn opaque_red_should_map_to_color_args() {
        let args = solid_color_args(Color::rgb(255, 0, 0), 320, 80, 30.0);
        assert_eq!(args, "c=#ff0000@1.000:s=320x80:r=30");
    }

    #[test]
    fn alpha_should_map_to_fraction() {
        let args = solid_color_args(Color::rgba(0, 128, 255, 128), 16, 16, 25.0);
        assert_eq!(args, "c=#0080ff@0.502:s=16x16:r=25");
    }
}
