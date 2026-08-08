//! Text overlay filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;

impl FilterGraphBuilder {
    /// Overlay text onto the video using the `drawtext` filter.
    ///
    /// See [`DrawTextOptions`] for all configurable fields including position,
    /// font, size, color, opacity, and optional background box.
    #[must_use]
    pub fn drawtext(mut self, opts: DrawTextOptions) -> Self {
        self.steps.push(FilterStep::DrawText { opts });
        self
    }

    /// Scroll text from right to left as a news ticker.
    ///
    /// Uses `FFmpeg`'s `drawtext` filter with the expression `x = w - t * speed`
    /// so the text enters from the right edge at playback start and advances
    /// left by `speed_px_per_sec` pixels per second.
    ///
    /// `y` is an `FFmpeg` expression string for the vertical position,
    /// e.g. `"h-50"` for 50 pixels above the bottom.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if:
    /// - `text` is empty, or
    /// - `speed_px_per_sec` is ≤ 0.0.
    #[must_use]
    pub fn ticker(
        mut self,
        text: &str,
        y: &str,
        speed_px_per_sec: f32,
        font_size: u32,
        font_color: &str,
    ) -> Self {
        self.steps.push(FilterStep::Ticker {
            text: text.to_owned(),
            y: y.to_owned(),
            speed_px_per_sec,
            font_size,
            font_color: font_color.to_owned(),
        });
        self
    }

    /// Burn SRT subtitles into the video (hard subtitles).
    ///
    /// Subtitles are read from the `.srt` file at `srt_path` and rendered
    /// at the timecodes defined in the file using `FFmpeg`'s `subtitles` filter.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if:
    /// - the extension is not `.srt`, or
    /// - the file does not exist at build time.
    #[must_use]
    pub fn subtitles_srt(mut self, srt_path: &str) -> Self {
        self.steps.push(FilterStep::SubtitlesSrt {
            path: srt_path.to_owned(),
            force_style: None,
        });
        self
    }

    /// Burn SRT subtitles with an ASS style override (`force_style`).
    ///
    /// Identical to [`subtitles_srt`](Self::subtitles_srt) but passes
    /// `force_style` to the `subtitles` filter, e.g.
    /// `"Fontsize=24,PrimaryColour=&H00FFFFFF&,Alignment=2"`.
    #[must_use]
    pub fn subtitles_srt_styled(mut self, srt_path: &str, force_style: &str) -> Self {
        self.steps.push(FilterStep::SubtitlesSrt {
            path: srt_path.to_owned(),
            force_style: Some(force_style.to_owned()),
        });
        self
    }

    /// Burn ASS/SSA styled subtitles into the video (hard subtitles).
    ///
    /// Subtitles are read from the `.ass` or `.ssa` file at `ass_path` and
    /// rendered with full styling using `FFmpeg`'s dedicated `ass` filter,
    /// which preserves fonts, colours, and positioning better than the generic
    /// `subtitles` filter.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if:
    /// - the extension is not `.ass` or `.ssa`, or
    /// - the file does not exist at build time.
    #[must_use]
    pub fn subtitles_ass(mut self, ass_path: &str) -> Self {
        self.steps.push(FilterStep::SubtitlesAss {
            path: ass_path.to_owned(),
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_drawtext_opts() -> DrawTextOptions {
        DrawTextOptions {
            text: "Hello".to_string(),
            x: "10".to_string(),
            y: "10".to_string(),
            font_size: 24,
            font_color: "white".to_string(),
            font_file: None,
            opacity: 1.0,
            box_color: None,
            box_border_width: 0,
        }
    }

    #[test]
    fn filter_step_drawtext_should_produce_correct_filter_name() {
        let step = FilterStep::DrawText {
            opts: make_drawtext_opts(),
        };
        assert_eq!(step.filter_name(), "drawtext");
    }

    #[test]
    fn filter_step_drawtext_should_produce_correct_args_without_box() {
        let step = FilterStep::DrawText {
            opts: make_drawtext_opts(),
        };
        let args = step.args();
        assert!(
            args.contains("text='Hello'"),
            "args must contain text: {args}"
        );
        assert!(args.contains("x=10"), "args must contain x: {args}");
        assert!(args.contains("y=10"), "args must contain y: {args}");
        assert!(
            args.contains("fontsize=24"),
            "args must contain fontsize: {args}"
        );
        assert!(
            args.contains("fontcolor=white@1.00"),
            "args must contain fontcolor with opacity: {args}"
        );
        assert!(
            !args.contains("box=1"),
            "args must not contain box when box_color is None: {args}"
        );
    }

    #[test]
    fn filter_step_drawtext_with_box_should_include_box_args() {
        let opts = DrawTextOptions {
            box_color: Some("black@0.5".to_string()),
            box_border_width: 5,
            ..make_drawtext_opts()
        };
        let step = FilterStep::DrawText { opts };
        let args = step.args();
        assert!(args.contains("box=1"), "args must contain box=1: {args}");
        assert!(
            args.contains("boxcolor=black@0.5"),
            "args must contain boxcolor: {args}"
        );
        assert!(
            args.contains("boxborderw=5"),
            "args must contain boxborderw: {args}"
        );
    }

    #[test]
    fn filter_step_drawtext_with_font_file_should_include_fontfile_arg() {
        let opts = DrawTextOptions {
            font_file: Some("/usr/share/fonts/arial.ttf".to_string()),
            ..make_drawtext_opts()
        };
        let step = FilterStep::DrawText { opts };
        let args = step.args();
        assert!(
            args.contains("fontfile=/usr/share/fonts/arial.ttf"),
            "args must contain fontfile: {args}"
        );
    }

    #[test]
    fn filter_step_drawtext_should_escape_colon_in_text() {
        let opts = DrawTextOptions {
            text: "Time: 12:00".to_string(),
            ..make_drawtext_opts()
        };
        let step = FilterStep::DrawText { opts };
        let args = step.args();
        assert!(
            args.contains("Time\\: 12\\:00"),
            "colons in text must be escaped: {args}"
        );
    }

    #[test]
    fn filter_step_drawtext_should_escape_backslash_in_text() {
        let opts = DrawTextOptions {
            text: "path\\file".to_string(),
            ..make_drawtext_opts()
        };
        let step = FilterStep::DrawText { opts };
        let args = step.args();
        assert!(
            args.contains("path\\\\file"),
            "backslash in text must be escaped: {args}"
        );
    }

    #[test]
    fn builder_drawtext_with_valid_opts_should_succeed() {
        let result = FilterGraph::builder()
            .drawtext(make_drawtext_opts())
            .build();
        assert!(
            result.is_ok(),
            "drawtext with valid opts must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_drawtext_with_empty_text_should_return_invalid_config() {
        let opts = DrawTextOptions {
            text: String::new(),
            ..make_drawtext_opts()
        };
        let result = FilterGraph::builder().drawtext(opts).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for empty text, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("text"),
                "reason should mention text: {reason}"
            );
        }
    }

    #[test]
    fn builder_drawtext_with_opacity_too_high_should_return_invalid_config() {
        let opts = DrawTextOptions {
            opacity: 1.5,
            ..make_drawtext_opts()
        };
        let result = FilterGraph::builder().drawtext(opts).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for opacity > 1.0, got {result:?}"
        );
    }

    #[test]
    fn builder_drawtext_with_negative_opacity_should_return_invalid_config() {
        let opts = DrawTextOptions {
            opacity: -0.1,
            ..make_drawtext_opts()
        };
        let result = FilterGraph::builder().drawtext(opts).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for opacity < 0.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_subtitles_srt_should_produce_correct_filter_name() {
        let step = FilterStep::SubtitlesSrt {
            path: "subs.srt".to_owned(),
            force_style: None,
        };
        assert_eq!(step.filter_name(), "subtitles");
    }

    #[test]
    fn filter_step_subtitles_srt_should_produce_correct_args() {
        let step = FilterStep::SubtitlesSrt {
            path: "subs.srt".to_owned(),
            force_style: None,
        };
        assert_eq!(step.args(), "filename=subs.srt");
    }

    #[test]
    fn filter_step_subtitles_srt_force_style_appended_to_args() {
        let step = FilterStep::SubtitlesSrt {
            path: "subs.srt".to_owned(),
            force_style: Some("Fontsize=24,PrimaryColour=&H00FFFFFF&,Alignment=2".to_owned()),
        };
        assert_eq!(
            step.args(),
            "filename=subs.srt:force_style='Fontsize=24,PrimaryColour=&H00FFFFFF&,Alignment=2'"
        );
    }

    #[test]
    fn filter_step_subtitles_args_escape_windows_path() {
        // A Windows absolute path must be escaped for the filter's `:`-separated
        // option parser: backslashes → '/', drive colon → '\:'.
        let step = FilterStep::SubtitlesSrt {
            path: r"C:\subs\ep1.srt".to_owned(),
            force_style: None,
        };
        assert_eq!(step.args(), r"filename=C\:/subs/ep1.srt");
        let step = FilterStep::SubtitlesAss {
            path: r"D:\subs\ep1.ass".to_owned(),
        };
        assert_eq!(step.args(), r"filename=D\:/subs/ep1.ass");
    }

    #[test]
    fn builder_subtitles_srt_with_wrong_extension_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .subtitles_srt("subtitles.vtt")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for wrong extension, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("unsupported subtitle format"),
                "reason should mention unsupported format: {reason}"
            );
        }
    }

    #[test]
    fn builder_subtitles_srt_with_no_extension_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .subtitles_srt("subtitles_no_ext")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for missing extension, got {result:?}"
        );
    }

    #[test]
    fn builder_subtitles_srt_with_nonexistent_file_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .subtitles_srt("/nonexistent/path/subs_ab12cd.srt")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for nonexistent file, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("subtitle file not found"),
                "reason should mention file not found: {reason}"
            );
        }
    }

    #[test]
    fn filter_step_subtitles_ass_should_produce_correct_filter_name() {
        let step = FilterStep::SubtitlesAss {
            path: "subs.ass".to_owned(),
        };
        assert_eq!(step.filter_name(), "ass");
    }

    #[test]
    fn filter_step_subtitles_ass_should_produce_correct_args() {
        let step = FilterStep::SubtitlesAss {
            path: "subs.ass".to_owned(),
        };
        assert_eq!(step.args(), "filename=subs.ass");
    }

    #[test]
    fn filter_step_subtitles_ssa_should_produce_correct_filter_name() {
        let step = FilterStep::SubtitlesAss {
            path: "subs.ssa".to_owned(),
        };
        assert_eq!(step.filter_name(), "ass");
    }

    #[test]
    fn builder_subtitles_ass_with_wrong_extension_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .subtitles_ass("subtitles.srt")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for wrong extension, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("unsupported subtitle format"),
                "reason should mention unsupported format: {reason}"
            );
        }
    }

    #[test]
    fn builder_subtitles_ass_with_no_extension_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .subtitles_ass("subtitles_no_ext")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for missing extension, got {result:?}"
        );
    }

    #[test]
    fn builder_subtitles_ass_with_nonexistent_file_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .subtitles_ass("/nonexistent/path/subs_ab12cd.ass")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for nonexistent .ass file, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("subtitle file not found"),
                "reason should mention file not found: {reason}"
            );
        }
    }

    #[test]
    fn builder_subtitles_ssa_with_nonexistent_file_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .subtitles_ass("/nonexistent/path/subs_ab12cd.ssa")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for nonexistent .ssa file, got {result:?}"
        );
    }

    #[test]
    fn filter_step_ticker_should_produce_correct_filter_name() {
        let step = FilterStep::Ticker {
            text: "Breaking news".to_owned(),
            y: "h-50".to_owned(),
            speed_px_per_sec: 100.0,
            font_size: 24,
            font_color: "white".to_owned(),
        };
        assert_eq!(step.filter_name(), "drawtext");
    }

    #[test]
    fn filter_step_ticker_should_produce_correct_args() {
        let step = FilterStep::Ticker {
            text: "Breaking news".to_owned(),
            y: "h-50".to_owned(),
            speed_px_per_sec: 100.0,
            font_size: 24,
            font_color: "white".to_owned(),
        };
        let args = step.args();
        assert!(
            args.contains("text='Breaking news'"),
            "args should contain escaped text: {args}"
        );
        assert!(
            args.contains("x=w-t*100"),
            "args should contain scrolling x expression: {args}"
        );
        assert!(args.contains("y=h-50"), "args should contain y: {args}");
        assert!(
            args.contains("fontsize=24"),
            "args should contain fontsize: {args}"
        );
        assert!(
            args.contains("fontcolor=white"),
            "args should contain fontcolor: {args}"
        );
    }

    #[test]
    fn filter_step_ticker_should_escape_special_characters_in_text() {
        let step = FilterStep::Ticker {
            text: "colon:backslash\\apostrophe'".to_owned(),
            y: "10".to_owned(),
            speed_px_per_sec: 50.0,
            font_size: 20,
            font_color: "red".to_owned(),
        };
        let args = step.args();
        assert!(
            args.contains("\\:"),
            "colon should be escaped in args: {args}"
        );
        assert!(
            args.contains("\\'"),
            "apostrophe should be escaped in args: {args}"
        );
        assert!(
            args.contains("\\\\"),
            "backslash should be escaped in args: {args}"
        );
    }

    #[test]
    fn builder_ticker_with_empty_text_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .ticker("", "h-50", 100.0, 24, "white")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for empty text, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("ticker text must not be empty"),
                "reason should mention empty text: {reason}"
            );
        }
    }

    #[test]
    fn builder_ticker_with_zero_speed_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .ticker("Breaking news", "h-50", 0.0, 24, "white")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for speed = 0.0, got {result:?}"
        );
        if let Err(FilterError::InvalidConfig { reason }) = result {
            assert!(
                reason.contains("speed_px_per_sec"),
                "reason should mention speed_px_per_sec: {reason}"
            );
        }
    }

    #[test]
    fn builder_ticker_with_negative_speed_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .ticker("Breaking news", "h-50", -50.0, 24, "white")
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for negative speed, got {result:?}"
        );
    }
}
