//! What the linked `FFmpeg` build can actually do.
//!
//! An `FFmpeg` build carries only the filters it was configured with, so a feature
//! that is present in this crate's API can still be unavailable at run time. A host
//! that asks first can disable the tool rather than let a user build something the
//! render will refuse (#1809).

/// The `libavfilter` filter a text layer is drawn with.
///
/// Exposed so a caller can name the missing piece in its own message without
/// hard-coding `FFmpeg` vocabulary.
pub const TEXT_FILTER: &str = "drawtext";

/// Whether this build can render a text layer.
///
/// [`TEXT_FILTER`] needs freetype, which several common `FFmpeg` packages are built
/// without; where it is missing, every text layer fails when its graph is built.
///
/// # Examples
///
/// ```
/// if !ff_filter::text_rendering_available() {
///     println!("this FFmpeg build has no {}", ff_filter::TEXT_FILTER);
/// }
/// ```
#[must_use]
pub fn text_rendering_available() -> bool {
    crate::filter_inner::have_filter(c"drawtext")
}

#[cfg(test)]
mod tests {
    use super::{TEXT_FILTER, text_rendering_available};
    use crate::graph::FilterStep;

    #[test]
    fn text_filter_should_name_the_filter_the_text_layer_builds() {
        let step = FilterStep::DrawText {
            opts: crate::graph::DrawTextOptions {
                text: "x".to_string(),
                x: "0".to_string(),
                y: "0".to_string(),
                font_size: 12,
                font_color: "0xFFFFFF".to_string(),
                opacity: 1.0,
                font_file: None,
                box_color: None,
                box_border_width: 0,
            },
        };
        // Deterministic and build-independent: if either name is changed without the
        // other, the capability query starts answering about a different filter than
        // the one a text layer is actually built from.
        assert_eq!(TEXT_FILTER, step.filter_name());
    }

    #[test]
    fn text_rendering_available_should_agree_with_building_a_text_source() {
        use ff_format::TextSpec;

        let built = crate::graph::TextSource::new(&TextSpec::new("probe"), 64, 64, 30.0).is_ok();
        // The claim is checked against the thing it is a claim about, so the query
        // cannot drift from reality on either kind of build: where `drawtext` is
        // missing the source must fail to build, and where it is present it must not.
        assert_eq!(
            text_rendering_available(),
            built,
            "the capability query and the actual text graph disagree"
        );
    }
}
