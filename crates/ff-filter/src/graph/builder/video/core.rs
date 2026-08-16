//! Temporal video filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;

impl FilterGraphBuilder {
    /// Trim the stream to the half-open interval `[start, end)` in seconds.
    #[must_use]
    pub fn trim(mut self, start: f64, end: f64) -> Self {
        self.steps.push(FilterStep::Trim {
            start: Some(start),
            end: Some(end),
        });
        self
    }

    /// Change playback speed by `factor`.
    ///
    /// `factor > 1.0` = fast motion (e.g. `2.0` = double speed).
    /// `factor < 1.0` = slow motion (e.g. `0.5` = half speed).
    ///
    /// **Video**: uses `setpts=PTS/{factor}`.
    /// **Audio**: uses chained `atempo` filters (each in [0.5, 2.0]) so the
    /// full range 0.1–100.0 is covered without quality degradation.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `factor` is outside [0.1, 100.0].
    #[must_use]
    pub fn speed(mut self, factor: f64) -> Self {
        self.steps.push(FilterStep::Speed { factor });
        self
    }

    /// Reverse video playback using `FFmpeg`'s `reverse` filter.
    ///
    /// **Warning**: `reverse` buffers the entire clip in memory before producing
    /// any output. Only use this on short clips to avoid excessive memory usage.
    #[must_use]
    pub fn reverse(mut self) -> Self {
        self.steps.push(FilterStep::Reverse);
        self
    }

    /// Concatenate `n_segments` sequential video inputs using `FFmpeg`'s `concat` filter.
    ///
    /// Requires `n_segments` video input slots (push to slots 0 through
    /// `n_segments - 1` in order). [`build`](Self::build) returns
    /// [`FilterError::InvalidConfig`] if `n_segments < 2`.
    #[must_use]
    pub fn concat_video(mut self, n_segments: u32) -> Self {
        self.steps.push(FilterStep::ConcatVideo { n: n_segments });
        self
    }

    /// Freeze the frame at `pts_sec` for `duration_sec` seconds using `FFmpeg`'s `loop` filter.
    ///
    /// The frame nearest to `pts_sec` is held for `duration_sec` seconds before
    /// playback resumes. Frame numbers are approximated using a 25 fps assumption;
    /// accuracy depends on the source stream's actual frame rate.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `pts_sec` is negative or `duration_sec` is ≤ 0.0.
    #[must_use]
    pub fn freeze_frame(mut self, pts_sec: f64, duration_sec: f64) -> Self {
        self.steps.push(FilterStep::FreezeFrame {
            pts: pts_sec,
            duration: duration_sec,
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_step_trim_should_produce_correct_args() {
        let step = FilterStep::Trim {
            start: Some(10.0),
            end: Some(30.0),
        };
        assert_eq!(step.filter_name(), "trim");
        assert_eq!(step.args(), "start=10:end=30");
    }

    #[test]
    fn filter_step_trim_open_ended_should_omit_the_none_bound() {
        assert_eq!(
            FilterStep::Trim {
                start: Some(10.0),
                end: None,
            }
            .args(),
            "start=10"
        );
        assert_eq!(
            FilterStep::Trim {
                start: None,
                end: Some(30.0),
            }
            .args(),
            "end=30"
        );
        assert_eq!(
            FilterStep::Trim {
                start: None,
                end: None,
            }
            .args(),
            ""
        );
    }

    #[test]
    fn filter_step_reset_pts_should_produce_setpts_pts_startpts() {
        let step = FilterStep::ResetPts;
        assert_eq!(step.filter_name(), "setpts");
        assert_eq!(step.args(), "PTS-STARTPTS");
    }

    #[test]
    fn filter_step_offset_pts_should_produce_setpts_offset_expr() {
        let step = FilterStep::OffsetPts { seconds: 2.5 };
        assert_eq!(step.filter_name(), "setpts");
        assert_eq!(step.args(), "PTS+2.5/TB");
    }

    #[test]
    fn filter_step_speed_should_produce_correct_filter_name() {
        let step = FilterStep::Speed { factor: 2.0 };
        assert_eq!(step.filter_name(), "setpts");
    }

    #[test]
    fn filter_step_speed_should_produce_correct_args_for_double_speed() {
        let step = FilterStep::Speed { factor: 2.0 };
        assert_eq!(step.args(), "PTS/2");
    }

    #[test]
    fn filter_step_speed_should_produce_correct_args_for_half_speed() {
        let step = FilterStep::Speed { factor: 0.5 };
        assert_eq!(step.args(), "PTS/0.5");
    }

    #[test]
    fn builder_speed_with_factor_below_minimum_should_return_invalid_config() {
        let result = FilterGraph::builder().speed(0.09).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for factor below 0.1, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("speed factor"),
                "reason should mention speed factor: {reason}"
            );
        }
    }

    #[test]
    fn builder_speed_with_factor_above_maximum_should_return_invalid_config() {
        let result = FilterGraph::builder().speed(100.1).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for factor above 100.0, got {result:?}"
        );
    }

    #[test]
    fn builder_speed_with_zero_factor_should_return_invalid_config() {
        let result = FilterGraph::builder().speed(0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for factor 0.0, got {result:?}"
        );
    }

    #[test]
    fn builder_speed_at_boundary_values_should_succeed() {
        let low = FilterGraph::builder().speed(0.1).build();
        assert!(low.is_ok(), "speed(0.1) should succeed, got {low:?}");
        let high = FilterGraph::builder().speed(100.0).build();
        assert!(high.is_ok(), "speed(100.0) should succeed, got {high:?}");
    }

    #[test]
    fn filter_step_reverse_should_produce_correct_filter_name_and_empty_args() {
        let step = FilterStep::Reverse;
        assert_eq!(step.filter_name(), "reverse");
        assert_eq!(step.args(), "");
    }

    #[test]
    fn builder_reverse_should_succeed() {
        let result = FilterGraph::builder().reverse().build();
        assert!(
            result.is_ok(),
            "reverse must build successfully, got {result:?}"
        );
    }

    #[test]
    fn filter_step_concat_video_should_have_correct_filter_name() {
        let step = FilterStep::ConcatVideo { n: 2 };
        assert_eq!(step.filter_name(), "concat");
    }

    #[test]
    fn filter_step_concat_video_should_produce_correct_args_for_n2() {
        let step = FilterStep::ConcatVideo { n: 2 };
        assert_eq!(step.args(), "n=2:v=1:a=0");
    }

    #[test]
    fn filter_step_concat_video_should_produce_correct_args_for_n3() {
        let step = FilterStep::ConcatVideo { n: 3 };
        assert_eq!(step.args(), "n=3:v=1:a=0");
    }

    #[test]
    fn builder_concat_video_valid_should_build_successfully() {
        let result = FilterGraph::builder().concat_video(2).build();
        assert!(
            result.is_ok(),
            "concat_video(2) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_concat_video_with_n1_should_return_invalid_config() {
        let result = FilterGraph::builder().concat_video(1).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for n=1, got {result:?}"
        );
    }

    #[test]
    fn builder_concat_video_with_n0_should_return_invalid_config() {
        let result = FilterGraph::builder().concat_video(0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for n=0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_freeze_frame_should_produce_correct_filter_name() {
        let step = FilterStep::FreezeFrame {
            pts: 2.0,
            duration: 3.0,
        };
        assert_eq!(step.filter_name(), "loop");
    }

    #[test]
    fn filter_step_freeze_frame_should_produce_correct_args() {
        let step = FilterStep::FreezeFrame {
            pts: 2.0,
            duration: 3.0,
        };
        // 2.0s * 25fps = frame 50; 3.0s * 25fps = 75 loop iterations
        assert_eq!(step.args(), "loop=75:size=1:start=50");
    }

    #[test]
    fn filter_step_freeze_frame_at_zero_pts_should_produce_start_zero() {
        let step = FilterStep::FreezeFrame {
            pts: 0.0,
            duration: 1.0,
        };
        assert_eq!(step.args(), "loop=25:size=1:start=0");
    }

    #[test]
    fn builder_freeze_frame_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().freeze_frame(2.0, 3.0).build();
        assert!(
            result.is_ok(),
            "freeze_frame(2.0, 3.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_freeze_frame_with_negative_pts_should_return_invalid_config() {
        let result = FilterGraph::builder().freeze_frame(-1.0, 3.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for negative pts, got {result:?}"
        );
    }

    #[test]
    fn builder_freeze_frame_with_zero_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().freeze_frame(2.0, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for zero duration, got {result:?}"
        );
    }

    #[test]
    fn builder_freeze_frame_with_negative_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().freeze_frame(2.0, -1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for negative duration, got {result:?}"
        );
    }
}
