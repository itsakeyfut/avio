//! Porter-Duff alpha-compositing operators.

// CompositeOp

/// Porter-Duff compositing operator for combining two video layers.
///
/// # What the filter path actually computes
///
/// `Over` and `Under` are built with the `overlay` filter and are genuine alpha
/// compositing. **`In`, `Out`, `Atop` and `Xor` are not**: they are built with
/// `blend`'s `all_expr`, which applies one expression to every plane, and the
/// composite chain normalises both inputs to `yuv420p` first, a format with no
/// alpha plane. So they evaluate the Porter-Duff formula with each colour
/// channel standing in for alpha (`In` reduces to a per-channel multiply).
///
/// That is a workaround, not a design: `FFmpeg` has no Porter-Duff filter, and
/// `all_expr` can only reference the same plane of both inputs, so it cannot
/// express a colour term that depends on the *other* input's alpha. Reaching the
/// real operators through libavfilter needs `alphaextract` plus a second
/// `blend`.
///
/// The GPU compositor implements the W3C / Porter-Duff definitions properly
/// (#1670). Preview and export both composite on the GPU, so the split is not
/// between them: these four render one way with an adapter and another whenever
/// the whole-frame fallback swaps in this filter path (a headless machine, a
/// forced-CPU run). Closing that is #1753.
///
/// Unlike [`BlendMode`](crate::BlendMode), which is a colour function of two
/// pixels, these are meant to be alpha algebra. There is no `blend all_mode`
/// token for Porter-Duff compositing, so this type has no `FfmpegToken` impl;
/// each operator maps to a specific `FFmpeg` construction (`overlay` or `blend`
/// with a per-channel expression) in the filter graph.
// Open catalog: the Porter-Duff operator set is added to incrementally.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CompositeOp {
    /// Top layer rendered over the bottom (standard alpha compositing).
    ///
    /// Built via `overlay=format=auto:shortest=1`.
    #[default]
    Over,

    /// Bottom layer rendered over the top; `Over` with the inputs swapped.
    ///
    /// Built via `overlay` with swapped input order.
    Under,

    /// Intended as the top layer masked by the bottom layer's alpha
    /// (intersection). On the filter path the expression runs per colour plane,
    /// so it reduces to a per-channel multiply; see the type docs.
    ///
    /// Built via `blend` with `c0_expr='B*A/255'`.
    In,

    /// Top layer visible only where the bottom layer is transparent.
    ///
    /// Built via `blend` with `c0_expr='B*(255-A)/255'`.
    Out,

    /// Top layer placed atop the bottom; visible only where the bottom is opaque.
    ///
    /// Built via `blend` with `c0_expr='B*A/255 + A*(255-B)/255'`.
    Atop,

    /// Pixels from exactly one layer (XOR of opaque regions).
    ///
    /// Built via `blend` with `c0_expr='B*(255-A)/255 + A*(255-B)/255'`.
    Xor,
}

impl CompositeOp {
    /// Returns the `FFmpeg` `blend` `all_expr` formula for the expression-based
    /// operators (`In`/`Out`/`Atop`/`Xor`), or `None` for `Over`/`Under` which
    /// are built with the `overlay` filter rather than `blend`.
    ///
    /// In the formula, `A` is the bottom pixel and `B` is the top pixel. This is
    /// the single source of these formulas, shared by the `Composite` filter step
    /// and the `MultiTrackComposer` canvas compositing.
    #[must_use]
    pub(crate) fn blend_all_expr(self) -> Option<&'static str> {
        match self {
            Self::Over | Self::Under => None,
            Self::In => Some("B*A/255"),
            Self::Out => Some("B*(255-A)/255"),
            Self::Atop => Some("B*A/255 + A*(255-B)/255"),
            Self::Xor => Some("B*(255-A)/255 + A*(255-B)/255"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CompositeOp;

    #[test]
    fn blend_all_expr_should_be_none_for_overlay_built_operators() {
        assert_eq!(CompositeOp::Over.blend_all_expr(), None);
        assert_eq!(CompositeOp::Under.blend_all_expr(), None);
    }

    #[test]
    fn blend_all_expr_should_return_porter_duff_formula_for_expression_operators() {
        assert_eq!(CompositeOp::In.blend_all_expr(), Some("B*A/255"));
        assert_eq!(CompositeOp::Out.blend_all_expr(), Some("B*(255-A)/255"));
        assert_eq!(
            CompositeOp::Atop.blend_all_expr(),
            Some("B*A/255 + A*(255-B)/255")
        );
        assert_eq!(
            CompositeOp::Xor.blend_all_expr(),
            Some("B*(255-A)/255 + A*(255-B)/255")
        );
    }

    #[test]
    fn composite_op_should_default_to_over() {
        assert_eq!(CompositeOp::default(), CompositeOp::Over);
    }
}
