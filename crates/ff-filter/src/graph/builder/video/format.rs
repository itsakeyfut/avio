//! Format video filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;
use ff_format::{ColorRange, ColorSpace, PixelFormat};

impl FilterGraphBuilder {
    /// Convert the video to a suitable pixel format from the given options.
    ///
    /// Each list is rendered with the `FFmpeg`-canonical token; values with no `FFmpeg`
    /// equivalent are skipped. Alpha format is not a `format`-filter option — set it on the
    /// compositing step instead (see [`FilterGraphBuilder::blend`]).
    #[must_use]
    pub fn format(
        mut self,
        pix_fmts: Vec<PixelFormat>,
        color_spaces: Vec<ColorSpace>,
        color_ranges: Vec<ColorRange>,
    ) -> Self {
        // This check is not required, but there's no point in adding the step if all requirements are empty.
        if !pix_fmts.is_empty() || !color_spaces.is_empty() || !color_ranges.is_empty() {
            self.steps.push(FilterStep::Format {
                pix_fmts,
                color_spaces,
                color_ranges,
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
            color_ranges: vec![ColorRange::Limited],
        };
        assert_eq!(step.filter_name(), "format");
        assert_eq!(
            step.args(),
            "pix_fmts=rgba|yuv420p:color_spaces=gbr:color_ranges=tv"
        );
    }

    #[test]
    fn format_args_should_skip_values_without_ffmpeg_token() {
        // `Other` (no static pix_fmt token), `Bt601` (needs the #1217 split) and `Unknown`
        // range all return `None` and must be skipped rather than emitting invalid tokens.
        let step = FilterStep::Format {
            pix_fmts: vec![PixelFormat::Other(123), PixelFormat::Yuv420p],
            color_spaces: vec![ColorSpace::Bt601, ColorSpace::Bt2020],
            color_ranges: vec![ColorRange::Unknown],
        };
        assert_eq!(step.args(), "pix_fmts=yuv420p:color_spaces=bt2020nc");
    }
}
