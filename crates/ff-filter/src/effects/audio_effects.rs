//! Frame-level audio effects added to [`FilterGraph`] after construction.

use std::path::Path;

use crate::error::FilterError;
use crate::graph::FilterGraph;
use crate::graph::filter_step::FilterStep;

/// Noise type used as the initial spectral model for `afftdn`.
// Open catalog: `FFmpeg` noise colours (blue, violet, velvet, …) can be added.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoiseType {
    /// White noise (flat spectrum).
    White,
    /// Pink noise (−3 dB/octave).
    Pink,
    /// Brown / red noise (−6 dB/octave).
    Brown,
}

impl NoiseType {
    fn afftdn_flag(self) -> &'static str {
        match self {
            NoiseType::White => "w",
            NoiseType::Pink => "p",
            NoiseType::Brown => "b",
        }
    }
}

impl FilterGraph {
    /// Reduce noise using a statistical noise-type model.
    ///
    /// `nr_level` is the noise reduction amount in dB, clamped to [0.0, 97.0].
    ///
    /// Uses `FFmpeg`'s `afftdn` filter.
    pub fn noise_reduce(&mut self, nt: NoiseType, nr_level: f32) -> &mut Self {
        self.inner.push_step(FilterStep::NoiseReduce {
            noise_type_flag: nt.afftdn_flag().to_string(),
            nr_level: nr_level.clamp(0.0, 97.0),
        });
        self
    }

    /// Capture a noise profile from the first `profile_duration_secs` seconds,
    /// then reduce noise in the full stream.
    ///
    /// `nr_level` is the reduction amount in dB, clamped to [0.0, 97.0].
    /// `profile_duration_secs` is clamped to a minimum of 0.1 seconds.
    ///
    /// Uses `FFmpeg`'s `afftdn` filter with the `pl` profile-length option.
    pub fn noise_reduce_profile(&mut self, profile_duration_secs: f32, nr_level: f32) -> &mut Self {
        self.inner.push_step(FilterStep::NoiseReduceProfile {
            profile_duration_secs: profile_duration_secs.max(0.1),
            nr_level: nr_level.clamp(0.0, 97.0),
        });
        self
    }

    /// Change audio speed and pitch simultaneously by `factor`.
    ///
    /// Equivalent to playing a tape at a different speed: `factor > 1.0` makes
    /// audio faster and higher-pitched; `factor < 1.0` makes it slower and lower.
    ///
    /// Uses `FFmpeg`'s `asetrate` filter. Range: 0.1–10.0.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if `factor` is outside 0.1–10.0.
    pub fn speed_change(&mut self, factor: f64) -> Result<&mut Self, FilterError> {
        if !(0.1..=10.0).contains(&factor) {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("speed_change factor must be 0.1–10.0, got {factor}"),
            });
        }
        self.inner.push_step(FilterStep::SpeedChange { factor });
        Ok(self)
    }

    /// Shift audio pitch by `semitones` without changing playback speed.
    ///
    /// Range: −24.0 to +24.0 semitones. Uses `asetrate` to change the
    /// decoded sample rate followed by `atempo` to restore original duration.
    /// Beyond ±12 semitones the `atempo` compensation factor falls outside a
    /// single instance's `[0.5, 2.0]` range and is realised by chaining.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if `semitones` is outside −24.0..=24.0.
    pub fn pitch_shift(&mut self, semitones: f32) -> Result<&mut Self, FilterError> {
        if !(-24.0..=24.0).contains(&semitones) {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("semitones must be in -24..=24, got {semitones}"),
            });
        }
        self.inner.push_step(FilterStep::PitchShift { semitones });
        Ok(self)
    }

    /// Stretch or compress audio duration by `factor` without pitch change.
    ///
    /// `factor < 1.0` = slower (longer duration); `factor > 1.0` = faster
    /// (shorter duration). Range: 0.1–10.0.
    ///
    /// Uses `FFmpeg`'s `atempo` filter (WSOLA algorithm). Values outside
    /// [0.5, 2.0] are realised by chaining multiple `atempo` instances.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if `factor` is outside 0.1–10.0.
    pub fn time_stretch(&mut self, factor: f32) -> Result<&mut Self, FilterError> {
        if !(0.1..=10.0).contains(&factor) {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("time_stretch factor must be 0.1–10.0, got {factor}"),
            });
        }
        self.inner.push_step(FilterStep::TimeStretch { factor });
        Ok(self)
    }

    /// Add algorithmic echo/reverb with configurable delay taps.
    ///
    /// `in_gain` and `out_gain` are amplitude multipliers clamped to [0.0, 1.0].
    /// `delays` is a list of delay times in milliseconds.
    /// `decays` is the corresponding decay factor for each delay, clamped to [0.0, 1.0].
    /// `delays` and `decays` must have equal length (1–8 taps).
    ///
    /// Uses `FFmpeg`'s `aecho` filter.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if lengths differ or tap count is outside 1–8.
    pub fn reverb_echo(
        &mut self,
        in_gain: f32,
        out_gain: f32,
        delays: &[f32],
        decays: &[f32],
    ) -> Result<&mut Self, FilterError> {
        if delays.len() != decays.len() {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: "delays and decays must have equal length".into(),
            });
        }
        if !(1..=8).contains(&delays.len()) {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("tap count must be 1–8, got {}", delays.len()),
            });
        }
        self.inner.push_step(FilterStep::ReverbEcho {
            in_gain: in_gain.clamp(0.0, 1.0),
            out_gain: out_gain.clamp(0.0, 1.0),
            delays: delays.to_vec(),
            decays: decays.iter().map(|d| d.clamp(0.0, 1.0)).collect(),
        });
        Ok(self)
    }

    /// Reduce background audio level when foreground signal exceeds a threshold
    /// (audio ducking via sidechain compression).
    ///
    /// Push background audio to slot 0 and foreground (sidechain trigger) audio
    /// to slot 1.  When the foreground signal rises above `threshold_db`, the
    /// background is attenuated by `ratio`:1 over `attack_ms` milliseconds; it
    /// recovers over `release_ms` milliseconds when the foreground drops below the
    /// threshold.
    ///
    /// `threshold_db` is the trigger level in dBFS (e.g., −20.0).  It is
    /// converted to a linear amplitude ratio before being passed to the filter.
    /// `ratio` must be ≥ 1.0.  `attack_ms` and `release_ms` must be ≥ 0.0.
    ///
    /// Uses `FFmpeg`'s `sidechaincompress` filter.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if `ratio < 1.0` or either time value is
    /// negative.
    pub fn duck(
        &mut self,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
    ) -> Result<&mut Self, FilterError> {
        if ratio < 1.0 {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("duck ratio must be >= 1.0, got {ratio}"),
            });
        }
        if attack_ms < 0.0 {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("duck attack_ms must be >= 0.0, got {attack_ms}"),
            });
        }
        if release_ms < 0.0 {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("duck release_ms must be >= 0.0, got {release_ms}"),
            });
        }
        let threshold_linear = 10f32.powf(threshold_db / 20.0);
        self.inner.push_step(FilterStep::Duck {
            threshold_linear,
            ratio,
            attack_ms,
            release_ms,
        });
        Ok(self)
    }

    /// Add convolution reverb using an impulse response (IR) audio file.
    ///
    /// `ir_path` is a path to a `.wav` or `.flac` impulse response file.
    /// `wet` and `dry` are mix levels clamped to [0.0, 1.0].
    /// `pre_delay_ms` inserts silence before the reverb tail (clamped to 0–500 ms).
    ///
    /// Uses `FFmpeg`'s `amovie` (to load the IR) and `afir` (convolution) filters.
    ///
    /// Call this method after [`FilterGraph::builder()`] /
    /// [`FilterGraphBuilder::build()`](crate::FilterGraphBuilder::build) but
    /// **before** the first [`FilterGraph::push_audio()`] call.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if `ir_path` does not exist.
    /// The `afir` filter availability is checked at graph build time; if not
    /// available the graph build returns [`FilterError::BuildFailed`].
    pub fn reverb_ir(
        &mut self,
        ir_path: &Path,
        wet: f32,
        dry: f32,
        pre_delay_ms: u32,
    ) -> Result<&mut Self, FilterError> {
        if !ir_path.exists() {
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: format!("ir_path does not exist: {}", ir_path.display()),
            });
        }
        let ir_str = ir_path.display().to_string();
        self.inner.push_step(FilterStep::ReverbIr {
            ir_path: ir_str,
            wet: wet.clamp(0.0, 1.0),
            dry: dry.clamp(0.0, 1.0),
            pre_delay_ms: pre_delay_ms.min(500),
        });
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::effects::audio_effects::NoiseType;
    use crate::graph::filter_step::FilterStep;
    use crate::{FilterError, FilterGraph};
    use std::path::Path;

    #[test]
    fn noise_reduce_should_push_noise_reduce_step() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        graph.noise_reduce(NoiseType::White, 50.0);
        let step = FilterStep::NoiseReduce {
            noise_type_flag: "w".to_string(),
            nr_level: 50.0,
        };
        assert_eq!(step.filter_name(), "afftdn");
        assert!(
            step.args().contains("nt=w"),
            "args must contain nt=w: {}",
            step.args()
        );
        assert!(
            step.args().contains("nr=50"),
            "args must contain nr=50: {}",
            step.args()
        );
    }

    #[test]
    fn noise_reduce_clamps_nr_level_above_97() {
        let step = FilterStep::NoiseReduce {
            noise_type_flag: "p".to_string(),
            nr_level: 97.0,
        };
        assert!(
            step.args().contains("nr=97"),
            "nr_level=97.0 must appear in args: {}",
            step.args()
        );
    }

    #[test]
    fn noise_reduce_profile_args_should_contain_pl_and_nr() {
        let step = FilterStep::NoiseReduceProfile {
            profile_duration_secs: 0.5,
            nr_level: 30.0,
        };
        let args = step.args();
        assert!(args.contains("pl=0.5"), "args must contain pl=0.5: {args}");
        assert!(args.contains("nr=30"), "args must contain nr=30: {args}");
        assert!(args.contains("nf=-25"), "args must contain nf=-25: {args}");
    }

    #[test]
    fn noise_type_flags_should_match_afftdn_spec() {
        assert_eq!(NoiseType::White.afftdn_flag(), "w");
        assert_eq!(NoiseType::Pink.afftdn_flag(), "p");
        assert_eq!(NoiseType::Brown.afftdn_flag(), "b");
    }

    #[test]
    fn speed_change_zero_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.speed_change(0.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "factor=0.0 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn speed_change_above_range_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.speed_change(11.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "factor=11.0 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn speed_change_boundary_values_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(graph.speed_change(0.1).is_ok(), "factor=0.1 must succeed");
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(graph.speed_change(10.0).is_ok(), "factor=10.0 must succeed");
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(graph.speed_change(1.0).is_ok(), "factor=1.0 must succeed");
    }

    #[test]
    fn filter_step_speed_change_should_have_asetrate_filter_name() {
        let step = FilterStep::SpeedChange { factor: 2.0 };
        assert_eq!(step.filter_name(), "asetrate");
    }

    #[test]
    fn time_stretch_zero_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.time_stretch(0.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "factor=0.0 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn time_stretch_above_range_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.time_stretch(11.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "factor=11.0 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn time_stretch_boundary_values_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(graph.time_stretch(0.1).is_ok(), "factor=0.1 must succeed");
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(graph.time_stretch(10.0).is_ok(), "factor=10.0 must succeed");
    }

    #[test]
    fn filter_step_time_stretch_should_have_atempo_filter_name() {
        let step = FilterStep::TimeStretch { factor: 1.5 };
        assert_eq!(step.filter_name(), "atempo");
    }

    #[test]
    fn pitch_shift_24_semitones_up_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(
            graph.pitch_shift(24.0).is_ok(),
            "semitones=24.0 must succeed"
        );
    }

    #[test]
    fn pitch_shift_24_semitones_down_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(
            graph.pitch_shift(-24.0).is_ok(),
            "semitones=-24.0 must succeed"
        );
    }

    #[test]
    fn pitch_shift_above_24_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.pitch_shift(24.5);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "semitones=24.5 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn pitch_shift_below_minus_24_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.pitch_shift(-24.5);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "semitones=-24.5 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn pitch_shift_boundary_values_should_succeed() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(
            graph.pitch_shift(12.0).is_ok(),
            "semitones=12.0 must succeed"
        );
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(
            graph.pitch_shift(-12.0).is_ok(),
            "semitones=-12.0 must succeed"
        );
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(graph.pitch_shift(0.0).is_ok(), "semitones=0.0 must succeed");
    }

    #[test]
    fn filter_step_pitch_shift_should_have_asetrate_filter_name() {
        let step = FilterStep::PitchShift { semitones: 7.0 };
        assert_eq!(step.filter_name(), "asetrate");
    }

    #[test]
    fn reverb_echo_mismatched_lengths_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.reverb_echo(0.8, 0.9, &[500.0], &[0.5, 0.3]);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "mismatched lengths must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn reverb_echo_zero_taps_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.reverb_echo(0.8, 0.9, &[], &[]);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "zero taps must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn reverb_echo_nine_taps_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let delays = vec![100.0; 9];
        let decays = vec![0.5; 9];
        let result = graph.reverb_echo(0.8, 0.9, &delays, &decays);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "nine taps must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn filter_step_reverb_echo_should_have_aecho_filter_name() {
        let step = FilterStep::ReverbEcho {
            in_gain: 0.8,
            out_gain: 0.9,
            delays: vec![500.0],
            decays: vec![0.5],
        };
        assert_eq!(step.filter_name(), "aecho");
    }

    #[test]
    fn reverb_echo_args_should_contain_gains_delays_decays() {
        let step = FilterStep::ReverbEcho {
            in_gain: 0.8,
            out_gain: 0.9,
            delays: vec![500.0],
            decays: vec![0.5],
        };
        let args = step.args();
        assert!(
            args.contains("in_gain=0.8"),
            "args must contain in_gain=0.8: {args}"
        );
        assert!(
            args.contains("out_gain=0.9"),
            "args must contain out_gain=0.9: {args}"
        );
        assert!(
            args.contains("delays=500"),
            "args must contain delays=500: {args}"
        );
        assert!(
            args.contains("decays=0.5"),
            "args must contain decays=0.5: {args}"
        );
    }

    #[test]
    fn reverb_echo_multi_tap_args_should_join_with_pipe() {
        let step = FilterStep::ReverbEcho {
            in_gain: 0.8,
            out_gain: 0.9,
            delays: vec![500.0, 300.0],
            decays: vec![0.5, 0.3],
        };
        let args = step.args();
        assert!(
            args.contains("500|300") || args.contains("500.0|300"),
            "multi-tap delays must be joined with '|': {args}"
        );
        assert!(
            args.contains("0.5|0.3"),
            "multi-tap decays must be joined with '|': {args}"
        );
    }

    #[test]
    fn reverb_ir_nonexistent_path_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.reverb_ir(Path::new("no_such_file.wav"), 0.8, 0.2, 0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "non-existent ir_path must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn filter_step_reverb_ir_should_have_afir_filter_name() {
        let step = FilterStep::ReverbIr {
            ir_path: "hall.wav".to_string(),
            wet: 0.8,
            dry: 0.2,
            pre_delay_ms: 0,
        };
        assert_eq!(step.filter_name(), "afir");
    }

    #[test]
    fn reverb_ir_args_should_contain_wet_dry_and_ir_path() {
        let step = FilterStep::ReverbIr {
            ir_path: "hall.wav".to_string(),
            wet: 0.8,
            dry: 0.2,
            pre_delay_ms: 0,
        };
        let args = step.args();
        assert!(
            args.contains("hall.wav"),
            "args must contain ir_path: {args}"
        );
        assert!(
            args.contains("wet=0.8"),
            "args must contain wet=0.8: {args}"
        );
        assert!(
            args.contains("dry=0.2"),
            "args must contain dry=0.2: {args}"
        );
        assert!(
            !args.contains("adelay"),
            "no pre-delay when pre_delay_ms=0: {args}"
        );
    }

    #[test]
    fn reverb_ir_args_with_pre_delay_should_contain_adelay() {
        let step = FilterStep::ReverbIr {
            ir_path: "hall.wav".to_string(),
            wet: 0.8,
            dry: 0.2,
            pre_delay_ms: 100,
        };
        let args = step.args();
        assert!(
            args.contains("adelay=100"),
            "args must contain adelay=100 when pre_delay_ms=100: {args}"
        );
    }

    #[test]
    fn duck_ratio_below_one_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.duck(-20.0, 0.5, 10.0, 200.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "ratio=0.5 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn duck_negative_attack_ms_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.duck(-20.0, 20.0, -1.0, 200.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "attack_ms=-1.0 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn duck_negative_release_ms_should_return_ffmpeg_error() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        let result = graph.duck(-20.0, 20.0, 10.0, -1.0);
        assert!(
            matches!(result, Err(FilterError::Ffmpeg { .. })),
            "release_ms=-1.0 must return Err(FilterError::Ffmpeg {{ .. }}), got {result:?}"
        );
    }

    #[test]
    fn duck_valid_params_should_push_duck_step() {
        let mut graph = FilterGraph::builder().trim(0.0, 1.0).build().unwrap();
        assert!(
            graph.duck(-20.0, 20.0, 10.0, 200.0).is_ok(),
            "valid duck params must succeed"
        );
    }

    #[test]
    fn filter_step_duck_should_have_sidechaincompress_filter_name() {
        let step = FilterStep::Duck {
            threshold_linear: 0.1,
            ratio: 20.0,
            attack_ms: 10.0,
            release_ms: 200.0,
        };
        assert_eq!(step.filter_name(), "sidechaincompress");
    }

    #[test]
    fn filter_step_duck_args_should_contain_threshold_ratio_attack_release() {
        let step = FilterStep::Duck {
            threshold_linear: 0.1,
            ratio: 20.0,
            attack_ms: 10.0,
            release_ms: 200.0,
        };
        let args = step.args();
        assert!(
            args.contains("threshold=0.1"),
            "args must contain threshold=0.1: {args}"
        );
        assert!(
            args.contains("ratio=20"),
            "args must contain ratio=20: {args}"
        );
        assert!(
            args.contains("attack=10"),
            "args must contain attack=10: {args}"
        );
        assert!(
            args.contains("release=200"),
            "args must contain release=200: {args}"
        );
    }

    #[test]
    fn reverb_ir_pre_delay_above_500_should_be_clamped() {
        let step = FilterStep::ReverbIr {
            ir_path: "hall.wav".to_string(),
            wet: 0.8,
            dry: 0.2,
            pre_delay_ms: 999,
        };
        let args = step.args();
        assert!(
            args.contains("adelay=500"),
            "pre_delay_ms=999 must clamp to 500: {args}"
        );
    }
}
