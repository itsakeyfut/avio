//! Video transition filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;

impl FilterGraphBuilder {
    /// Fade in from black, starting at `start_sec` seconds and reaching full
    /// brightness after `duration_sec` seconds.
    #[must_use]
    pub fn fade_in(mut self, start_sec: f64, duration_sec: f64) -> Self {
        self.steps.push(FilterStep::FadeIn {
            start: start_sec,
            duration: duration_sec,
        });
        self
    }

    /// Fade out to black, starting at `start_sec` seconds and reaching full
    /// black after `duration_sec` seconds.
    #[must_use]
    pub fn fade_out(mut self, start_sec: f64, duration_sec: f64) -> Self {
        self.steps.push(FilterStep::FadeOut {
            start: start_sec,
            duration: duration_sec,
        });
        self
    }

    /// Fade in from white, starting at `start_sec` seconds and reaching full
    /// brightness after `duration_sec` seconds.
    #[must_use]
    pub fn fade_in_white(mut self, start_sec: f64, duration_sec: f64) -> Self {
        self.steps.push(FilterStep::FadeInWhite {
            start: start_sec,
            duration: duration_sec,
        });
        self
    }

    /// Fade out to white, starting at `start_sec` seconds and reaching full
    /// white after `duration_sec` seconds.
    #[must_use]
    pub fn fade_out_white(mut self, start_sec: f64, duration_sec: f64) -> Self {
        self.steps.push(FilterStep::FadeOutWhite {
            start: start_sec,
            duration: duration_sec,
        });
        self
    }

    /// Apply a cross-dissolve transition between two video streams using `xfade`.
    ///
    /// Requires two input slots: slot 0 is clip A (first clip), slot 1 is clip B
    /// (second clip). Call [`FilterGraph::push_video`] with slot 0 for clip A
    /// frames and slot 1 for clip B frames.
    ///
    /// - `transition`: the visual transition style.
    /// - `duration`: length of the overlap in seconds. Must be > 0.0.
    /// - `offset`: PTS offset (seconds) at which clip B starts playing.
    #[must_use]
    pub fn xfade(mut self, transition: XfadeTransition, duration: f64, offset: f64) -> Self {
        self.steps.push(FilterStep::XFade {
            transition,
            duration,
            offset,
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_step_fade_in_should_produce_correct_filter_name() {
        let step = FilterStep::FadeIn {
            start: 0.0,
            duration: 1.5,
        };
        assert_eq!(step.filter_name(), "fade");
    }

    #[test]
    fn filter_step_fade_in_should_produce_correct_args() {
        let step = FilterStep::FadeIn {
            start: 0.0,
            duration: 1.5,
        };
        assert_eq!(step.args(), "type=in:start_time=0:duration=1.5");
    }

    #[test]
    fn filter_step_fade_in_with_nonzero_start_should_produce_correct_args() {
        let step = FilterStep::FadeIn {
            start: 2.0,
            duration: 1.0,
        };
        assert_eq!(step.args(), "type=in:start_time=2:duration=1");
    }

    #[test]
    fn filter_step_fade_out_should_produce_correct_filter_name() {
        let step = FilterStep::FadeOut {
            start: 8.5,
            duration: 1.5,
        };
        assert_eq!(step.filter_name(), "fade");
    }

    #[test]
    fn filter_step_fade_out_should_produce_correct_args() {
        let step = FilterStep::FadeOut {
            start: 8.5,
            duration: 1.5,
        };
        assert_eq!(step.args(), "type=out:start_time=8.5:duration=1.5");
    }

    #[test]
    fn builder_fade_in_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().fade_in(0.0, 1.5).build();
        assert!(
            result.is_ok(),
            "fade_in(0.0, 1.5) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_fade_out_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().fade_out(8.5, 1.5).build();
        assert!(
            result.is_ok(),
            "fade_out(8.5, 1.5) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_fade_in_with_zero_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().fade_in(0.0, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for zero duration, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("duration"),
                "reason should mention duration: {reason}"
            );
        }
    }

    #[test]
    fn builder_fade_out_with_negative_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().fade_out(0.0, -1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for negative duration, got {result:?}"
        );
    }

    #[test]
    fn filter_step_fade_in_white_should_produce_correct_filter_name() {
        let step = FilterStep::FadeInWhite {
            start: 0.0,
            duration: 1.0,
        };
        assert_eq!(step.filter_name(), "fade");
    }

    #[test]
    fn filter_step_fade_in_white_should_produce_correct_args() {
        let step = FilterStep::FadeInWhite {
            start: 0.0,
            duration: 1.0,
        };
        assert_eq!(step.args(), "type=in:start_time=0:duration=1:color=white");
    }

    #[test]
    fn filter_step_fade_in_white_with_nonzero_start_should_produce_correct_args() {
        let step = FilterStep::FadeInWhite {
            start: 2.5,
            duration: 1.0,
        };
        assert_eq!(step.args(), "type=in:start_time=2.5:duration=1:color=white");
    }

    #[test]
    fn filter_step_fade_out_white_should_produce_correct_filter_name() {
        let step = FilterStep::FadeOutWhite {
            start: 8.0,
            duration: 1.0,
        };
        assert_eq!(step.filter_name(), "fade");
    }

    #[test]
    fn filter_step_fade_out_white_should_produce_correct_args() {
        let step = FilterStep::FadeOutWhite {
            start: 8.0,
            duration: 1.0,
        };
        assert_eq!(step.args(), "type=out:start_time=8:duration=1:color=white");
    }

    #[test]
    fn builder_fade_in_white_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().fade_in_white(0.0, 1.0).build();
        assert!(
            result.is_ok(),
            "fade_in_white(0.0, 1.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_fade_out_white_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().fade_out_white(8.0, 1.0).build();
        assert!(
            result.is_ok(),
            "fade_out_white(8.0, 1.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_fade_in_white_with_zero_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().fade_in_white(0.0, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for zero duration, got {result:?}"
        );
    }

    #[test]
    fn builder_fade_out_white_with_negative_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().fade_out_white(0.0, -1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for negative duration, got {result:?}"
        );
    }

    #[test]
    fn xfade_transition_dissolve_should_produce_correct_str() {
        assert_eq!(XfadeTransition::Dissolve.as_str(), "dissolve");
    }

    #[test]
    fn xfade_transition_all_variants_should_produce_unique_strings() {
        let variants = [
            (XfadeTransition::Dissolve, "dissolve"),
            (XfadeTransition::Fade, "fade"),
            (XfadeTransition::WipeLeft, "wipeleft"),
            (XfadeTransition::WipeRight, "wiperight"),
            (XfadeTransition::WipeUp, "wipeup"),
            (XfadeTransition::WipeDown, "wipedown"),
            (XfadeTransition::SlideLeft, "slideleft"),
            (XfadeTransition::SlideRight, "slideright"),
            (XfadeTransition::SlideUp, "slideup"),
            (XfadeTransition::SlideDown, "slidedown"),
            (XfadeTransition::CircleOpen, "circleopen"),
            (XfadeTransition::CircleClose, "circleclose"),
            (XfadeTransition::FadeGrays, "fadegrays"),
            (XfadeTransition::Pixelize, "pixelize"),
        ];
        for (variant, expected) in variants {
            assert_eq!(
                variant.as_str(),
                expected,
                "XfadeTransition::{variant:?} should produce \"{expected}\""
            );
        }
    }

    #[test]
    fn filter_step_xfade_should_produce_correct_filter_name() {
        let step = FilterStep::XFade {
            transition: XfadeTransition::Dissolve,
            duration: 1.0,
            offset: 4.0,
        };
        assert_eq!(step.filter_name(), "xfade");
    }

    #[test]
    fn filter_step_xfade_should_produce_correct_args() {
        let step = FilterStep::XFade {
            transition: XfadeTransition::Dissolve,
            duration: 1.0,
            offset: 4.0,
        };
        assert_eq!(step.args(), "transition=dissolve:duration=1:offset=4");
    }

    #[test]
    fn filter_step_xfade_wipe_right_should_produce_correct_args() {
        let step = FilterStep::XFade {
            transition: XfadeTransition::WipeRight,
            duration: 0.5,
            offset: 9.5,
        };
        assert_eq!(step.args(), "transition=wiperight:duration=0.5:offset=9.5");
    }

    #[test]
    fn builder_xfade_with_valid_params_should_succeed() {
        let result = FilterGraph::builder()
            .xfade(XfadeTransition::Dissolve, 1.0, 4.0)
            .build();
        assert!(
            result.is_ok(),
            "xfade(Dissolve, 1.0, 4.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_xfade_with_zero_duration_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .xfade(XfadeTransition::Dissolve, 0.0, 4.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for zero duration, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("duration"),
                "reason should mention duration: {reason}"
            );
        }
    }

    #[test]
    fn builder_xfade_with_negative_duration_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .xfade(XfadeTransition::Fade, -1.0, 0.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for negative duration, got {result:?}"
        );
    }
}
