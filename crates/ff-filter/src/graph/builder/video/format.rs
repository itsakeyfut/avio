//! Format video filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;
use ff_format::{AlphaMode, ColorRange, ColorSpace, PixelFormat};

impl FilterGraphBuilder {
    /// Convert the video to a suitable pixel format from the given options.
    #[must_use]
    pub fn format(
        mut self,
        pix_fmts: Vec<PixelFormat>,
        color_spaces: Vec<ColorSpace>,
        color_ranges: Vec<ColorRange>,
        alpha_modes: Vec<AlphaMode>,
    ) -> Self {
        // This check is not required, but there's no point in adding the step if all requirements are empty.
        if !pix_fmts.is_empty()
            || !color_spaces.is_empty()
            || !color_ranges.is_empty()
            || !alpha_modes.is_empty()
        {
            self.steps.push(FilterStep::Format {
                pix_fmts,
                color_spaces,
                color_ranges,
                alpha_modes,
            });
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_step_format_should_produce_correct_args() {
        let step = FilterStep::Format {
            pix_fmts: vec![PixelFormat::Rgba, PixelFormat::Yuv420p],
            color_spaces: vec![ColorSpace::Srgb],
            color_ranges: vec![],
            alpha_modes: vec![AlphaMode::Straight],
        };
        assert_eq!(step.filter_name(), "format");
        assert_eq!(
            step.args(),
            "pix_fmts=rgba|yuv420p:color_spaces=srgb:alpha_modes=straight"
        );
    }
}
