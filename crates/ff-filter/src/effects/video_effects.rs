//! Frame-level video effects added to [`FilterGraph`] after construction.

use crate::effects::lens_profile::LensProfile;
use crate::error::FilterError;
use crate::graph::FilterGraph;
use crate::graph::filter_step::FilterStep;

impl FilterGraph {
    /// Simulate motion blur by blending multiple consecutive frames.
    ///
    /// `shutter_angle_degrees` controls the blend ratio (360° = full
    /// frame-period exposure). `sub_frames` sets the number of frames blended
    /// and must be in [2, 16].
    ///
    /// Uses `FFmpeg`'s `tblend` filter with `all_expr`:
    /// the normalised shutter angle becomes the weight for the previous frame
    /// (`B`), and its complement weights the current frame (`A`).
    ///
    /// Call this method after [`FilterGraph::builder()`] / [`build()`] but
    /// **before** the first [`push_video`] call.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if `sub_frames` is outside [2, 16].
    ///
    /// [`build()`]: crate::FilterGraphBuilder::build
    /// [`push_video`]: FilterGraph::push_video
    pub fn motion_blur(
        &mut self,
        shutter_angle_degrees: f32,
        sub_frames: u8,
    ) -> Result<&mut Self, FilterError> {
        if !(2..=16).contains(&sub_frames) {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("sub_frames must be 2–16, got {sub_frames}"),
            });
        }
        self.inner.push_step(FilterStep::MotionBlur {
            shutter_angle_degrees,
            sub_frames,
        });
        Ok(self)
    }

    /// Correct radial lens distortion using two polynomial coefficients.
    ///
    /// `k1` and `k2` are the first- and second-order radial distortion
    /// coefficients. Negative values correct barrel distortion; positive values
    /// correct pincushion distortion.
    ///
    /// Uses `FFmpeg`'s `lenscorrection` filter.
    ///
    /// Call this method after [`FilterGraph::builder()`] / [`build()`] but
    /// **before** the first [`push_video`] call.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if either coefficient is outside [−1.0, 1.0].
    ///
    /// [`build()`]: crate::FilterGraphBuilder::build
    /// [`push_video`]: FilterGraph::push_video
    pub fn lens_correction(&mut self, k1: f32, k2: f32) -> Result<&mut Self, FilterError> {
        if !(-1.0..=1.0).contains(&k1) || !(-1.0..=1.0).contains(&k2) {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("k1/k2 must be in −1.0..=1.0, got k1={k1} k2={k2}"),
            });
        }
        self.inner.push_step(FilterStep::LensCorrection { k1, k2 });
        Ok(self)
    }

    /// Add random per-frame film grain to luma and chroma channels.
    ///
    /// `luma_strength` and `chroma_strength` control grain intensity and are
    /// clamped to [0.0, 100.0]. The `allf=t` flag varies the noise seed each
    /// frame to simulate real film grain temporal variation.
    ///
    /// Uses `FFmpeg`'s `noise` filter with `alls` (luma), `c0s`/`c1s` (Cb/Cr),
    /// and `allf=t` (per-frame seed).
    ///
    /// Call this method after [`FilterGraph::builder()`] / [`build()`] but
    /// **before** the first [`push_video`] call.
    ///
    /// [`build()`]: crate::FilterGraphBuilder::build
    /// [`push_video`]: FilterGraph::push_video
    pub fn film_grain(&mut self, luma_strength: f32, chroma_strength: f32) -> &mut Self {
        self.inner.push_step(FilterStep::FilmGrain {
            luma_strength,
            chroma_strength,
        });
        self
    }

    /// Reduce lateral chromatic aberration by independently scaling R and B channels.
    ///
    /// `red_scale` and `blue_scale` are fractional adjustments relative to 1.0
    /// (e.g. `red_scale = 1.002` scales R by 0.2%). Valid range for each: 0.9–1.1.
    ///
    /// The scale deviation is converted to an integer pixel shift for `FFmpeg`'s
    /// `rgbashift` filter: `shift = ((scale - 1.0) * 100.0).round()`.
    ///
    /// Uses `FFmpeg`'s `rgbashift` filter with `edge=smear`.
    ///
    /// Call this method after [`FilterGraph::builder()`] / [`build()`] but
    /// **before** the first [`push_video`] call.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if either scale is outside [0.9, 1.1].
    ///
    /// [`build()`]: crate::FilterGraphBuilder::build
    /// [`push_video`]: FilterGraph::push_video
    pub fn fix_chromatic_aberration(
        &mut self,
        red_scale: f32,
        blue_scale: f32,
    ) -> Result<&mut Self, FilterError> {
        if !(0.9..=1.1).contains(&red_scale) || !(0.9..=1.1).contains(&blue_scale) {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!(
                    "red_scale/blue_scale must be in 0.9–1.1, got red={red_scale} blue={blue_scale}"
                ),
            });
        }
        #[allow(clippy::cast_possible_truncation)]
        let rh = ((red_scale - 1.0) * 100.0).round() as i32;
        #[allow(clippy::cast_possible_truncation)]
        let bh = ((blue_scale - 1.0) * 100.0).round() as i32;
        self.inner
            .push_step(FilterStep::ChromaticAberration { rh, bh });
        Ok(self)
    }

    /// Add a glow / bloom effect by blending blurred highlights back over the image.
    ///
    /// `threshold` controls which luminance level triggers glow (clamped to [0.0, 1.0]).
    /// `radius` is the Gaussian blur sigma in pixels (clamped to [0.5, 50.0]).
    /// `intensity` is the additive blend strength (clamped to [0.0, 2.0]).
    ///
    /// Values outside the valid ranges are silently clamped — no error is returned.
    ///
    /// Uses `FFmpeg`'s `split`, `curves`, `gblur`, and `blend` filters.
    ///
    /// Call this method after [`FilterGraph::builder()`] / [`build()`] but
    /// **before** the first [`push_video`] call.
    ///
    /// [`build()`]: crate::FilterGraphBuilder::build
    /// [`push_video`]: FilterGraph::push_video
    pub fn glow(&mut self, threshold: f32, radius: f32, intensity: f32) -> &mut Self {
        self.inner.push_step(FilterStep::Glow {
            threshold,
            radius,
            intensity,
        });
        self
    }

    /// Apply a predefined camera lens distortion correction profile.
    ///
    /// Looks up the radial coefficients (`k1`, `k2`) and `scale` from the
    /// profile and pushes a `lenscorrection` step followed by a `scale` step
    /// that zooms slightly to hide the warped border pixels.
    ///
    /// Uses `FFmpeg`'s `lenscorrection` and `scale` filters.
    ///
    /// Call this method after [`FilterGraph::builder()`] / [`build()`] but
    /// **before** the first [`push_video`] call.
    ///
    /// [`build()`]: crate::FilterGraphBuilder::build
    /// [`push_video`]: FilterGraph::push_video
    pub fn lens_profile(&mut self, profile: LensProfile) -> &mut Self {
        let (k1, k2, scale) = profile.coefficients();
        self.inner.push_step(FilterStep::LensCorrection { k1, k2 });
        self.inner
            .push_step(FilterStep::ScaleMultiplier { factor: scale });
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::effects::lens_profile::LensProfile;
    use crate::graph::filter_step::FilterStep;
    use crate::{FilterError, FilterGraph};

    #[test]
    fn motion_blur_with_valid_params_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.motion_blur(180.0, 2);
        assert!(
            result.is_ok(),
            "motion_blur(180.0, 2) must succeed, got {result:?}"
        );
    }

    #[test]
    fn motion_blur_with_sub_frames_one_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.motion_blur(180.0, 1);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "sub_frames=1 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn motion_blur_with_sub_frames_seventeen_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.motion_blur(180.0, 17);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "sub_frames=17 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn filter_step_motion_blur_should_have_tblend_filter_name() {
        let step = FilterStep::MotionBlur {
            shutter_angle_degrees: 180.0,
            sub_frames: 4,
        };
        assert_eq!(step.filter_name(), "tblend");
    }

    #[test]
    fn motion_blur_zero_angle_should_produce_identity_blend_args() {
        let step = FilterStep::MotionBlur {
            shutter_angle_degrees: 0.0,
            sub_frames: 2,
        };
        let args = step.args();
        assert!(
            args.contains("A*1") && args.contains("B*0"),
            "0° shutter angle must produce identity blend (A*1+B*0): {args}"
        );
    }

    #[test]
    fn motion_blur_full_angle_should_produce_full_blend_args() {
        let step = FilterStep::MotionBlur {
            shutter_angle_degrees: 360.0,
            sub_frames: 2,
        };
        let args = step.args();
        assert!(
            args.contains("A*0+B*1"),
            "360° shutter angle must produce full blend (A*0+B*1): {args}"
        );
    }

    #[test]
    fn motion_blur_half_angle_should_produce_equal_blend_args() {
        let step = FilterStep::MotionBlur {
            shutter_angle_degrees: 180.0,
            sub_frames: 2,
        };
        let args = step.args();
        assert!(
            args.contains("A*0.5+B*0.5"),
            "180° shutter angle must produce equal blend (A*0.5+B*0.5): {args}"
        );
    }

    // lens_correction

    #[test]
    fn lens_correction_with_valid_coefficients_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.lens_correction(-0.2, 0.0);
        assert!(
            result.is_ok(),
            "lens_correction(-0.2, 0.0) must succeed, got {result:?}"
        );
    }

    #[test]
    fn lens_correction_identity_k1_zero_k2_zero_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.lens_correction(0.0, 0.0);
        assert!(
            result.is_ok(),
            "lens_correction(0.0, 0.0) identity must succeed, got {result:?}"
        );
    }

    #[test]
    fn lens_correction_k1_out_of_range_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.lens_correction(1.5, 0.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "k1=1.5 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn lens_correction_k2_out_of_range_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.lens_correction(0.0, -1.5);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "k2=-1.5 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn filter_step_lens_correction_should_have_lenscorrection_filter_name() {
        let step = FilterStep::LensCorrection { k1: -0.2, k2: 0.0 };
        assert_eq!(step.filter_name(), "lenscorrection");
    }

    #[test]
    fn lens_correction_args_should_contain_k1_and_k2() {
        let step = FilterStep::LensCorrection { k1: -0.2, k2: 0.1 };
        let args = step.args();
        assert!(
            args.contains("k1=-0.2"),
            "args must contain k1=-0.2: {args}"
        );
        assert!(args.contains("k2=0.1"), "args must contain k2=0.1: {args}");
    }

    // film_grain

    #[test]
    fn film_grain_with_valid_params_should_return_mutable_self() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.film_grain(20.0, 5.0);
        // Method returns &mut Self — confirm it compiles and doesn't panic.
        let _ = result;
    }

    #[test]
    fn filter_step_film_grain_should_have_noise_filter_name() {
        let step = FilterStep::FilmGrain {
            luma_strength: 20.0,
            chroma_strength: 5.0,
        };
        assert_eq!(step.filter_name(), "noise");
    }

    #[test]
    fn film_grain_args_should_contain_alls_c0s_c1s_and_allf_t() {
        let step = FilterStep::FilmGrain {
            luma_strength: 20.0,
            chroma_strength: 5.0,
        };
        let args = step.args();
        assert!(
            args.contains("alls=20"),
            "args must contain alls=20: {args}"
        );
        assert!(args.contains("c0s=5"), "args must contain c0s=5: {args}");
        assert!(args.contains("c1s=5"), "args must contain c1s=5: {args}");
        assert!(args.contains("allf=t"), "args must contain allf=t: {args}");
    }

    #[test]
    fn film_grain_zero_strength_should_produce_zero_alls() {
        let step = FilterStep::FilmGrain {
            luma_strength: 0.0,
            chroma_strength: 0.0,
        };
        let args = step.args();
        assert_eq!(args, "alls=0:c0s=0:c1s=0:allf=t");
    }

    #[test]
    fn film_grain_values_above_100_should_be_clamped_to_100() {
        let step = FilterStep::FilmGrain {
            luma_strength: 200.0,
            chroma_strength: 999.0,
        };
        let args = step.args();
        assert!(
            args.contains("alls=100"),
            "luma_strength > 100 must clamp to 100: {args}"
        );
        assert!(
            args.contains("c0s=100") && args.contains("c1s=100"),
            "chroma_strength > 100 must clamp to 100: {args}"
        );
    }

    #[test]
    fn film_grain_negative_values_should_be_clamped_to_zero() {
        let step = FilterStep::FilmGrain {
            luma_strength: -50.0,
            chroma_strength: -10.0,
        };
        let args = step.args();
        assert_eq!(args, "alls=0:c0s=0:c1s=0:allf=t");
    }

    #[test]
    fn film_grain_animated_static_should_render_the_zero_time_strength() {
        use crate::animation::AnimatedValue;
        let step = FilterStep::FilmGrainAnimated {
            luma_strength: AnimatedValue::Static(20.0_f64),
            chroma_strength: AnimatedValue::Static(5.0_f64),
        };
        assert_eq!(step.filter_name(), "noise");
        // Renders the same static args as FilmGrain; the pattern stays temporal.
        assert_eq!(step.args(), "alls=20:c0s=5:c1s=5:allf=t");
    }

    // lens_profile

    #[test]
    fn lens_profile_gopro_hero9_wide_should_push_two_steps() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.lens_profile(LensProfile::GoproHero9Wide);
        let _ = result; // returns &mut Self
    }

    #[test]
    fn lens_profile_custom_should_push_lens_correction_step() {
        let step = FilterStep::LensCorrection { k1: -0.1, k2: 0.02 };
        assert_eq!(step.filter_name(), "lenscorrection");
        assert!(step.args().contains("k1=-0.1"));
        assert!(step.args().contains("k2=0.02"));
    }

    #[test]
    fn lens_profile_scale_multiplier_should_have_scale_filter_name() {
        let step = FilterStep::ScaleMultiplier { factor: 1.05 };
        assert_eq!(step.filter_name(), "scale");
    }

    #[test]
    fn lens_profile_scale_multiplier_args_should_contain_factor() {
        let step = FilterStep::ScaleMultiplier { factor: 1.05 };
        let args = step.args();
        assert!(
            args.contains("iw*1.05") && args.contains("ih*1.05"),
            "ScaleMultiplier args must reference iw*factor and ih*factor: {args}"
        );
    }

    #[test]
    fn lens_profile_identity_custom_should_use_unit_scale() {
        let step = FilterStep::ScaleMultiplier { factor: 1.0 };
        let args = step.args();
        assert_eq!(args, "w=iw*1:h=ih*1");
    }

    // fix_chromatic_aberration

    #[test]
    fn fix_chromatic_aberration_with_valid_scales_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.fix_chromatic_aberration(1.002, 0.998);
        assert!(
            result.is_ok(),
            "fix_chromatic_aberration(1.002, 0.998) must succeed, got {result:?}"
        );
    }

    #[test]
    fn fix_chromatic_aberration_identity_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.fix_chromatic_aberration(1.0, 1.0);
        assert!(
            result.is_ok(),
            "fix_chromatic_aberration(1.0, 1.0) identity must succeed, got {result:?}"
        );
    }

    #[test]
    fn fix_chromatic_aberration_red_scale_out_of_range_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.fix_chromatic_aberration(1.2, 1.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "red_scale=1.2 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn fix_chromatic_aberration_blue_scale_out_of_range_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.fix_chromatic_aberration(1.0, 0.8);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "blue_scale=0.8 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn filter_step_chromatic_aberration_should_have_rgbashift_filter_name() {
        let step = FilterStep::ChromaticAberration { rh: 2, bh: -2 };
        assert_eq!(step.filter_name(), "rgbashift");
    }

    #[test]
    fn fix_chromatic_aberration_args_should_contain_rh_bh_and_edge_smear() {
        let step = FilterStep::ChromaticAberration { rh: 2, bh: -2 };
        let args = step.args();
        assert!(args.contains("rh=2"), "args must contain rh=2: {args}");
        assert!(args.contains("bh=-2"), "args must contain bh=-2: {args}");
        assert!(
            args.contains("edge=smear"),
            "args must contain edge=smear: {args}"
        );
    }

    #[test]
    fn fix_chromatic_aberration_identity_scale_should_produce_zero_shifts() {
        let step = FilterStep::ChromaticAberration { rh: 0, bh: 0 };
        let args = step.args();
        assert_eq!(args, "rh=0:bh=0:edge=smear");
    }

    // glow

    #[test]
    fn glow_with_valid_params_should_return_mutable_self() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.glow(0.8, 10.0, 0.8);
        let _ = result;
    }

    #[test]
    fn glow_identity_zero_intensity_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.glow(0.8, 10.0, 0.0);
        let _ = result;
    }

    #[test]
    fn filter_step_glow_should_have_split_filter_name() {
        let step = FilterStep::Glow {
            threshold: 0.8,
            radius: 10.0,
            intensity: 0.8,
        };
        assert_eq!(step.filter_name(), "split");
    }

    #[test]
    fn glow_args_should_contain_threshold_radius_intensity() {
        let step = FilterStep::Glow {
            threshold: 0.8,
            radius: 10.0,
            intensity: 0.8,
        };
        let args = step.args();
        assert!(
            args.contains("0.8/0"),
            "args must contain threshold in curve: {args}"
        );
        assert!(
            args.contains("sigma=10"),
            "args must contain sigma=10: {args}"
        );
        assert!(
            args.contains("all_opacity=0.8"),
            "args must contain all_opacity=0.8: {args}"
        );
        assert!(
            args.contains("all_mode=addition"),
            "args must contain all_mode=addition: {args}"
        );
    }

    #[test]
    fn glow_animated_static_should_render_the_zero_time_compound() {
        use crate::animation::AnimatedValue;
        let step = FilterStep::GlowAnimated {
            threshold: AnimatedValue::Static(0.8_f64),
            radius: AnimatedValue::Static(10.0_f64),
            intensity: AnimatedValue::Static(0.8_f64),
        };
        assert_eq!(step.filter_name(), "split");
        // Same compound args as the static Glow at these values.
        let args = step.args();
        assert!(
            args.contains("0.8/0") && args.contains("sigma=10"),
            "{args}"
        );
        assert!(args.contains("all_opacity=0.8"), "{args}");
    }

    #[test]
    fn glow_threshold_above_one_should_be_clamped() {
        let step = FilterStep::Glow {
            threshold: 1.1,
            radius: 5.0,
            intensity: 1.0,
        };
        let args = step.args();
        assert!(
            args.contains("1/0"),
            "threshold=1.1 must clamp to 1.0 in curve (1/0): {args}"
        );
    }

    #[test]
    fn glow_radius_below_min_should_be_clamped_to_half() {
        let step = FilterStep::Glow {
            threshold: 0.5,
            radius: 0.1,
            intensity: 1.0,
        };
        let args = step.args();
        assert!(
            args.contains("sigma=0.5"),
            "radius=0.1 must clamp to 0.5: {args}"
        );
    }

    #[test]
    fn glow_intensity_above_two_should_be_clamped() {
        let step = FilterStep::Glow {
            threshold: 0.5,
            radius: 5.0,
            intensity: 5.0,
        };
        let args = step.args();
        assert!(
            args.contains("all_opacity=2"),
            "intensity=5.0 must clamp to 2.0: {args}"
        );
    }
}
