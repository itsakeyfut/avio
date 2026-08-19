//! Spatial video filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;

impl FilterGraphBuilder {
    /// Scale the video to `width × height` pixels using the given resampling
    /// `algorithm`.
    ///
    /// Use [`ScaleAlgorithm::Fast`] for the best speed/quality trade-off.
    /// For highest quality use [`ScaleAlgorithm::Lanczos`] at the cost of
    /// additional CPU time.
    #[must_use]
    pub fn scale(mut self, width: u32, height: u32, algorithm: ScaleAlgorithm) -> Self {
        self.steps.push(FilterStep::Scale {
            width,
            height,
            algorithm,
        });
        self
    }

    /// Crop a rectangle starting at `(x, y)` with the given dimensions.
    ///
    /// Shorthand for [`crop_animated`](Self::crop_animated) with static values.
    #[must_use]
    pub fn crop(self, x: u32, y: u32, width: u32, height: u32) -> Self {
        self.crop_animated(
            AnimatedValue::Static(f64::from(x)),
            AnimatedValue::Static(f64::from(y)),
            AnimatedValue::Static(f64::from(width)),
            AnimatedValue::Static(f64::from(height)),
        )
    }

    /// Crop with optionally animated boundaries (pixels).
    ///
    /// When an [`AnimatedValue::Track`] is supplied, the corresponding animation
    /// is registered for per-frame `avfilter_graph_send_command` updates (#363).
    /// The initial filter graph is built from the value at [`Duration::ZERO`].
    ///
    /// Filter node names are assigned deterministically: the first call produces
    /// `"crop_0"`, the second `"crop_1"`, and so on.
    #[must_use]
    pub fn crop_animated(
        mut self,
        x: AnimatedValue<f64>,
        y: AnimatedValue<f64>,
        width: AnimatedValue<f64>,
        height: AnimatedValue<f64>,
    ) -> Self {
        let n = self
            .steps
            .iter()
            .filter(|s| matches!(s, FilterStep::CropAnimated { .. }))
            .count();
        let node_name = format!("crop_{n}");
        if let AnimatedValue::Track(track) = &x {
            self.animations.push(AnimationEntry {
                node_name: node_name.clone(),
                param: "x",
                track: track.clone(),
                suffix: "",
            });
        }
        if let AnimatedValue::Track(track) = &y {
            self.animations.push(AnimationEntry {
                node_name: node_name.clone(),
                param: "y",
                track: track.clone(),
                suffix: "",
            });
        }
        if let AnimatedValue::Track(track) = &width {
            self.animations.push(AnimationEntry {
                node_name: node_name.clone(),
                param: "w",
                track: track.clone(),
                suffix: "",
            });
        }
        if let AnimatedValue::Track(track) = &height {
            self.animations.push(AnimationEntry {
                node_name: node_name.clone(),
                param: "h",
                track: track.clone(),
                suffix: "",
            });
        }
        self.steps.push(FilterStep::CropAnimated {
            x,
            y,
            width,
            height,
        });
        self
    }

    /// Scale (resize / zoom) with optionally animated `width`/`height` (pixels).
    ///
    /// Unlike [`crop_animated`](Self::crop_animated) (which uses `send_command`), this
    /// **self-animates** via `scale=eval=frame` with the tracks compiled to
    /// `t`-expressions, so no [`AnimationEntry`] is registered — the same expression
    /// drives preview and export identically. A `Static` side is a constant.
    #[must_use]
    pub fn scale_animated(
        mut self,
        width: AnimatedValue<f64>,
        height: AnimatedValue<f64>,
        algorithm: ScaleAlgorithm,
    ) -> Self {
        self.steps.push(FilterStep::ScaleAnimated {
            width,
            height,
            algorithm,
        });
        self
    }

    /// Rotate with an optionally animated `angle` (degrees), filling exposed corners
    /// with `fill_color` (e.g. `"black"`, or `"none"` for transparent).
    ///
    /// **Self-animates** via `rotate=angle=EXPR` (the `angle` option is re-evaluated per
    /// frame), so no [`AnimationEntry`] is registered — the same expression drives
    /// preview and export. A `Static` angle renders a plain rotate. Output stays the
    /// input size (corners clipped / filled).
    #[must_use]
    pub fn rotate_animated(mut self, angle: AnimatedValue<f64>, fill_color: &str) -> Self {
        self.steps.push(FilterStep::RotateAnimated {
            angle,
            fill_color: fill_color.to_owned(),
        });
        self
    }

    /// Overlay a second input stream at position `(x, y)`.
    #[must_use]
    pub fn overlay(mut self, x: i32, y: i32) -> Self {
        self.steps.push(FilterStep::Overlay { x, y });
        self
    }

    /// Composite a PNG image (watermark / logo) over video.
    ///
    /// The image at `path` is loaded once at graph construction time via
    /// `FFmpeg`'s `movie` source filter. Its alpha channel is scaled by
    /// `opacity` using a `lut` filter, then composited onto the main stream
    /// with the `overlay` filter at position `(x, y)`.
    ///
    /// `x` and `y` are `FFmpeg` expression strings, e.g. `"10"`, `"W-w-10"`.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if:
    /// - the extension is not `.png`,
    /// - the file does not exist at build time, or
    /// - `opacity` is outside `[0.0, 1.0]`.
    #[must_use]
    pub fn overlay_image(mut self, path: &str, x: &str, y: &str, opacity: f32) -> Self {
        self.steps.push(FilterStep::OverlayImage {
            path: path.to_owned(),
            x: x.to_owned(),
            y: y.to_owned(),
            opacity,
            width: None,
            height: None,
        });
        self
    }

    /// Composite a PNG image over video, scaled to `width`×`height`.
    ///
    /// Identical to [`overlay_image`](Self::overlay_image) but inserts a `scale`
    /// filter on the image branch (`movie → scale → lut → overlay`), sizing the
    /// overlay before compositing. `width`/`height` are `FFmpeg` `scale`
    /// expression strings — e.g. `"300"`, `"iw*0.15"` — and `"-1"` preserves the
    /// aspect ratio of the other dimension.
    #[must_use]
    pub fn overlay_image_scaled(
        mut self,
        path: &str,
        x: &str,
        y: &str,
        opacity: f32,
        width: &str,
        height: &str,
    ) -> Self {
        self.steps.push(FilterStep::OverlayImage {
            path: path.to_owned(),
            x: x.to_owned(),
            y: y.to_owned(),
            opacity,
            width: Some(width.to_owned()),
            height: Some(height.to_owned()),
        });
        self
    }

    /// Rotate the video clockwise by `angle_degrees`, filling exposed corners
    /// with `fill_color`.
    ///
    /// `fill_color` accepts any color string understood by `FFmpeg` — for example
    /// `"black"`, `"white"`, `"0x00000000"` (transparent), or `"gray"`.
    /// Pass `"black"` to reproduce the classic solid-background rotation.
    #[must_use]
    pub fn rotate(mut self, angle_degrees: f64, fill_color: &str) -> Self {
        self.steps.push(FilterStep::Rotate {
            angle_degrees,
            fill_color: fill_color.to_owned(),
        });
        self
    }

    /// Flip the video horizontally (mirror left–right) using `FFmpeg`'s `hflip` filter.
    #[must_use]
    pub fn hflip(mut self) -> Self {
        self.steps.push(FilterStep::HFlip);
        self
    }

    /// Flip the video vertically (mirror top–bottom) using `FFmpeg`'s `vflip` filter.
    #[must_use]
    pub fn vflip(mut self) -> Self {
        self.steps.push(FilterStep::VFlip);
        self
    }

    /// Pad the frame to `width × height` pixels, placing the source at `(x, y)`
    /// and filling the exposed borders with `color`.
    ///
    /// Pass a negative value for `x` or `y` to centre the source on that axis
    /// (`x = -1` → `(width − source_w) / 2`).
    ///
    /// `color` accepts any color string understood by `FFmpeg` — for example
    /// `"black"`, `"white"`, `"0x000000"`.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `width` or `height` is zero.
    #[must_use]
    pub fn pad(mut self, width: u32, height: u32, x: i32, y: i32, color: &str) -> Self {
        self.steps.push(FilterStep::Pad {
            width,
            height,
            x,
            y,
            color: color.to_owned(),
        });
        self
    }

    /// Scale the source frame to fit within `width × height` while preserving its
    /// aspect ratio, then centre it on a `width × height` canvas filled with
    /// `color` (letterbox / pillarbox).
    ///
    /// Wide sources (wider aspect ratio than the target) get horizontal black bars
    /// (*letterbox*); tall sources get vertical bars (*pillarbox*).
    ///
    /// `color` accepts any color string understood by `FFmpeg` — for example
    /// `"black"`, `"white"`, `"0x000000"`.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `width` or `height` is zero.
    #[must_use]
    pub fn fit_to_aspect(mut self, width: u32, height: u32, color: &str) -> Self {
        self.steps.push(FilterStep::FitToAspect {
            width,
            height,
            color: color.to_owned(),
        });
        self
    }

    /// Scale the source frame to *cover* `width × height` while preserving its
    /// aspect ratio, then centre-crop the overflow onto a `width × height` canvas
    /// (no bars). The cover counterpart to [`fit_to_aspect`](Self::fit_to_aspect).
    ///
    /// Sources with a different aspect ratio than the target are scaled up until
    /// both dimensions cover the canvas, then the excess is cropped equally from
    /// both sides (centred).
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `width` or `height` is zero.
    #[must_use]
    pub fn fill_to_aspect(mut self, width: u32, height: u32) -> Self {
        self.steps.push(FilterStep::FillToAspect { width, height });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_step_scale_should_produce_correct_args() {
        let step = FilterStep::Scale {
            width: 1280,
            height: 720,
            algorithm: ScaleAlgorithm::Fast,
        };
        assert_eq!(step.filter_name(), "scale");
        assert_eq!(step.args(), "w=1280:h=720:flags=fast_bilinear");
    }

    #[test]
    fn filter_step_scale_lanczos_should_produce_lanczos_flags() {
        let step = FilterStep::Scale {
            width: 1920,
            height: 1080,
            algorithm: ScaleAlgorithm::Lanczos,
        };
        assert_eq!(step.args(), "w=1920:h=1080:flags=lanczos");
    }

    #[test]
    fn filter_step_overlay_should_produce_correct_args() {
        let step = FilterStep::Overlay { x: 10, y: 20 };
        assert_eq!(step.filter_name(), "overlay");
        assert_eq!(step.args(), "x=10:y=20");
    }

    #[test]
    fn filter_step_crop_should_produce_correct_args() {
        let step = FilterStep::Crop {
            x: 0,
            y: 0,
            width: 640,
            height: 360,
        };
        assert_eq!(step.filter_name(), "crop");
        assert_eq!(step.args(), "x=0:y=0:w=640:h=360");
    }

    #[test]
    fn filter_step_rotate_should_produce_correct_args() {
        let step = FilterStep::Rotate {
            angle_degrees: 90.0,
            fill_color: "black".to_owned(),
        };
        assert_eq!(step.filter_name(), "rotate");
        assert_eq!(
            step.args(),
            format!("angle={}:fillcolor=black", 90_f64.to_radians())
        );
    }

    #[test]
    fn filter_step_rotate_transparent_fill_should_produce_correct_args() {
        let step = FilterStep::Rotate {
            angle_degrees: 45.0,
            fill_color: "0x00000000".to_owned(),
        };
        assert_eq!(step.filter_name(), "rotate");
        let args = step.args();
        assert!(
            args.contains("fillcolor=0x00000000"),
            "args should contain transparent fill: {args}"
        );
    }

    #[test]
    fn builder_crop_with_zero_width_should_return_invalid_config() {
        let result = FilterGraph::builder().crop(0, 0, 0, 100).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for width=0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("crop width and height must be > 0"),
                "reason should mention crop dimensions: {reason}"
            );
        }
    }

    #[test]
    fn builder_crop_with_zero_height_should_return_invalid_config() {
        let result = FilterGraph::builder().crop(0, 0, 100, 0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for height=0, got {result:?}"
        );
    }

    #[test]
    fn builder_crop_with_valid_dimensions_should_succeed() {
        let result = FilterGraph::builder().crop(0, 0, 64, 64).build();
        assert!(
            result.is_ok(),
            "crop with valid dimensions must build successfully, got {result:?}"
        );
    }

    #[test]
    fn filter_step_hflip_should_produce_correct_filter_name_and_empty_args() {
        let step = FilterStep::HFlip;
        assert_eq!(step.filter_name(), "hflip");
        assert_eq!(step.args(), "");
    }

    #[test]
    fn filter_step_vflip_should_produce_correct_filter_name_and_empty_args() {
        let step = FilterStep::VFlip;
        assert_eq!(step.filter_name(), "vflip");
        assert_eq!(step.args(), "");
    }

    #[test]
    fn builder_hflip_should_succeed() {
        let result = FilterGraph::builder().hflip().build();
        assert!(
            result.is_ok(),
            "hflip must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_vflip_should_succeed() {
        let result = FilterGraph::builder().vflip().build();
        assert!(
            result.is_ok(),
            "vflip must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_hflip_twice_should_succeed() {
        let result = FilterGraph::builder().hflip().hflip().build();
        assert!(
            result.is_ok(),
            "double hflip (round-trip) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn filter_step_pad_should_produce_correct_filter_name() {
        let step = FilterStep::Pad {
            width: 1920,
            height: 1080,
            x: -1,
            y: -1,
            color: "black".to_owned(),
        };
        assert_eq!(step.filter_name(), "pad");
    }

    #[test]
    fn filter_step_pad_negative_xy_should_produce_centred_args() {
        let step = FilterStep::Pad {
            width: 1920,
            height: 1080,
            x: -1,
            y: -1,
            color: "black".to_owned(),
        };
        assert_eq!(
            step.args(),
            "width=1920:height=1080:x=(ow-iw)/2:y=(oh-ih)/2:color=black"
        );
    }

    #[test]
    fn filter_step_pad_explicit_xy_should_produce_numeric_args() {
        let step = FilterStep::Pad {
            width: 1920,
            height: 1080,
            x: 320,
            y: 180,
            color: "0x000000".to_owned(),
        };
        assert_eq!(
            step.args(),
            "width=1920:height=1080:x=320:y=180:color=0x000000"
        );
    }

    #[test]
    fn filter_step_pad_zero_xy_should_produce_zero_offset_args() {
        let step = FilterStep::Pad {
            width: 1280,
            height: 720,
            x: 0,
            y: 0,
            color: "black".to_owned(),
        };
        assert_eq!(step.args(), "width=1280:height=720:x=0:y=0:color=black");
    }

    #[test]
    fn builder_pad_with_valid_params_should_succeed() {
        let result = FilterGraph::builder()
            .pad(1920, 1080, -1, -1, "black")
            .build();
        assert!(
            result.is_ok(),
            "pad with valid params must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_pad_with_zero_width_should_return_invalid_config() {
        let result = FilterGraph::builder().pad(0, 1080, -1, -1, "black").build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for width=0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("pad width and height must be > 0"),
                "reason should mention pad dimensions: {reason}"
            );
        }
    }

    #[test]
    fn builder_pad_with_zero_height_should_return_invalid_config() {
        let result = FilterGraph::builder().pad(1920, 0, -1, -1, "black").build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for height=0, got {result:?}"
        );
    }

    #[test]
    fn builder_fill_to_aspect_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().fill_to_aspect(1920, 1080).build();
        assert!(
            result.is_ok(),
            "fill_to_aspect with valid params must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_fill_to_aspect_with_zero_width_should_return_invalid_config() {
        let result = FilterGraph::builder().fill_to_aspect(0, 1080).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for width=0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("fill_to_aspect width and height must be > 0"),
                "reason should mention fill_to_aspect dimensions: {reason}"
            );
        }
    }

    #[test]
    fn builder_fill_to_aspect_with_zero_height_should_return_invalid_config() {
        let result = FilterGraph::builder().fill_to_aspect(1920, 0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for height=0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_fit_to_aspect_should_produce_correct_filter_name() {
        let step = FilterStep::FitToAspect {
            width: 1920,
            height: 1080,
            color: "black".to_owned(),
        };
        assert_eq!(step.filter_name(), "scale");
    }

    #[test]
    fn filter_step_fit_to_aspect_should_produce_scale_args_with_force_original_aspect_ratio() {
        let step = FilterStep::FitToAspect {
            width: 1920,
            height: 1080,
            color: "black".to_owned(),
        };
        let args = step.args();
        assert!(
            args.contains("w=1920") && args.contains("h=1080"),
            "args must contain target dimensions: {args}"
        );
        assert!(
            args.contains("force_original_aspect_ratio=decrease"),
            "args must request aspect-ratio-preserving scale: {args}"
        );
    }

    #[test]
    fn builder_fit_to_aspect_with_valid_params_should_succeed() {
        let result = FilterGraph::builder()
            .fit_to_aspect(1920, 1080, "black")
            .build();
        assert!(
            result.is_ok(),
            "fit_to_aspect with valid params must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_fit_to_aspect_with_zero_width_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .fit_to_aspect(0, 1080, "black")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for width=0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("fit_to_aspect width and height must be > 0"),
                "reason should mention fit_to_aspect dimensions: {reason}"
            );
        }
    }

    #[test]
    fn builder_fit_to_aspect_with_zero_height_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .fit_to_aspect(1920, 0, "black")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for height=0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_overlay_image_should_produce_correct_filter_name() {
        let step = FilterStep::OverlayImage {
            path: "logo.png".to_owned(),
            x: "10".to_owned(),
            y: "10".to_owned(),
            opacity: 1.0,
            width: None,
            height: None,
        };
        assert_eq!(step.filter_name(), "overlay");
    }

    #[test]
    fn filter_step_overlay_image_should_produce_correct_args() {
        let step = FilterStep::OverlayImage {
            path: "logo.png".to_owned(),
            x: "W-w-10".to_owned(),
            y: "H-h-10".to_owned(),
            opacity: 0.7,
            width: None,
            height: None,
        };
        assert_eq!(step.args(), "W-w-10:H-h-10");
    }

    #[test]
    fn builder_overlay_image_defaults_to_native_size() {
        // The plain `overlay_image` builder must not request any scaling — the
        // image composites at its native resolution (width/height are None).
        let steps = FilterGraph::builder()
            .overlay_image("logo.png", "10", "10", 1.0)
            .steps()
            .to_vec();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            FilterStep::OverlayImage { width, height, .. } => {
                assert!(width.is_none());
                assert!(height.is_none());
            }
            other => panic!("expected OverlayImage, got {other:?}"),
        }
    }

    #[test]
    fn builder_overlay_image_scaled_carries_size_exprs() {
        // `overlay_image_scaled` records the scale expressions on the step; the
        // filter name / positional args are unchanged (scale is a separate node
        // on the image branch, built by `add_overlay_image_step`).
        let steps = FilterGraph::builder()
            .overlay_image_scaled("logo.png", "W-w-10", "H-h-10", 0.8, "iw*0.15", "-1")
            .steps()
            .to_vec();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            FilterStep::OverlayImage {
                width,
                height,
                opacity,
                ..
            } => {
                assert_eq!(width.as_deref(), Some("iw*0.15"));
                assert_eq!(height.as_deref(), Some("-1"));
                assert!((*opacity - 0.8).abs() < 1e-6);
            }
            other => panic!("expected OverlayImage, got {other:?}"),
        }
        assert_eq!(steps[0].filter_name(), "overlay");
        assert_eq!(steps[0].args(), "W-w-10:H-h-10");
    }

    #[test]
    fn builder_overlay_image_with_wrong_extension_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .overlay_image("logo.jpg", "10", "10", 1.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for wrong extension, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("unsupported image format"),
                "reason should mention unsupported format: {reason}"
            );
        }
    }

    #[test]
    fn builder_overlay_image_with_no_extension_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .overlay_image("logo_no_ext", "10", "10", 1.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for missing extension, got {result:?}"
        );
    }

    #[test]
    fn builder_overlay_image_with_nonexistent_file_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .overlay_image("/nonexistent/path/logo_ab12cd.png", "10", "10", 1.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for nonexistent file, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("overlay image not found"),
                "reason should mention file not found: {reason}"
            );
        }
    }

    #[test]
    fn builder_overlay_image_with_opacity_above_1_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .overlay_image("/nonexistent/logo.png", "10", "10", 1.1)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for opacity > 1.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("opacity"),
                "reason should mention opacity: {reason}"
            );
        }
    }

    #[test]
    fn builder_overlay_image_with_negative_opacity_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .overlay_image("/nonexistent/logo.png", "10", "10", -0.1)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for opacity < 0.0, got {result:?}"
        );
    }

    #[test]
    fn crop_animated_static_values_should_produce_same_args_as_crop() {
        let step_animated = FilterStep::CropAnimated {
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            width: AnimatedValue::Static(640.0),
            height: AnimatedValue::Static(360.0),
        };
        assert_eq!(step_animated.filter_name(), "crop");
        assert_eq!(step_animated.args(), "x=0:y=0:w=640:h=360");
    }

    #[test]
    fn crop_animated_with_zero_width_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .crop_animated(
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(100.0),
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for width=0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("crop width and height must be > 0"),
                "reason should mention crop dimensions: {reason}"
            );
        }
    }

    #[test]
    fn crop_animated_with_zero_height_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .crop_animated(
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(100.0),
                AnimatedValue::Static(0.0),
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for height=0, got {result:?}"
        );
    }

    #[test]
    fn crop_animated_with_valid_dimensions_should_succeed() {
        let result = FilterGraph::builder()
            .crop_animated(
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(64.0),
                AnimatedValue::Static(64.0),
            )
            .build();
        assert!(
            result.is_ok(),
            "crop_animated with valid dimensions must build successfully, got {result:?}"
        );
    }

    #[test]
    fn crop_animated_with_track_should_register_animation_entry() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let track = crate::animation::AnimationTrack::new()
            .push(Keyframe {
                timestamp: Duration::ZERO,
                value: 0.0_f64,
                easing: Easing::Linear,
            })
            .push(Keyframe {
                timestamp: Duration::from_secs(1),
                value: 100.0_f64,
                easing: Easing::Linear,
            });

        let graph = FilterGraph::builder()
            .crop_animated(
                AnimatedValue::Track(track),
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(64.0),
                AnimatedValue::Static(64.0),
            )
            .build()
            .unwrap();

        assert_eq!(
            graph.pending_animations.len(),
            1,
            "one Track param should produce one AnimationEntry"
        );
        assert_eq!(graph.pending_animations[0].node_name, "crop_0");
        assert_eq!(graph.pending_animations[0].param, "x");
    }

    #[test]
    fn crop_animated_all_track_params_should_register_four_entries() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let make_track = |start: f64, end: f64| {
            crate::animation::AnimationTrack::new()
                .push(Keyframe {
                    timestamp: Duration::ZERO,
                    value: start,
                    easing: Easing::Linear,
                })
                .push(Keyframe {
                    timestamp: Duration::from_secs(1),
                    value: end,
                    easing: Easing::Linear,
                })
        };

        let graph = FilterGraph::builder()
            .crop_animated(
                AnimatedValue::Track(make_track(0.0, 0.0)),
                AnimatedValue::Track(make_track(0.0, 0.0)),
                AnimatedValue::Track(make_track(640.0, 320.0)), // starts at 640 (valid)
                AnimatedValue::Track(make_track(360.0, 180.0)), // starts at 360 (valid)
            )
            .build()
            .unwrap();

        assert_eq!(
            graph.pending_animations.len(),
            4,
            "four Track params should register four AnimationEntry items"
        );
        let params: Vec<&str> = graph.pending_animations.iter().map(|e| e.param).collect();
        assert_eq!(params, ["x", "y", "w", "h"]);
        assert!(
            graph
                .pending_animations
                .iter()
                .all(|e| e.node_name == "crop_0"),
            "all entries must point to crop_0"
        );
    }

    #[test]
    fn crop_animated_second_call_should_use_crop_1_node_name() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let track = crate::animation::AnimationTrack::new().push(Keyframe {
            timestamp: Duration::ZERO,
            value: 50.0_f64,
            easing: Easing::Linear,
        });

        let graph = FilterGraph::builder()
            .crop_animated(
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(64.0),
                AnimatedValue::Static(64.0),
            )
            .crop_animated(
                AnimatedValue::Track(track),
                AnimatedValue::Static(0.0),
                AnimatedValue::Static(64.0),
                AnimatedValue::Static(64.0),
            )
            .build()
            .unwrap();

        assert_eq!(graph.pending_animations.len(), 1);
        assert_eq!(
            graph.pending_animations[0].node_name, "crop_1",
            "second crop_animated call must produce node name crop_1"
        );
    }
}
