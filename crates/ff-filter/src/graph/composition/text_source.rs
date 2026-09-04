//! Text/title layer source primitive.
//!
//! [`TextSource`] renders a pure [`TextSpec`] into a source-only `rgba` frame
//! stream over `drawtext`, with no timeline/track/clip knowledge. It backs the
//! engine's first-class text clips.

use ff_format::{Anchor, Color, TextSpec, VideoFrame};

use crate::error::FilterError;
use crate::graph::graph::FilterGraph;
use crate::graph::types::DrawTextOptions;

/// Formats an RGB color (alpha handled separately) as an `FFmpeg` `0xRRGGBB` token.
fn color_hex(c: Color) -> String {
    format!("0x{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

/// Maps an [`Anchor`] + pixel offset to `drawtext` `x`/`y` expression strings.
///
/// `drawtext` exposes `w`/`h` (canvas) and `text_w`/`text_h` (text box). The
/// offset is applied only when non-zero, keeping the common (centred) case clean.
fn anchor_exprs(anchor: Anchor, offset: (i32, i32)) -> (String, String) {
    let base_x = match anchor {
        Anchor::TopLeft | Anchor::CenterLeft | Anchor::BottomLeft => "0",
        Anchor::TopCenter | Anchor::Center | Anchor::BottomCenter => "(w-text_w)/2",
        Anchor::TopRight | Anchor::CenterRight | Anchor::BottomRight => "w-text_w",
    };
    let base_y = match anchor {
        Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => "0",
        Anchor::CenterLeft | Anchor::Center | Anchor::CenterRight => "(h-text_h)/2",
        Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => "h-text_h",
    };
    let (dx, dy) = offset;
    let x = if dx == 0 {
        base_x.to_string()
    } else {
        format!("({base_x})+({dx})")
    };
    let y = if dy == 0 {
        base_y.to_string()
    } else {
        format!("({base_y})+({dy})")
    };
    (x, y)
}

/// Translates a pure [`TextSpec`] into the FFmpeg-flavoured [`DrawTextOptions`].
pub(super) fn spec_to_drawtext(spec: &TextSpec) -> DrawTextOptions {
    let (x, y) = anchor_exprs(spec.anchor, spec.offset);
    let style = &spec.style;
    DrawTextOptions {
        text: spec.text.clone(),
        x,
        y,
        font_size: style.font_size,
        font_color: color_hex(style.color),
        opacity: f32::from(style.color.a) / 255.0,
        font_file: style
            .font_file
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        box_color: style
            .box_color
            .map(|bc| format!("{}@{:.3}", color_hex(bc), f32::from(bc.a) / 255.0)),
        box_border_width: style.box_border_width,
    }
}

/// Renders a [`TextSpec`] into a source-only `rgba` frame stream.
///
/// The output composites as a transparent overlay (text over a fully transparent
/// canvas). Frames are produced sequentially via [`pull`](Self::pull); the source
/// exposes no seek, so a host that seeks rebuilds it. It is model-agnostic: it
/// takes only a spec plus the target canvas size and frame rate.
pub struct TextSource {
    graph: FilterGraph,
}

impl TextSource {
    /// Builds a text source for a `width × height` canvas at `fps`.
    ///
    /// # Errors
    ///
    /// - [`FilterError::InvalidConfig`] when the text is empty.
    /// - [`FilterError`] when the underlying `FFmpeg` graph cannot be built (e.g.
    ///   `color` / `drawtext` filters are unavailable).
    pub fn new(spec: &TextSpec, width: u32, height: u32, fps: f64) -> Result<Self, FilterError> {
        if spec.text.is_empty() {
            return Err(FilterError::InvalidConfig {
                reason: "text must not be empty".to_string(),
            });
        }
        let opts = spec_to_drawtext(spec);
        let graph = super::composition_inner::build_text_source(&opts, width, height, fps)?;
        Ok(Self { graph })
    }

    /// Pulls the next rendered frame (`rgba`), or `None` when none is buffered yet
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
    use super::{anchor_exprs, spec_to_drawtext};
    use ff_format::{Anchor, Color, TextSpec, TextStyle};

    #[test]
    fn center_anchor_should_map_to_centered_exprs() {
        let (x, y) = anchor_exprs(Anchor::Center, (0, 0));
        assert_eq!(x, "(w-text_w)/2");
        assert_eq!(y, "(h-text_h)/2");
    }

    #[test]
    fn corner_anchor_should_map_to_edge_exprs() {
        let (x, y) = anchor_exprs(Anchor::BottomRight, (0, 0));
        assert_eq!(x, "w-text_w");
        assert_eq!(y, "h-text_h");
        let (x0, y0) = anchor_exprs(Anchor::TopLeft, (0, 0));
        assert_eq!(x0, "0");
        assert_eq!(y0, "0");
    }

    #[test]
    fn nonzero_offset_should_wrap_base_expr() {
        let (x, y) = anchor_exprs(Anchor::Center, (10, -20));
        assert_eq!(x, "((w-text_w)/2)+(10)");
        assert_eq!(y, "((h-text_h)/2)+(-20)");
    }

    #[test]
    fn spec_to_drawtext_should_map_color_and_opacity() {
        let mut spec = TextSpec::new("Hi");
        spec.style = TextStyle {
            color: Color::rgba(255, 0, 0, 128),
            ..TextStyle::default()
        };
        let opts = spec_to_drawtext(&spec);
        assert_eq!(opts.text, "Hi");
        assert_eq!(opts.font_color, "0xFF0000");
        assert!((opts.opacity - 128.0 / 255.0).abs() < 1e-6);
        assert!(opts.box_color.is_none());
    }

    #[test]
    fn spec_to_drawtext_default_should_be_opaque_white() {
        let spec = TextSpec::new("Hi");
        let opts = spec_to_drawtext(&spec);
        assert_eq!(opts.font_color, "0xFFFFFF");
        assert!((opts.opacity - 1.0).abs() < 1e-6);
        assert_eq!(opts.font_size, 48);
        assert!(opts.font_file.is_none());
    }

    #[test]
    fn spec_to_drawtext_should_map_font_file() {
        use std::path::PathBuf;
        let mut spec = TextSpec::new("Hi");
        spec.style = TextStyle {
            font_file: Some(PathBuf::from("/fonts/Roboto.ttf")),
            ..TextStyle::default()
        };
        let opts = spec_to_drawtext(&spec);
        assert_eq!(opts.font_file.as_deref(), Some("/fonts/Roboto.ttf"));
    }

    #[test]
    fn spec_to_drawtext_should_map_box_color_with_alpha() {
        let mut spec = TextSpec::new("Hi");
        spec.style = TextStyle {
            box_color: Some(Color::rgba(0, 0, 0, 255)),
            box_border_width: 8,
            ..TextStyle::default()
        };
        let opts = spec_to_drawtext(&spec);
        assert_eq!(opts.box_color.as_deref(), Some("0x000000@1.000"));
        assert_eq!(opts.box_border_width, 8);
    }
}
