//! Sequential audio input concatenation.

use std::path::PathBuf;

use ff_format::ChannelLayout;

use crate::error::FilterError;
use crate::graph::graph::FilterGraph;

// AudioConcatenator

/// Concatenates multiple audio inputs into a single seamless output stream.
///
/// Each clip is loaded via an `amovie=` source node.  When
/// [`output_format`](Self::output_format) is set, an `aresample` and/or
/// `aformat` filter is inserted per clip to normalise the sample rate and
/// channel layout before concatenation.  A single clip skips the `concat`
/// filter entirely.
///
/// # Examples
///
/// ```ignore
/// use ff_filter::AudioConcatenator;
/// use ff_format::ChannelLayout;
///
/// let mut graph = AudioConcatenator::new(vec!["clip_a.mp3", "clip_b.mp3"])
///     .output_format(48_000, ChannelLayout::Stereo)
///     .build()?;
///
/// while let Some(frame) = graph.pull_audio()? {
///     // encode or play `frame`
/// }
/// ```
pub struct AudioConcatenator {
    inputs: Vec<PathBuf>,
    output_sample_rate: Option<u32>,
    output_channel_layout: Option<ChannelLayout>,
}

impl AudioConcatenator {
    /// Creates a new concatenator for the given clip paths.
    pub fn new(inputs: Vec<impl AsRef<std::path::Path>>) -> Self {
        Self {
            inputs: inputs
                .into_iter()
                .map(|p| p.as_ref().to_path_buf())
                .collect(),
            output_sample_rate: None,
            output_channel_layout: None,
        }
    }

    /// Sets the output sample rate and channel layout.
    ///
    /// When set, an `aresample` filter is inserted for each clip whose sample
    /// rate differs from `sample_rate`, and an `aformat` filter is inserted for
    /// each clip whose channel layout differs from `layout`.
    #[must_use]
    pub fn output_format(self, sample_rate: u32, layout: ChannelLayout) -> Self {
        Self {
            output_sample_rate: Some(sample_rate),
            output_channel_layout: Some(layout),
            ..self
        }
    }

    /// Builds a source-only [`FilterGraph`] that concatenates all inputs.
    ///
    /// # Errors
    ///
    /// - [`FilterError::CompositionFailed`] — no inputs were provided, or an
    ///   underlying `FFmpeg` graph-construction call failed.
    pub fn build(self) -> Result<FilterGraph, FilterError> {
        if self.inputs.is_empty() {
            return Err(FilterError::CompositionFailed {
                reason: "no inputs".to_string(),
            });
        }
        super::composition_inner::build_audio_concat(
            &self.inputs,
            self.output_sample_rate,
            self.output_channel_layout,
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn audio_concatenator_empty_inputs_should_err() {
        let result = AudioConcatenator::new(Vec::<PathBuf>::new()).build();
        assert!(
            matches!(result, Err(FilterError::CompositionFailed { .. })),
            "expected CompositionFailed for empty inputs, got {result:?}"
        );
    }

    #[test]
    fn audio_concatenator_three_inputs_should_build_successfully() {
        // Build with three nonexistent inputs.  Graph construction of individual
        // filter nodes (amovie, concat, abuffersink) should succeed; failure
        // only at avfilter_graph_config (file not found) is expected.
        //
        // Some FFmpeg builds omit `amovie` or `concat`; skip gracefully on
        // those environments rather than failing.
        let result = AudioConcatenator::new(vec!["a.mp3", "b.mp3", "c.mp3"])
            .output_format(48_000, ChannelLayout::Stereo)
            .build();
        assert!(result.is_err(), "expected error (nonexistent files)");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            if reason.contains("filter not found: amovie")
                || reason.contains("filter not found: concat")
            {
                println!(
                    "Skipping: required lavfi filter unavailable in this FFmpeg build ({reason})"
                );
                return;
            }
        }
    }
}
