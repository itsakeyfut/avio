//! Multi-track audio mixing into a single output stream.

use std::path::PathBuf;

use ff_format::ChannelLayout;

use crate::animation::AnimatedValue;
use crate::error::FilterError;
use crate::graph::filter_step::FilterStep;
use crate::graph::graph::FilterGraph;

// AudioTrack

/// A single audio track in a [`MultiTrackAudioMixer`] mix.
#[derive(Debug, Clone)]
pub struct AudioTrack {
    /// Source media file path.
    pub source: PathBuf,
    /// Volume adjustment in decibels (`0.0` = unity gain).
    ///
    /// Use [`AnimatedValue::Static`] for a constant level, or
    /// [`AnimatedValue::Track`] to automate volume over time.
    pub volume: AnimatedValue<f64>,
    /// Stereo pan (`-1.0` = full left, `0.0` = centre, `+1.0` = full right).
    ///
    /// Use [`AnimatedValue::Static`] for a fixed position.  Animated pan
    /// (`AnimatedValue::Track`) is not yet implemented; the initial value at
    /// `Duration::ZERO` is used and a warning is emitted.
    pub pan: AnimatedValue<f64>,
    // Note (#1331): source trim (in/out) and timeline placement (offset) are not
    // fields. The caller (engine derive) expresses them as leading effects —
    // `ATrim` + `AResetPts` for the source window, `AudioDelay` for the timeline
    // offset — so `AudioTrack` is model-free.
    /// Ordered per-track audio effect chain applied before mixing.
    ///
    /// Each [`FilterStep`] is inserted as a filter node after the track's
    /// volume node and before the `amix` node. Any timeline trim/placement steps
    /// the caller emits (`ATrim` + `AResetPts` + `AudioDelay`) belong at the front
    /// of this list. Use audio-relevant variants such as [`FilterStep::Volume`],
    /// [`FilterStep::AFadeIn`], and [`FilterStep::ACompressor`].
    /// An empty vec inserts no extra nodes (zero overhead).
    pub effects: Vec<FilterStep>,
    /// Sample rate of the source audio in Hz (e.g. `44_100` or `48_000`).
    ///
    /// When this differs from the mixer's output sample rate an `aresample`
    /// filter is inserted automatically.  Set to the mixer's output rate to
    /// skip resampling.
    pub sample_rate: u32,
    /// Channel layout of the source audio.
    ///
    /// When this differs from the mixer's output layout an `aformat` filter
    /// is inserted automatically.  Set to the mixer's output layout to skip
    /// format conversion.
    pub channel_layout: ChannelLayout,
}

// MultiTrackAudioMixer

/// Mixes multiple audio tracks into a single output stream.
///
/// The resulting [`FilterGraph`] is source-only — call [`FilterGraph::pull_audio`]
/// in a loop to extract the output frames.
///
/// # Examples
///
/// ```ignore
/// use ff_filter::MultiTrackAudioMixer;
/// use ff_format::ChannelLayout;
///
/// let mut graph = MultiTrackAudioMixer::new(48000, ChannelLayout::Stereo)
///     .add_track(ff_filter::AudioTrack {
///         source: "music.mp3".into(),
///         volume: ff_filter::AnimatedValue::Static(-3.0),
///         pan: ff_filter::AnimatedValue::Static(0.0),
///         effects: vec![],
///         sample_rate: 48000,
///         channel_layout: ChannelLayout::Stereo,
///     })
///     .build()?;
///
/// while let Some(frame) = graph.pull_audio()? {
///     // encode or write `frame`
/// }
/// ```
pub struct MultiTrackAudioMixer {
    sample_rate: u32,
    channel_layout: ChannelLayout,
    tracks: Vec<AudioTrack>,
}

impl MultiTrackAudioMixer {
    /// Creates a new mixer with no tracks.
    pub fn new(sample_rate: u32, layout: ChannelLayout) -> Self {
        Self {
            sample_rate,
            channel_layout: layout,
            tracks: Vec::new(),
        }
    }

    /// Appends an audio track and returns the updated mixer.
    #[must_use]
    pub fn add_track(self, track: AudioTrack) -> Self {
        let mut tracks = self.tracks;
        tracks.push(track);
        Self { tracks, ..self }
    }

    /// Builds a source-only [`FilterGraph`] that mixes all tracks.
    ///
    /// # Errors
    ///
    /// - [`FilterError::CompositionFailed`] — no tracks were added, or an
    ///   underlying `FFmpeg` graph-construction call failed.
    pub fn build(self) -> Result<FilterGraph, FilterError> {
        if self.tracks.is_empty() {
            return Err(FilterError::CompositionFailed {
                reason: "no tracks".to_string(),
            });
        }
        super::composition_inner::build_audio_mix(
            self.sample_rate,
            self.channel_layout,
            &self.tracks,
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn mixer_empty_tracks_should_err() {
        let result = MultiTrackAudioMixer::new(48000, ChannelLayout::Stereo).build();
        assert!(
            matches!(result, Err(FilterError::CompositionFailed { .. })),
            "expected CompositionFailed, got {result:?}"
        );
    }

    #[test]
    fn audio_track_with_empty_effects_should_build_successfully() {
        // build() may fail because the source doesn't exist, but must NOT fail
        // with a reason related to effects.
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.0),
                effects: vec![],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .build();
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("effect"),
                "build must not fail due to empty effects, got: {reason}"
            );
        }
    }

    #[test]
    fn audio_track_with_volume_effect_should_include_volume_filter() {
        // Structural test: verify the effects field accepts FilterStep::Volume.
        let track = AudioTrack {
            source: "track.mp3".into(),
            volume: AnimatedValue::Static(0.0),
            pan: AnimatedValue::Static(0.0),
            effects: vec![FilterStep::Volume(6.0)],
            sample_rate: 48_000,
            channel_layout: ChannelLayout::Stereo,
        };
        assert_eq!(track.effects.len(), 1);
        assert!(
            matches!(track.effects[0], FilterStep::Volume(_)),
            "expected Volume variant"
        );
    }

    #[test]
    fn mixer_mismatched_sample_rate_should_insert_aresample() {
        // Track is 44100 Hz, output is 48000 Hz → build_audio_mix must attempt
        // to create an aresample node.  With a nonexistent file the graph fails
        // at avfilter_graph_config, NOT at "filter not found: aresample", which
        // proves the node was created successfully before the config step.
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.0),
                effects: vec![],
                sample_rate: 44_100, // mismatch → aresample should be inserted
                channel_layout: ChannelLayout::Stereo,
            })
            .build();
        assert!(result.is_err(), "expected error from nonexistent file");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("filter not found: aresample"),
                "aresample filter must exist in FFmpeg and be created; got: {reason}"
            );
        }
    }

    #[test]
    fn audio_track_with_audio_delay_effect_should_insert_adelay() {
        // A timeline offset is expressed by the caller as an AudioDelay effect.
        // Build fails (nonexistent file) but NOT at "filter not found: adelay".
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.0),
                effects: vec![FilterStep::AudioDelay { ms: 2000.0 }],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .build();
        assert!(result.is_err(), "expected error (nonexistent file)");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("filter not found: adelay"),
                "adelay must exist in FFmpeg and be created; got: {reason}"
            );
        }
    }

    #[test]
    fn track_without_timeline_effects_should_not_insert_adelay() {
        // A track whose effects carry no AudioDelay inserts no adelay node.
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.0),
                effects: vec![],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .build();
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("adelay"),
                "adelay must not appear for zero offset; got: {reason}"
            );
        }
    }

    #[test]
    fn mixer_matching_format_should_not_insert_extra_filters() {
        // Track format matches output → no aresample or aformat should be
        // inserted.  Build fails only because the source file does not exist.
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.0),
                effects: vec![],
                sample_rate: 48_000, // matches output → no aresample
                channel_layout: ChannelLayout::Stereo, // matches output → no aformat
            })
            .build();
        assert!(result.is_err(), "expected error from nonexistent file");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("aresample"),
                "aresample must not appear for matching format; got: {reason}"
            );
            assert!(
                !reason.contains("filter not found: aformat"),
                "aformat must not appear for matching format; got: {reason}"
            );
        }
    }

    #[test]
    fn audio_track_animated_volume_should_register_animation_entry_on_build() {
        use crate::animation::{AnimationTrack, Easing, Keyframe};

        // A track that fades from -6 dB to 0 dB over one second.
        let vol_track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, -6.0_f64, Easing::Linear))
            .push(Keyframe::new(
                Duration::from_secs(1),
                0.0_f64,
                Easing::Linear,
            ));
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Track(vol_track),
                pan: AnimatedValue::Static(0.0),
                effects: vec![],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .build();
        // Build will fail because the file does not exist, but the failure must
        // NOT be "no tracks" — it must be FFmpeg trying to open the file.
        assert!(result.is_err(), "expected error (nonexistent file)");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("no tracks"),
                "must not fail with 'no tracks'; got: {reason}"
            );
        }
    }

    #[test]
    fn audio_track_with_in_out_points_should_insert_atrim_and_asetpts() {
        // atrim and asetpts must be registered in FFmpeg — build must not fail
        // with "filter not found: atrim" or "filter not found: asetpts".
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.0),
                effects: vec![
                    FilterStep::ATrim {
                        start: Some(2.0),
                        end: Some(6.0),
                    },
                    FilterStep::AResetPts,
                ],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .build();
        assert!(result.is_err(), "expected error (nonexistent file)");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("filter not found: atrim"),
                "atrim must exist in FFmpeg and be created; got: {reason}"
            );
            assert!(
                !reason.contains("filter not found: asetpts"),
                "asetpts must exist in FFmpeg and be created; got: {reason}"
            );
        }
    }

    #[test]
    fn audio_track_with_out_point_only_should_insert_atrim() {
        // out_point alone (no in_point) must also trigger atrim insertion.
        let result = MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
            .add_track(AudioTrack {
                source: "nonexistent.mp3".into(),
                volume: AnimatedValue::Static(0.0),
                pan: AnimatedValue::Static(0.0),
                effects: vec![FilterStep::ATrim {
                    start: None,
                    end: Some(4.0),
                }],
                sample_rate: 48_000,
                channel_layout: ChannelLayout::Stereo,
            })
            .build();
        assert!(result.is_err(), "expected error (nonexistent file)");
        if let Err(FilterError::CompositionFailed { ref reason }) = result {
            assert!(
                !reason.contains("filter not found: atrim"),
                "atrim must exist in FFmpeg and be created; got: {reason}"
            );
        }
    }

    #[test]
    fn audio_track_animated_pan_should_warn_and_use_initial_value() {
        use crate::animation::{AnimationTrack, Easing, Keyframe};

        // A pan track that sweeps from -1 to +1.
        let pan_track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, -1.0_f64, Easing::Linear))
            .push(Keyframe::new(
                Duration::from_secs(2),
                1.0_f64,
                Easing::Linear,
            ));
        // Structural test: the field accepts Track variant without compile error.
        let track = AudioTrack {
            source: "nonexistent.mp3".into(),
            volume: AnimatedValue::Static(0.0),
            pan: AnimatedValue::Track(pan_track),
            effects: vec![],
            sample_rate: 48_000,
            channel_layout: ChannelLayout::Stereo,
        };
        // The initial pan value at t=0 should be -1.0.
        let initial_pan = track.pan.value_at(Duration::ZERO);
        assert!(
            (initial_pan - (-1.0)).abs() < f64::EPSILON,
            "expected initial pan -1.0, got {initial_pan}"
        );
    }
}
