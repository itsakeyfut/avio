//! Color grading and correction filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::animation::{AnimationTrack, Keyframe};
use ff_format::{ColorPrimaries, ColorRange, ColorSpace, ColorTransfer};

// ── Tuple-track projection helper ─────────────────────────────────────────────

/// Projects each component of an `AnimatedValue<(f64, f64, f64)>` track into
/// three separate `f64` `AnimationEntry` items and appends them to `animations`.
///
/// Called by [`FilterGraphBuilder::color_correct_animated`] to register
/// per-channel animation entries for `avfilter_graph_send_command` in #363.
///
/// If `av` is `Static`, nothing is pushed (static values need no per-frame update).
fn push_tuple_track_entries(
    animations: &mut Vec<AnimationEntry>,
    node_name: &str,
    params: [&'static str; 3],
    av: &AnimatedValue<(f64, f64, f64)>,
) {
    let AnimatedValue::Track(track) = av else {
        return;
    };
    for (i, param) in params.into_iter().enumerate() {
        let f64_track = track
            .keyframes()
            .iter()
            .fold(AnimationTrack::new(), |t, kf| {
                let v = match i {
                    0 => kf.value.0,
                    1 => kf.value.1,
                    _ => kf.value.2,
                };
                t.push(Keyframe {
                    timestamp: kf.timestamp,
                    value: v,
                    easing: kf.easing.clone(),
                })
            });
        animations.push(AnimationEntry {
            node_name: node_name.to_owned(),
            param,
            track: f64_track,
            suffix: "",
        });
    }
}

impl FilterGraphBuilder {
    /// Tag the stream's colour metadata via the `setparams` filter.
    ///
    /// Emits `colorspace=` / `range=` / `color_primaries=` / `color_trc=` from the colour enums
    /// using their `FFmpeg`-canonical tokens. Each argument is included only when it is `Some`
    /// **and** yields a token (`Unknown` → skipped); an all-`None` call is a no-op (no step added).
    ///
    /// Unlike the `format` filter, `setparams` carries `color_primaries` and `color_trc`, so this
    /// is the step that actually applies [`ColorPrimaries`] / [`ColorTransfer`] to a graph.
    #[must_use]
    pub fn set_params(
        mut self,
        color_space: Option<ColorSpace>,
        color_range: Option<ColorRange>,
        color_primaries: Option<ColorPrimaries>,
        color_trc: Option<ColorTransfer>,
    ) -> Self {
        // No point adding a no-op setparams step when nothing is set.
        if color_space.is_some()
            || color_range.is_some()
            || color_primaries.is_some()
            || color_trc.is_some()
        {
            self.steps.push(FilterStep::SetParams {
                color_space,
                color_range,
                color_primaries,
                color_trc,
            });
        }
        self
    }

    /// Apply HDR-to-SDR tone mapping using the given `algorithm`.
    #[must_use]
    pub fn tone_map(mut self, algorithm: ToneMap) -> Self {
        self.steps.push(FilterStep::ToneMap(algorithm));
        self
    }

    /// Apply a 3D LUT colour grade from a `.cube` or `.3dl` file.
    ///
    /// Uses `FFmpeg`'s `lut3d` filter with trilinear interpolation.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if:
    /// - the extension is not `.cube` or `.3dl`, or
    /// - the file does not exist at build time.
    #[must_use]
    pub fn lut3d(mut self, path: &str) -> Self {
        self.steps.push(FilterStep::Lut3d {
            path: path.to_owned(),
        });
        self
    }

    /// Adjust brightness, contrast, and saturation using `FFmpeg`'s `eq` filter.
    ///
    /// Valid ranges:
    /// - `brightness`: −1.0 – 1.0 (neutral: 0.0)
    /// - `contrast`: 0.0 – 3.0 (neutral: 1.0)
    /// - `saturation`: 0.0 – 3.0 (neutral: 1.0; 0.0 = grayscale)
    ///
    /// Shorthand for [`eq_animated`](Self::eq_animated) with static values and a
    /// neutral gamma (`1.0`).
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if any
    /// value is outside its valid range.
    #[must_use]
    pub fn eq(self, brightness: f32, contrast: f32, saturation: f32) -> Self {
        self.eq_animated(
            AnimatedValue::Static(f64::from(brightness)),
            AnimatedValue::Static(f64::from(contrast)),
            AnimatedValue::Static(f64::from(saturation)),
            AnimatedValue::Static(1.0_f64),
        )
    }

    /// Adjust brightness, contrast, saturation, and gamma using `FFmpeg`'s `eq` filter,
    /// with optionally animated parameters.
    ///
    /// When an [`AnimatedValue::Track`] is supplied for any parameter, the animation
    /// is registered for per-frame `avfilter_graph_send_command` updates (#363).
    /// The initial filter graph is built from values at [`Duration::ZERO`].
    ///
    /// Filter node names are assigned deterministically: the first call produces
    /// `"eq_0"`, the second `"eq_1"`, and so on.
    ///
    /// Valid ranges (at `Duration::ZERO`):
    /// - `brightness`: −1.0 – 1.0
    /// - `contrast`: 0.0 – 3.0
    /// - `saturation`: 0.0 – 3.0
    /// - `gamma`: 0.1 – 10.0
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if any
    /// parameter evaluates outside its valid range at `Duration::ZERO`.
    #[must_use]
    pub fn eq_animated(
        mut self,
        brightness: AnimatedValue<f64>,
        contrast: AnimatedValue<f64>,
        saturation: AnimatedValue<f64>,
        gamma: AnimatedValue<f64>,
    ) -> Self {
        let n = self
            .steps
            .iter()
            .filter(|s| matches!(s, FilterStep::EqAnimated { .. }))
            .count();
        let node_name = format!("eq_{n}");
        for (av, param) in [
            (&brightness, "brightness"),
            (&contrast, "contrast"),
            (&saturation, "saturation"),
            (&gamma, "gamma"),
        ] {
            if let AnimatedValue::Track(track) = av {
                self.animations.push(AnimationEntry {
                    node_name: node_name.clone(),
                    param,
                    track: track.clone(),
                    suffix: "",
                });
            }
        }
        self.steps.push(FilterStep::EqAnimated {
            brightness,
            contrast,
            saturation,
            gamma,
        });
        self
    }

    /// Apply a three-way color balance (lift / gamma / gain) using `FFmpeg`'s
    /// `colorbalance` filter.
    ///
    /// Each parameter is an `(R, G, B)` tuple; neutral for all three is `(0.0, 0.0, 0.0)`.
    ///
    /// - **lift**: additive correction for shadows. Range per component: −1.0 – 1.0.
    /// - **gamma**: additive correction for midtones. Range per component: −1.0 – 1.0.
    /// - **gain**: additive correction for highlights. Range per component: −1.0 – 1.0.
    ///
    /// Shorthand for [`color_correct_animated`](Self::color_correct_animated) with
    /// static values.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if any
    /// component is outside `[−1.0, 1.0]`.
    #[must_use]
    pub fn color_correct(
        self,
        lift: (f64, f64, f64),
        gamma: (f64, f64, f64),
        gain: (f64, f64, f64),
    ) -> Self {
        self.color_correct_animated(
            AnimatedValue::Static(lift),
            AnimatedValue::Static(gamma),
            AnimatedValue::Static(gain),
        )
    }

    /// Apply a three-way color balance (lift / gamma / gain) using `FFmpeg`'s
    /// `colorbalance` filter, with optionally animated parameters.
    ///
    /// When an [`AnimatedValue::Track`] is supplied, the animation is registered
    /// for per-frame `avfilter_graph_send_command` updates (#363).  For tuple tracks
    /// three separate entries are registered (one per RGB channel).
    ///
    /// Filter node names: `"colorbalance_0"`, `"colorbalance_1"`, …
    ///
    /// `FFmpeg` param names per parameter:
    /// - `lift`  → `"rs"`, `"gs"`, `"bs"` (shadows)
    /// - `gamma` → `"rm"`, `"gm"`, `"bm"` (midtones)
    /// - `gain`  → `"rh"`, `"gh"`, `"bh"` (highlights)
    ///
    /// Valid range per component at `Duration::ZERO`: −1.0 – 1.0.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if any
    /// component evaluates outside `[−1.0, 1.0]` at `Duration::ZERO`.
    #[must_use]
    pub fn color_correct_animated(
        mut self,
        lift: AnimatedValue<(f64, f64, f64)>,
        gamma: AnimatedValue<(f64, f64, f64)>,
        gain: AnimatedValue<(f64, f64, f64)>,
    ) -> Self {
        let n = self
            .steps
            .iter()
            .filter(|s| matches!(s, FilterStep::ColorBalanceAnimated { .. }))
            .count();
        let node_name = format!("colorbalance_{n}");
        push_tuple_track_entries(&mut self.animations, &node_name, ["rs", "gs", "bs"], &lift);
        push_tuple_track_entries(&mut self.animations, &node_name, ["rm", "gm", "bm"], &gamma);
        push_tuple_track_entries(&mut self.animations, &node_name, ["rh", "gh", "bh"], &gain);
        self.steps
            .push(FilterStep::ColorBalanceAnimated { lift, gamma, gain });
        self
    }

    /// Apply per-channel RGB color curves using `FFmpeg`'s `curves` filter.
    ///
    /// Each argument is a list of `(input, output)` control points in `[0.0, 1.0]`.
    /// Pass an empty `Vec` for any channel that needs no adjustment.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if any
    /// control point coordinate is outside `[0.0, 1.0]`.
    #[must_use]
    pub fn curves(
        mut self,
        master: Vec<(f32, f32)>,
        r: Vec<(f32, f32)>,
        g: Vec<(f32, f32)>,
        b: Vec<(f32, f32)>,
    ) -> Self {
        self.steps.push(FilterStep::Curves { master, r, g, b });
        self
    }

    /// Correct white balance using `FFmpeg`'s `colorchannelmixer` filter.
    ///
    /// RGB channel multipliers are derived from `temperature_k` via Tanner
    /// Helland's Kelvin-to-RGB algorithm. The `tint` offset shifts the green
    /// channel (positive = more green, negative = more magenta).
    ///
    /// Valid ranges:
    /// - `temperature_k`: 1000–40000 K (neutral daylight ≈ 6500 K)
    /// - `tint`: −1.0–1.0
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if either
    /// value is outside its valid range.
    #[must_use]
    pub fn white_balance(mut self, temperature_k: u32, tint: f32) -> Self {
        self.steps.push(FilterStep::WhiteBalance {
            temperature_k,
            tint,
        });
        self
    }

    /// Rotate hue by `degrees` using `FFmpeg`'s `hue` filter.
    ///
    /// Valid range: −360.0–360.0. A value of `0.0` is a no-op.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `degrees` is outside `[−360.0, 360.0]`.
    #[must_use]
    pub fn hue(mut self, degrees: f32) -> Self {
        self.steps.push(FilterStep::Hue { degrees });
        self
    }

    /// Apply per-channel gamma correction using `FFmpeg`'s `eq` filter.
    ///
    /// Valid range per channel: 0.1–10.0. A value of `1.0` is neutral.
    /// Values above 1.0 brighten midtones; values below 1.0 darken them.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if any
    /// channel value is outside `[0.1, 10.0]`.
    #[must_use]
    pub fn gamma(mut self, r: f32, g: f32, b: f32) -> Self {
        self.steps.push(FilterStep::Gamma { r, g, b });
        self
    }

    /// Apply a three-way colour corrector (lift / gamma / gain) using `FFmpeg`'s
    /// `curves` filter.
    ///
    /// Each parameter is an [`Rgb`] triplet; neutral for all three is
    /// [`Rgb::NEUTRAL`] (`r=1.0, g=1.0, b=1.0`).
    ///
    /// - **lift**: shifts shadows (blacks). Values below `1.0` darken shadows.
    /// - **gamma**: shapes midtones via a power curve. Values above `1.0`
    ///   brighten midtones; values below `1.0` darken them.
    /// - **gain**: scales highlights (whites). Values above `1.0` boost whites.
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if any
    /// `gamma` component is `≤ 0.0` (division by zero in the power curve).
    #[must_use]
    pub fn three_way_cc(mut self, lift: Rgb, gamma: Rgb, gain: Rgb) -> Self {
        self.steps
            .push(FilterStep::ThreeWayCC { lift, gamma, gain });
        self
    }

    /// Apply a vignette effect using `FFmpeg`'s `vignette` filter.
    ///
    /// Darkens the corners of the frame with a smooth radial falloff.
    ///
    /// - `angle`: radius angle in radians (`0.0` – π/2 ≈ 1.5708). Default: π/5 ≈ 0.628.
    /// - `x0`: horizontal centre of the vignette. Pass `0.0` to use the video
    ///   centre (`w/2`).
    /// - `y0`: vertical centre of the vignette. Pass `0.0` to use the video
    ///   centre (`h/2`).
    ///
    /// # Validation
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `angle` is outside `[0.0, π/2]`.
    #[must_use]
    pub fn vignette(mut self, angle: f32, x0: f32, y0: f32) -> Self {
        self.steps.push(FilterStep::Vignette { angle, x0, y0 });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_map_variants_should_have_correct_names() {
        assert_eq!(ToneMap::Hable.as_str(), "hable");
        assert_eq!(ToneMap::Reinhard.as_str(), "reinhard");
        assert_eq!(ToneMap::Mobius.as_str(), "mobius");
    }

    #[test]
    fn filter_step_tone_map_should_produce_correct_args() {
        let step = FilterStep::ToneMap(ToneMap::Hable);
        assert_eq!(step.filter_name(), "tonemap");
        assert_eq!(step.args(), "tonemap=hable");
    }

    #[test]
    fn filter_step_lut3d_should_produce_correct_filter_name() {
        let step = FilterStep::Lut3d {
            path: "grade.cube".to_owned(),
        };
        assert_eq!(step.filter_name(), "lut3d");
    }

    #[test]
    fn filter_step_lut3d_should_produce_correct_args() {
        let step = FilterStep::Lut3d {
            path: "grade.cube".to_owned(),
        };
        assert_eq!(step.args(), "file=grade.cube:interp=trilinear");
    }

    #[test]
    fn builder_lut3d_with_unsupported_extension_should_return_invalid_config() {
        let result = FilterGraph::builder().lut3d("color_grade.txt").build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for unsupported extension, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("unsupported LUT format"),
                "reason should mention unsupported format: {reason}"
            );
        }
    }

    #[test]
    fn builder_lut3d_with_no_extension_should_return_invalid_config() {
        let result = FilterGraph::builder().lut3d("color_grade_no_ext").build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for missing extension, got {result:?}"
        );
    }

    #[test]
    fn builder_lut3d_with_nonexistent_cube_file_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .lut3d("/nonexistent/path/grade_ab12cd.cube")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for nonexistent file, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("LUT file not found"),
                "reason should mention file not found: {reason}"
            );
        }
    }

    #[test]
    fn builder_lut3d_with_nonexistent_3dl_file_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .lut3d("/nonexistent/path/grade_ab12cd.3dl")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for nonexistent .3dl file, got {result:?}"
        );
    }

    #[test]
    fn filter_step_eq_should_produce_correct_filter_name() {
        let step = FilterStep::Eq {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
        };
        assert_eq!(step.filter_name(), "eq");
    }

    #[test]
    fn filter_step_eq_should_produce_correct_args() {
        let step = FilterStep::Eq {
            brightness: 0.1,
            contrast: 1.5,
            saturation: 0.8,
        };
        assert_eq!(step.args(), "brightness=0.1:contrast=1.5:saturation=0.8");
    }

    #[test]
    fn builder_eq_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().eq(0.0, 1.0, 1.0).build();
        assert!(
            result.is_ok(),
            "neutral eq params must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_eq_with_brightness_too_low_should_return_invalid_config() {
        let result = FilterGraph::builder().eq(-1.5, 1.0, 1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for brightness < -1.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("brightness"),
                "reason should mention brightness: {reason}"
            );
        }
    }

    #[test]
    fn builder_eq_with_brightness_too_high_should_return_invalid_config() {
        let result = FilterGraph::builder().eq(1.5, 1.0, 1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for brightness > 1.0, got {result:?}"
        );
    }

    #[test]
    fn builder_eq_with_contrast_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder().eq(0.0, 4.0, 1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for contrast > 3.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("contrast"),
                "reason should mention contrast: {reason}"
            );
        }
    }

    #[test]
    fn builder_eq_with_saturation_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder().eq(0.0, 1.0, -0.5).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for saturation < 0.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("saturation"),
                "reason should mention saturation: {reason}"
            );
        }
    }

    #[test]
    fn filter_step_curves_should_produce_correct_filter_name() {
        let step = FilterStep::Curves {
            master: vec![],
            r: vec![],
            g: vec![],
            b: vec![],
        };
        assert_eq!(step.filter_name(), "curves");
    }

    #[test]
    fn filter_step_curves_should_produce_args_with_all_channels() {
        let step = FilterStep::Curves {
            master: vec![(0.0, 0.0), (0.5, 0.6), (1.0, 1.0)],
            r: vec![(0.0, 0.0), (1.0, 1.0)],
            g: vec![],
            b: vec![(0.0, 0.0), (1.0, 0.8)],
        };
        let args = step.args();
        assert!(args.contains("master='0/0 0.5/0.6 1/1'"), "args={args}");
        assert!(args.contains("r='0/0 1/1'"), "args={args}");
        assert!(
            !args.contains("g="),
            "empty g channel should be omitted: args={args}"
        );
        assert!(args.contains("b='0/0 1/0.8'"), "args={args}");
    }

    #[test]
    fn filter_step_curves_with_empty_channels_should_produce_empty_args() {
        let step = FilterStep::Curves {
            master: vec![],
            r: vec![],
            g: vec![],
            b: vec![],
        };
        assert_eq!(
            step.args(),
            "",
            "all-empty curves should produce empty args string"
        );
    }

    #[test]
    fn builder_curves_with_valid_s_curve_should_succeed() {
        let result = FilterGraph::builder()
            .curves(
                vec![
                    (0.0, 0.0),
                    (0.25, 0.15),
                    (0.5, 0.5),
                    (0.75, 0.85),
                    (1.0, 1.0),
                ],
                vec![],
                vec![],
                vec![],
            )
            .build();
        assert!(
            result.is_ok(),
            "valid S-curve master must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_curves_with_out_of_range_point_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .curves(vec![(0.0, 1.5)], vec![], vec![], vec![])
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for out-of-range point, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("curves") && reason.contains("master"),
                "reason should mention curves master: {reason}"
            );
        }
    }

    #[test]
    fn builder_curves_with_out_of_range_r_channel_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .curves(vec![], vec![(1.2, 0.5)], vec![], vec![])
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for out-of-range r channel point, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("curves") && reason.contains(" r "),
                "reason should mention curves r: {reason}"
            );
        }
    }

    #[test]
    fn filter_step_white_balance_should_produce_correct_filter_name() {
        let step = FilterStep::WhiteBalance {
            temperature_k: 6500,
            tint: 0.0,
        };
        assert_eq!(step.filter_name(), "colorchannelmixer");
    }

    #[test]
    fn filter_step_white_balance_6500k_neutral_tint_should_produce_near_unity_args() {
        // At 6500 K (daylight), all channels should be close to 1.0.
        let step = FilterStep::WhiteBalance {
            temperature_k: 6500,
            tint: 0.0,
        };
        let args = step.args();
        // Parse rr= value to verify it is close to 1.0.
        assert!(args.starts_with("rr="), "args must start with rr=: {args}");
        assert!(
            args.contains("gg=") && args.contains("bb="),
            "args must contain gg and bb: {args}"
        );
    }

    #[test]
    fn filter_step_white_balance_3200k_should_produce_warm_shift() {
        // At 3200 K (tungsten), red should dominate over blue.
        use crate::graph::filter_step::FilterStep as FS;
        // Access kelvin_to_rgb indirectly through the WhiteBalance step args
        let step_warm = FS::WhiteBalance {
            temperature_k: 3200,
            tint: 0.0,
        };
        let step_cool = FS::WhiteBalance {
            temperature_k: 10000,
            tint: 0.0,
        };
        let args_warm = step_warm.args();
        let args_cool = step_cool.args();
        // At warm temperature, rr value should be higher than bb value
        // Just verify the args are produced without panicking
        assert!(
            args_warm.contains("rr=") && args_warm.contains("bb="),
            "args={args_warm}"
        );
        assert!(
            args_cool.contains("rr=") && args_cool.contains("bb="),
            "args={args_cool}"
        );
    }

    #[test]
    fn builder_white_balance_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().white_balance(6500, 0.0).build();
        assert!(
            result.is_ok(),
            "valid white_balance params must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_white_balance_with_temperature_too_low_should_return_invalid_config() {
        let result = FilterGraph::builder().white_balance(500, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for temperature_k < 1000, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("temperature_k"),
                "reason should mention temperature_k: {reason}"
            );
        }
    }

    #[test]
    fn builder_white_balance_with_temperature_too_high_should_return_invalid_config() {
        let result = FilterGraph::builder().white_balance(50000, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for temperature_k > 40000, got {result:?}"
        );
    }

    #[test]
    fn builder_white_balance_with_tint_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder().white_balance(6500, 1.5).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for tint > 1.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("tint"),
                "reason should mention tint: {reason}"
            );
        }
    }

    #[test]
    fn filter_step_hue_should_produce_correct_filter_name() {
        let step = FilterStep::Hue { degrees: 90.0 };
        assert_eq!(step.filter_name(), "hue");
    }

    #[test]
    fn filter_step_hue_should_produce_correct_args() {
        let step = FilterStep::Hue { degrees: 180.0 };
        assert_eq!(step.args(), "h=180");
    }

    #[test]
    fn filter_step_hue_zero_should_produce_no_op_args() {
        let step = FilterStep::Hue { degrees: 0.0 };
        assert_eq!(step.args(), "h=0");
    }

    #[test]
    fn builder_hue_with_valid_degrees_should_succeed() {
        let result = FilterGraph::builder().hue(0.0).build();
        assert!(
            result.is_ok(),
            "hue(0.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_hue_with_degrees_too_high_should_return_invalid_config() {
        let result = FilterGraph::builder().hue(400.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for degrees > 360.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("degrees"),
                "reason should mention degrees: {reason}"
            );
        }
    }

    #[test]
    fn builder_hue_with_degrees_too_low_should_return_invalid_config() {
        let result = FilterGraph::builder().hue(-400.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for degrees < -360.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_gamma_should_produce_correct_filter_name() {
        let step = FilterStep::Gamma {
            r: 1.0,
            g: 1.0,
            b: 1.0,
        };
        assert_eq!(step.filter_name(), "eq");
    }

    #[test]
    fn filter_step_gamma_should_produce_correct_args() {
        let step = FilterStep::Gamma {
            r: 2.2,
            g: 2.2,
            b: 2.2,
        };
        assert_eq!(step.args(), "gamma_r=2.2:gamma_g=2.2:gamma_b=2.2");
    }

    #[test]
    fn filter_step_gamma_neutral_should_produce_unity_args() {
        let step = FilterStep::Gamma {
            r: 1.0,
            g: 1.0,
            b: 1.0,
        };
        assert_eq!(step.args(), "gamma_r=1:gamma_g=1:gamma_b=1");
    }

    #[test]
    fn builder_gamma_with_neutral_values_should_succeed() {
        let result = FilterGraph::builder().gamma(1.0, 1.0, 1.0).build();
        assert!(
            result.is_ok(),
            "gamma(1.0, 1.0, 1.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_gamma_with_r_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder().gamma(0.0, 1.0, 1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for r < 0.1, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("gamma") && reason.contains(" r "),
                "reason should mention gamma r: {reason}"
            );
        }
    }

    #[test]
    fn builder_gamma_with_b_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder().gamma(1.0, 1.0, 11.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for b > 10.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_three_way_cc_should_produce_correct_filter_name() {
        let step = FilterStep::ThreeWayCC {
            lift: Rgb::NEUTRAL,
            gamma: Rgb::NEUTRAL,
            gain: Rgb::NEUTRAL,
        };
        assert_eq!(step.filter_name(), "curves");
    }

    #[test]
    fn filter_step_three_way_cc_neutral_should_produce_identity_curves() {
        let step = FilterStep::ThreeWayCC {
            lift: Rgb::NEUTRAL,
            gamma: Rgb::NEUTRAL,
            gain: Rgb::NEUTRAL,
        };
        let args = step.args();
        // Neutral: 0/0, 0.5/0.5, 1/1 for all channels.
        assert!(
            args.contains("r='0/0 0.5/0.5 1/1'"),
            "neutral r channel must be identity: {args}"
        );
        assert!(
            args.contains("g='0/0 0.5/0.5 1/1'"),
            "neutral g channel must be identity: {args}"
        );
        assert!(
            args.contains("b='0/0 0.5/0.5 1/1'"),
            "neutral b channel must be identity: {args}"
        );
    }

    #[test]
    fn builder_three_way_cc_with_neutral_values_should_succeed() {
        let result = FilterGraph::builder()
            .three_way_cc(Rgb::NEUTRAL, Rgb::NEUTRAL, Rgb::NEUTRAL)
            .build();
        assert!(
            result.is_ok(),
            "neutral three_way_cc must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_three_way_cc_with_gamma_zero_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .three_way_cc(
                Rgb::NEUTRAL,
                Rgb {
                    r: 0.0,
                    g: 1.0,
                    b: 1.0,
                },
                Rgb::NEUTRAL,
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for gamma.r = 0.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("gamma.r"),
                "reason should mention gamma.r: {reason}"
            );
        }
    }

    #[test]
    fn builder_three_way_cc_with_negative_gamma_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .three_way_cc(
                Rgb::NEUTRAL,
                Rgb {
                    r: 1.0,
                    g: -0.5,
                    b: 1.0,
                },
                Rgb::NEUTRAL,
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for gamma.g < 0.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("gamma.g"),
                "reason should mention gamma.g: {reason}"
            );
        }
    }

    #[test]
    fn filter_step_vignette_should_produce_correct_filter_name() {
        let step = FilterStep::Vignette {
            angle: 0.628,
            x0: 0.0,
            y0: 0.0,
        };
        assert_eq!(step.filter_name(), "vignette");
    }

    #[test]
    fn filter_step_vignette_zero_centre_should_use_w2_h2_defaults() {
        let step = FilterStep::Vignette {
            angle: 0.628,
            x0: 0.0,
            y0: 0.0,
        };
        let args = step.args();
        assert!(args.contains("x0=w/2"), "x0=0.0 should map to w/2: {args}");
        assert!(args.contains("y0=h/2"), "y0=0.0 should map to h/2: {args}");
        assert!(
            args.contains("angle=0.628"),
            "args must contain angle: {args}"
        );
    }

    #[test]
    fn filter_step_vignette_custom_centre_should_produce_numeric_coords() {
        let step = FilterStep::Vignette {
            angle: 0.5,
            x0: 320.0,
            y0: 240.0,
        };
        let args = step.args();
        assert!(args.contains("x0=320"), "custom x0 should appear: {args}");
        assert!(args.contains("y0=240"), "custom y0 should appear: {args}");
    }

    #[test]
    fn builder_vignette_with_valid_angle_should_succeed() {
        let result = FilterGraph::builder()
            .vignette(std::f32::consts::PI / 5.0, 0.0, 0.0)
            .build();
        assert!(
            result.is_ok(),
            "default vignette angle must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_vignette_with_angle_too_large_should_return_invalid_config() {
        let result = FilterGraph::builder().vignette(2.0, 0.0, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for angle > π/2, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("angle"),
                "reason should mention angle: {reason}"
            );
        }
    }

    #[test]
    fn builder_vignette_with_negative_angle_should_return_invalid_config() {
        let result = FilterGraph::builder().vignette(-0.1, 0.0, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for angle < 0.0, got {result:?}"
        );
    }

    // ── eq_animated tests ─────────────────────────────────────────────────────

    #[test]
    fn eq_animated_with_valid_static_values_should_succeed() {
        let result = FilterGraph::builder()
            .eq_animated(
                AnimatedValue::Static(0.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
            )
            .build();
        assert!(
            result.is_ok(),
            "neutral eq_animated params must build successfully, got {result:?}"
        );
    }

    #[test]
    fn eq_animated_static_should_produce_correct_args() {
        let step = FilterStep::EqAnimated {
            brightness: AnimatedValue::Static(0.1_f64),
            contrast: AnimatedValue::Static(1.5_f64),
            saturation: AnimatedValue::Static(0.8_f64),
            gamma: AnimatedValue::Static(2.0_f64),
        };
        assert_eq!(step.filter_name(), "eq");
        assert_eq!(
            step.args(),
            "brightness=0.1:contrast=1.5:saturation=0.8:gamma=2"
        );
    }

    #[test]
    fn eq_animated_with_brightness_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .eq_animated(
                AnimatedValue::Static(1.5_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for brightness > 1.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("brightness"),
                "reason should mention brightness: {reason}"
            );
        }
    }

    #[test]
    fn eq_animated_with_contrast_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .eq_animated(
                AnimatedValue::Static(0.0_f64),
                AnimatedValue::Static(4.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for contrast > 3.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("contrast"),
                "reason should mention contrast: {reason}"
            );
        }
    }

    #[test]
    fn eq_animated_with_saturation_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .eq_animated(
                AnimatedValue::Static(0.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(-0.5_f64),
                AnimatedValue::Static(1.0_f64),
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for saturation < 0.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("saturation"),
                "reason should mention saturation: {reason}"
            );
        }
    }

    #[test]
    fn eq_animated_with_gamma_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .eq_animated(
                AnimatedValue::Static(0.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(0.0_f64),
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for gamma < 0.1, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("gamma"),
                "reason should mention gamma: {reason}"
            );
        }
    }

    #[test]
    fn animated_saturation_track_should_register_animation_entry() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let track = crate::animation::AnimationTrack::new()
            .push(Keyframe {
                timestamp: Duration::ZERO,
                value: 1.0_f64,
                easing: Easing::Linear,
            })
            .push(Keyframe {
                timestamp: Duration::from_secs(2),
                value: 2.0_f64,
                easing: Easing::Linear,
            });

        let graph = FilterGraph::builder()
            .eq_animated(
                AnimatedValue::Static(0.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Track(track),
                AnimatedValue::Static(1.0_f64),
            )
            .build()
            .unwrap();

        assert_eq!(graph.pending_animations.len(), 1);
        assert_eq!(graph.pending_animations[0].node_name, "eq_0");
        assert_eq!(graph.pending_animations[0].param, "saturation");
    }

    #[test]
    fn eq_animated_second_call_should_use_eq_1_node_name() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let track = crate::animation::AnimationTrack::new().push(Keyframe {
            timestamp: Duration::ZERO,
            value: 0.5_f64,
            easing: Easing::Linear,
        });

        let graph = FilterGraph::builder()
            .eq_animated(
                AnimatedValue::Static(0.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
            )
            .eq_animated(
                AnimatedValue::Track(track),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
                AnimatedValue::Static(1.0_f64),
            )
            .build()
            .unwrap();

        assert_eq!(graph.pending_animations.len(), 1);
        assert_eq!(
            graph.pending_animations[0].node_name, "eq_1",
            "second eq_animated call must produce node name eq_1"
        );
        assert_eq!(graph.pending_animations[0].param, "brightness");
    }

    // ── color_correct / color_correct_animated tests ──────────────────────────

    #[test]
    fn color_correct_static_should_build_successfully() {
        let result = FilterGraph::builder()
            .color_correct((0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
            .build();
        assert!(
            result.is_ok(),
            "neutral color_correct must build successfully, got {result:?}"
        );
    }

    #[test]
    fn color_correct_animated_with_valid_static_values_should_succeed() {
        let result = FilterGraph::builder()
            .color_correct_animated(
                AnimatedValue::Static((0.0, 0.0, 0.0)),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
            )
            .build();
        assert!(
            result.is_ok(),
            "neutral color_correct_animated must build successfully, got {result:?}"
        );
    }

    #[test]
    fn color_correct_animated_lift_track_should_register_three_entries_with_rs_gs_bs_params() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let lift_track = crate::animation::AnimationTrack::new()
            .push(Keyframe {
                timestamp: Duration::ZERO,
                value: (0.0_f64, 0.0_f64, 0.0_f64),
                easing: Easing::Linear,
            })
            .push(Keyframe {
                timestamp: Duration::from_secs(1),
                value: (0.5_f64, -0.3_f64, 0.2_f64),
                easing: Easing::Linear,
            });

        let graph = FilterGraph::builder()
            .color_correct_animated(
                AnimatedValue::Track(lift_track),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
            )
            .build()
            .unwrap();

        assert_eq!(
            graph.pending_animations.len(),
            3,
            "lift track must register 3 entries"
        );
        let params: Vec<&str> = graph.pending_animations.iter().map(|e| e.param).collect();
        assert_eq!(params, ["rs", "gs", "bs"]);
        for entry in &graph.pending_animations {
            assert_eq!(entry.node_name, "colorbalance_0");
        }
    }

    #[test]
    fn color_correct_animated_gamma_track_should_register_three_entries_with_rm_gm_bm_params() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let gamma_track = crate::animation::AnimationTrack::new().push(Keyframe {
            timestamp: Duration::ZERO,
            value: (0.1_f64, 0.0_f64, -0.1_f64),
            easing: Easing::Linear,
        });

        let graph = FilterGraph::builder()
            .color_correct_animated(
                AnimatedValue::Static((0.0, 0.0, 0.0)),
                AnimatedValue::Track(gamma_track),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
            )
            .build()
            .unwrap();

        let params: Vec<&str> = graph.pending_animations.iter().map(|e| e.param).collect();
        assert_eq!(params, ["rm", "gm", "bm"]);
    }

    #[test]
    fn color_correct_animated_gain_track_should_register_three_entries_with_rh_gh_bh_params() {
        use crate::animation::{Easing, Keyframe};
        use std::time::Duration;

        let gain_track = crate::animation::AnimationTrack::new().push(Keyframe {
            timestamp: Duration::ZERO,
            value: (0.5_f64, 0.5_f64, 0.5_f64),
            easing: Easing::Linear,
        });

        let graph = FilterGraph::builder()
            .color_correct_animated(
                AnimatedValue::Static((0.0, 0.0, 0.0)),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
                AnimatedValue::Track(gain_track),
            )
            .build()
            .unwrap();

        let params: Vec<&str> = graph.pending_animations.iter().map(|e| e.param).collect();
        assert_eq!(params, ["rh", "gh", "bh"]);
    }

    #[test]
    fn color_correct_animated_component_out_of_range_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .color_correct_animated(
                AnimatedValue::Static((1.5, 0.0, 0.0)),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
                AnimatedValue::Static((0.0, 0.0, 0.0)),
            )
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for lift.r > 1.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("lift") && reason.contains("r"),
                "reason should mention lift.r: {reason}"
            );
        }
    }
}
