//! Pure text/title layer spec types.
//!
//! [`TextSpec`] and [`TextStyle`] describe a text layer's content, position, and
//! styling as plain values, with **no** `FFmpeg` dependency. A rendering primitive
//! (in `ff-filter`) translates a [`TextSpec`] into a compositable frame; the model
//! (in `avio`) uses it for first-class text clips. Neither concern leaks here.

use std::path::PathBuf;

use crate::color::Color;

/// Where a text layer is anchored within its canvas.
///
/// The renderer places the text box at the anchor, then applies
/// [`TextSpec::offset`] as a pixel nudge from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Anchor {
    /// Top-left corner.
    TopLeft,
    /// Top edge, horizontally centred.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Left edge, vertically centred.
    CenterLeft,
    /// Centre of the canvas. The default.
    #[default]
    Center,
    /// Right edge, vertically centred.
    CenterRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom edge, horizontally centred.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

/// Visual styling for a text layer.
///
/// Mirrors the typical draw-text options as plain values; opacity is carried by
/// the alpha channel of [`color`](Self::color) (and [`box_color`](Self::box_color)).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TextStyle {
    /// Font size in points.
    pub font_size: u32,
    /// Fill color of the text (alpha = opacity).
    pub color: Color,
    /// Optional path to a `TrueType` font file. A default font is used when `None`.
    pub font_file: Option<PathBuf>,
    /// Optional background box color behind the text (alpha = box opacity). No box
    /// when `None`.
    pub box_color: Option<Color>,
    /// Background box padding in pixels. Ignored when [`box_color`](Self::box_color)
    /// is `None`.
    pub box_border_width: u32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_size: 48,
            color: Color::WHITE,
            font_file: None,
            box_color: None,
            box_border_width: 0,
        }
    }
}

/// A text/title layer: the string, its placement, and its styling.
///
/// Size-agnostic: the canvas dimensions are supplied by the renderer, so the same
/// spec can render at any resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TextSpec {
    /// The text to render (UTF-8).
    pub text: String,
    /// Where the text is anchored within the canvas.
    pub anchor: Anchor,
    /// Pixel offset `(dx, dy)` from the anchor (positive = right / down).
    pub offset: (i32, i32),
    /// Visual styling.
    pub style: TextStyle,
}

impl TextSpec {
    /// Creates a centred text spec with default styling.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            anchor: Anchor::Center,
            offset: (0, 0),
            style: TextStyle::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Anchor, TextSpec, TextStyle};
    use crate::color::Color;

    #[test]
    fn anchor_default_should_be_center() {
        assert_eq!(Anchor::default(), Anchor::Center);
    }

    #[test]
    fn text_style_default_should_be_white_48() {
        let s = TextStyle::default();
        assert_eq!(s.font_size, 48);
        assert_eq!(s.color, Color::WHITE);
        assert!(s.font_file.is_none());
        assert!(s.box_color.is_none());
    }

    #[test]
    fn text_spec_new_should_center_with_default_style() {
        let spec = TextSpec::new("Hello");
        assert_eq!(spec.text, "Hello");
        assert_eq!(spec.anchor, Anchor::Center);
        assert_eq!(spec.offset, (0, 0));
        assert_eq!(spec.style, TextStyle::default());
    }
}
