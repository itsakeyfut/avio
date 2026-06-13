//! Compositing and blend filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::composite::CompositeOp;
use ff_format::AlphaMode;

impl FilterGraphBuilder {
    /// Composite a `top` layer over `self` (the bottom) using a Porter-Duff
    /// [`CompositeOp`], `opacity`, and [`AlphaMode`].
    ///
    /// The bottom stream is `self`; the top stream is pushed on input slot 1.
    /// `opacity` is clamped to `[0.0, 1.0]`; it affects only `Over`/`Under`
    /// (the expression operators ignore it). `alpha` selects how the top layer's
    /// alpha is interpreted by the `overlay` filter (`alpha=`);
    /// [`AlphaMode::Straight`] matches `FFmpeg`'s default.
    ///
    /// This is the dedicated alpha-compositing entry point, separate from
    /// [`blend`](FilterGraphBuilder::blend) (which carries the pixel-value
    /// [`BlendMode`] modes); both build the same graphs for the Porter-Duff cases.
    #[must_use]
    pub fn composite(
        mut self,
        top: FilterGraphBuilder,
        op: CompositeOp,
        opacity: f32,
        alpha: AlphaMode,
    ) -> Self {
        let opacity = opacity.clamp(0.0, 1.0);
        self.steps.push(FilterStep::Composite {
            op,
            top: Box::new(top),
            opacity,
            alpha,
        });
        self
    }

    /// Blend a `top` layer over `self` (the bottom) using the given [`BlendMode`],
    /// `opacity`, and [`AlphaMode`].
    ///
    /// `opacity` is clamped to `[0.0, 1.0]` before being stored. `alpha` selects how
    /// the top layer's alpha is interpreted by the `overlay` filter (`alpha=`);
    /// [`AlphaMode::Straight`] matches `FFmpeg`'s default and adds no extra argument.
    ///
    /// # Normal mode
    ///
    /// The bottom stream is `self`; the top stream is pushed on input slot 1.
    /// When `opacity == 1.0` the filter chain is:
    /// ```text
    /// [bottom][top]overlay=format=auto:shortest=1[out]
    /// ```
    /// When `opacity < 1.0` a `colorchannelmixer=aa=<opacity>` step is applied
    /// to the top stream first:
    /// ```text
    /// [top]colorchannelmixer=aa=<opacity>[top_faded];
    /// [bottom][top_faded]overlay=format=auto:shortest=1[out]
    /// ```
    ///
    /// # Unimplemented modes
    ///
    /// All modes other than [`BlendMode::Normal`] are defined but not yet
    /// implemented.  Calling [`build`](FilterGraphBuilder::build) with an
    /// unimplemented mode returns
    /// [`FilterError::InvalidConfig`].
    #[must_use]
    pub fn blend(
        mut self,
        top: FilterGraphBuilder,
        mode: BlendMode,
        opacity: f32,
        alpha: AlphaMode,
    ) -> Self {
        let opacity = opacity.clamp(0.0, 1.0);
        self.steps.push(FilterStep::Blend {
            top: Box::new(top),
            mode,
            opacity,
            alpha,
        });
        self
    }

    /// Key out pixels matching `color` using `FFmpeg`'s `chromakey` filter.
    ///
    /// - `color`: `FFmpeg` color string, e.g. `"green"`, `"0x00FF00"`, `"#00FF00"`.
    /// - `similarity`: match radius in `[0.0, 1.0]`; higher = more pixels removed.
    /// - `blend`: edge softness in `[0.0, 1.0]`; `0.0` = hard edge.
    ///
    /// `similarity` and `blend` are validated in [`build`](FilterGraphBuilder::build);
    /// out-of-range values return [`FilterError::InvalidConfig`].
    ///
    /// The output pixel format is `yuva420p` (adds an alpha channel).
    /// Use this for YCbCr-encoded sources (most video).
    #[must_use]
    pub fn chromakey(mut self, color: &str, similarity: f32, blend: f32) -> Self {
        self.steps.push(FilterStep::ChromaKey {
            color: color.to_string(),
            similarity,
            blend,
        });
        self
    }

    /// Apply a grayscale `matte` as the alpha channel of `self`.
    ///
    /// White (255) in the matte produces fully opaque output; black (0) produces
    /// fully transparent output.  Wraps `FFmpeg`'s `alphamerge` filter.
    ///
    /// The `matte` pipeline is applied to the second input slot (slot 1).
    /// Call [`push_video`](crate::FilterGraph::push_video) with `slot=1` to
    /// supply matte frames at runtime.
    #[must_use]
    pub fn alpha_matte(mut self, matte: FilterGraphBuilder) -> Self {
        self.steps.push(FilterStep::AlphaMatte {
            matte: Box::new(matte),
        });
        self
    }

    /// Reduce color spill from the key color on subject edges.
    ///
    /// Applies `FFmpeg`'s `hue` filter with saturation `1.0 - strength`.
    /// The typical pipeline is `chromakey` → `spill_suppress`.
    ///
    /// `strength` must be in `[0.0, 1.0]`; out-of-range values return
    /// [`FilterError::InvalidConfig`] from [`build`](FilterGraphBuilder::build).
    #[must_use]
    pub fn spill_suppress(mut self, key_color: &str, strength: f32) -> Self {
        self.steps.push(FilterStep::SpillSuppress {
            key_color: key_color.to_string(),
            strength,
        });
        self
    }

    /// Key out pixels by luminance value using `FFmpeg`'s `lumakey` filter.
    ///
    /// - `threshold`: luma cutoff in `[0.0, 1.0]`; `0.0` = black, `1.0` = white.
    /// - `tolerance`: match radius around the threshold in `[0.0, 1.0]`.
    /// - `softness`: edge feather width in `[0.0, 1.0]`; `0.0` = hard edge.
    /// - `invert`: when `false`, keys out pixels matching the threshold; when `true`,
    ///   the alpha channel is negated after keying, making the complementary region
    ///   transparent (useful for dark-background sources).
    ///
    /// `threshold`, `tolerance`, and `softness` are validated in
    /// [`build`](FilterGraphBuilder::build); out-of-range values return
    /// [`FilterError::InvalidConfig`].
    ///
    /// The output pixel format is `yuva420p` (adds an alpha channel).
    #[must_use]
    pub fn lumakey(mut self, threshold: f32, tolerance: f32, softness: f32, invert: bool) -> Self {
        self.steps.push(FilterStep::LumaKey {
            threshold,
            tolerance,
            softness,
            invert,
        });
        self
    }

    /// Apply a rectangular alpha mask using `FFmpeg`'s `geq` filter.
    ///
    /// Pixels inside the rectangle (`x`, `y`, `width`, `height`) are fully
    /// opaque; pixels outside are fully transparent.  When `invert` is `true`
    /// the roles are swapped: inside becomes transparent and outside becomes
    /// opaque.
    ///
    /// `width` and `height` must be > 0; zero values return
    /// [`FilterError::InvalidConfig`] from [`build`](FilterGraphBuilder::build).
    ///
    /// The output carries an alpha channel (`rgba`).
    #[must_use]
    pub fn rect_mask(mut self, x: u32, y: u32, width: u32, height: u32, invert: bool) -> Self {
        self.steps.push(FilterStep::RectMask {
            x,
            y,
            width,
            height,
            invert,
        });
        self
    }

    /// Feather (soften) the alpha channel edges using a Gaussian blur.
    ///
    /// Splits the stream into a color copy and an alpha copy, blurs the alpha
    /// plane with `gblur=sigma=<radius>`, then re-merges via `alphamerge`.
    ///
    /// `radius` is the blur kernel half-size in pixels and must be > 0.
    /// A value of `0` returns [`FilterError::InvalidConfig`] from
    /// [`build`](FilterGraphBuilder::build).
    ///
    /// Typically chained after a keying or masking step such as
    /// [`chromakey`](FilterGraphBuilder::chromakey),
    /// [`rect_mask`](FilterGraphBuilder::rect_mask), or
    /// [`polygon_matte`](FilterGraphBuilder::polygon_matte).
    #[must_use]
    pub fn feather_mask(mut self, radius: u32) -> Self {
        self.steps.push(FilterStep::FeatherMask { radius });
        self
    }

    /// Apply a polygon alpha mask defined by normalised vertex coordinates.
    ///
    /// Pixels inside the polygon are fully opaque; pixels outside are fully
    /// transparent.  When `invert` is `true` the roles are swapped.
    ///
    /// - `vertices`: polygon corners as `(x, y)` in `[0.0, 1.0]` (normalised
    ///   to frame size).  Minimum 3, maximum 16.
    /// - `invert`: when `false`, inside = opaque; when `true`, outside = opaque.
    ///
    /// Vertex count and coordinates are validated in
    /// [`build`](FilterGraphBuilder::build); out-of-range values return
    /// [`FilterError::InvalidConfig`].
    ///
    /// The output carries an alpha channel (`rgba`).
    #[must_use]
    pub fn polygon_matte(mut self, vertices: Vec<(f32, f32)>, invert: bool) -> Self {
        self.steps
            .push(FilterStep::PolygonMatte { vertices, invert });
        self
    }

    /// Key out pixels matching `color` in RGB space using `FFmpeg`'s `colorkey` filter.
    ///
    /// - `color`: `FFmpeg` color string, e.g. `"green"`, `"0x00FF00"`, `"#00FF00"`.
    /// - `similarity`: match radius in `[0.0, 1.0]`; higher = more pixels removed.
    /// - `blend`: edge softness in `[0.0, 1.0]`; `0.0` = hard edge.
    ///
    /// `similarity` and `blend` are validated in [`build`](FilterGraphBuilder::build);
    /// out-of-range values return [`FilterError::InvalidConfig`].
    ///
    /// The output pixel format is `rgba`.
    /// Use this for RGB-encoded sources; prefer [`chromakey`](FilterGraphBuilder::chromakey)
    /// for YCbCr-encoded video.
    #[must_use]
    pub fn colorkey(mut self, color: &str, similarity: f32, blend: f32) -> Self {
        self.steps.push(FilterStep::ColorKey {
            color: color.to_string(),
            similarity,
            blend,
        });
        self
    }
}
