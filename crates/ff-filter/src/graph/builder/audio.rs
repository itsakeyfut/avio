//! Audio filter methods for [`FilterGraphBuilder`].

#[allow(clippy::wildcard_imports)]
use super::*;

impl FilterGraphBuilder {
    // Audio filters

    /// Audio fade-in from silence, starting at `start_sec` seconds and reaching
    /// full volume after `duration_sec` seconds.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `duration_sec` is ≤ 0.0.
    #[must_use]
    pub fn afade_in(mut self, start_sec: f64, duration_sec: f64) -> Self {
        self.steps.push(FilterStep::AFadeIn {
            start: start_sec,
            duration: duration_sec,
        });
        self
    }

    /// Audio fade-out to silence, starting at `start_sec` seconds and reaching
    /// full silence after `duration_sec` seconds.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `duration_sec` is ≤ 0.0.
    #[must_use]
    pub fn afade_out(mut self, start_sec: f64, duration_sec: f64) -> Self {
        self.steps.push(FilterStep::AFadeOut {
            start: start_sec,
            duration: duration_sec,
        });
        self
    }

    /// Reverse audio playback using `FFmpeg`'s `areverse` filter.
    ///
    /// **Warning**: `areverse` buffers the entire clip in memory before producing
    /// any output. Only use this on short clips to avoid excessive memory usage.
    #[must_use]
    pub fn areverse(mut self) -> Self {
        self.steps.push(FilterStep::AReverse);
        self
    }

    /// Apply EBU R128 two-pass loudness normalization.
    ///
    /// `target_lufs` is the target integrated loudness (e.g. `−23.0`),
    /// `true_peak_db` is the true-peak ceiling (e.g. `−1.0`), and
    /// `lra` is the target loudness range in LU (e.g. `7.0`).
    ///
    /// Pass 1 measures integrated loudness with the `ebur128` filter.
    /// Pass 2 applies a linear `volume` correction.  All audio frames are
    /// buffered in memory between the two passes — use only for clips that
    /// fit comfortably in RAM.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `target_lufs >= 0.0`, `true_peak_db > 0.0`, or `lra <= 0.0`.
    #[must_use]
    pub fn loudness_normalize(mut self, target_lufs: f32, true_peak_db: f32, lra: f32) -> Self {
        self.steps.push(FilterStep::LoudnessNormalize {
            target_lufs,
            true_peak_db,
            lra,
        });
        self
    }

    /// Normalize the audio peak level to `target_db` dBFS using a two-pass approach.
    ///
    /// Pass 1 measures the true peak with `astats=metadata=1`.
    /// Pass 2 applies `volume={gain}dB` so the output peak reaches `target_db`.
    /// All audio frames are buffered in memory between the two passes — use only
    /// for clips that fit comfortably in RAM.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `target_db > 0.0` (cannot normalize above digital full scale).
    #[must_use]
    pub fn normalize_peak(mut self, target_db: f32) -> Self {
        self.steps.push(FilterStep::NormalizePeak { target_db });
        self
    }

    /// Apply a noise gate to suppress audio below a given threshold.
    ///
    /// Uses `FFmpeg`'s `agate` filter. Audio below `threshold_db` (dBFS) is
    /// attenuated; audio above it passes through unmodified. The threshold is
    /// converted from dBFS to the linear amplitude ratio expected by `agate`.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `attack_ms` or `release_ms` is ≤ 0.0.
    #[must_use]
    pub fn agate(mut self, threshold_db: f32, attack_ms: f32, release_ms: f32) -> Self {
        self.steps.push(FilterStep::ANoiseGate {
            threshold_db,
            attack_ms,
            release_ms,
        });
        self
    }

    /// Apply a dynamic range compressor to the audio.
    ///
    /// Uses `FFmpeg`'s `acompressor` filter. Audio peaks above `threshold_db`
    /// (dBFS) are reduced by `ratio`:1.  `makeup_db` applies additional gain
    /// after compression to restore perceived loudness.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `ratio < 1.0`, `attack_ms ≤ 0.0`, or `release_ms ≤ 0.0`.
    #[must_use]
    pub fn compressor(
        mut self,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
    ) -> Self {
        self.steps.push(FilterStep::ACompressor {
            threshold_db,
            ratio,
            attack_ms,
            release_ms,
            makeup_db,
        });
        self
    }

    /// Downmix stereo audio to mono by equally mixing both channels.
    ///
    /// Uses `FFmpeg`'s `pan` filter with the expression
    /// `mono|c0=0.5*c0+0.5*c1`.  The output has a single channel.
    #[must_use]
    pub fn stereo_to_mono(mut self) -> Self {
        self.steps.push(FilterStep::StereoToMono);
        self
    }

    /// Remap audio channels using `FFmpeg`'s `channelmap` filter.
    ///
    /// `mapping` is a `|`-separated list of output channel names taken from
    /// input channels, e.g. `"FR|FL"` swaps left and right.
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if
    /// `mapping` is empty.
    #[must_use]
    pub fn channel_map(mut self, mapping: &str) -> Self {
        self.steps.push(FilterStep::ChannelMap {
            mapping: mapping.to_string(),
        });
        self
    }

    /// Shift audio for A/V sync correction.
    ///
    /// Positive `ms`: uses `FFmpeg`'s `adelay` filter to delay the audio
    /// (audio plays later). Negative `ms`: uses `FFmpeg`'s `atrim` filter to
    /// advance the audio by trimming the start (audio plays earlier).
    /// Zero `ms` is a no-op.
    #[must_use]
    pub fn audio_delay(mut self, ms: f64) -> Self {
        self.steps.push(FilterStep::AudioDelay { ms });
        self
    }

    /// Concatenate `n_segments` sequential audio inputs using `FFmpeg`'s `concat` filter.
    ///
    /// Requires `n_segments` audio input slots (push to slots 0 through
    /// `n_segments - 1` in order). [`build`](Self::build) returns
    /// [`FilterError::InvalidConfig`] if `n_segments < 2`.
    #[must_use]
    pub fn concat_audio(mut self, n_segments: u32) -> Self {
        self.steps.push(FilterStep::ConcatAudio { n: n_segments });
        self
    }

    /// Adjust audio volume by `gain_db` decibels (negative = quieter).
    #[must_use]
    pub fn volume(mut self, gain_db: f64) -> Self {
        self.steps.push(FilterStep::Volume(gain_db));
        self
    }

    /// Mix `inputs` audio streams together (additive: the inputs are summed, not
    /// averaged; push each stream to its slot `0..inputs`).
    #[must_use]
    pub fn amix(mut self, inputs: usize) -> Self {
        self.steps.push(FilterStep::Amix(inputs));
        self
    }

    /// Apply a multi-band parametric equalizer.
    ///
    /// Each [`EqBand`] maps to one `FFmpeg` filter node chained in sequence:
    /// - [`EqBand::LowShelf`] → `lowshelf`
    /// - [`EqBand::HighShelf`] → `highshelf`
    /// - [`EqBand::Peak`] → `equalizer`
    ///
    /// [`build`](Self::build) returns [`FilterError::InvalidConfig`] if `bands`
    /// is empty.
    #[must_use]
    pub fn equalizer(mut self, bands: Vec<EqBand>) -> Self {
        self.steps.push(FilterStep::ParametricEq { bands });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_step_volume_should_produce_correct_args() {
        let step = FilterStep::Volume(-6.0);
        assert_eq!(step.filter_name(), "volume");
        assert_eq!(step.args(), "volume=-6dB");
    }

    #[test]
    fn volume_should_convert_db_to_ffmpeg_string() {
        assert_eq!(FilterStep::Volume(-6.0).args(), "volume=-6dB");
        assert_eq!(FilterStep::Volume(6.0).args(), "volume=6dB");
        assert_eq!(FilterStep::Volume(0.0).args(), "volume=0dB");
    }

    #[test]
    fn filter_step_amix_should_produce_correct_args() {
        let step = FilterStep::Amix(3);
        assert_eq!(step.filter_name(), "amix");
        assert_eq!(step.args(), "inputs=3:normalize=0");
    }

    #[test]
    fn filter_step_parametric_eq_should_have_filter_name_equalizer() {
        let step = FilterStep::ParametricEq {
            bands: vec![EqBand::Peak {
                freq_hz: 1000.0,
                gain_db: 3.0,
                q: 1.0,
            }],
        };
        assert_eq!(step.filter_name(), "equalizer");
    }

    #[test]
    fn eq_band_peak_should_produce_correct_args() {
        let band = EqBand::Peak {
            freq_hz: 1000.0,
            gain_db: 3.0,
            q: 1.0,
        };
        assert_eq!(band.args(), "f=1000:g=3:width_type=q:width=1");
    }

    #[test]
    fn eq_band_low_shelf_should_produce_correct_args() {
        let band = EqBand::LowShelf {
            freq_hz: 200.0,
            gain_db: -3.0,
            slope: 1.0,
        };
        assert_eq!(band.args(), "f=200:g=-3:s=1");
    }

    #[test]
    fn eq_band_high_shelf_should_produce_correct_args() {
        let band = EqBand::HighShelf {
            freq_hz: 8000.0,
            gain_db: 2.0,
            slope: 0.5,
        };
        assert_eq!(band.args(), "f=8000:g=2:s=0.5");
    }

    #[test]
    fn builder_equalizer_with_single_peak_band_should_succeed() {
        let result = FilterGraph::builder()
            .equalizer(vec![EqBand::Peak {
                freq_hz: 1000.0,
                gain_db: 3.0,
                q: 1.0,
            }])
            .build();
        assert!(
            result.is_ok(),
            "equalizer with single Peak band must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_equalizer_with_multiple_bands_should_succeed() {
        let result = FilterGraph::builder()
            .equalizer(vec![
                EqBand::LowShelf {
                    freq_hz: 200.0,
                    gain_db: -2.0,
                    slope: 1.0,
                },
                EqBand::Peak {
                    freq_hz: 1000.0,
                    gain_db: 3.0,
                    q: 1.4,
                },
                EqBand::HighShelf {
                    freq_hz: 8000.0,
                    gain_db: 1.0,
                    slope: 0.5,
                },
            ])
            .build();
        assert!(
            result.is_ok(),
            "equalizer with three bands must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_equalizer_with_empty_bands_should_return_invalid_config() {
        let result = FilterGraph::builder().equalizer(vec![]).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for empty bands, got {result:?}"
        );
    }

    #[test]
    fn filter_step_afade_in_should_have_correct_filter_name() {
        let step = FilterStep::AFadeIn {
            start: 0.0,
            duration: 1.0,
        };
        assert_eq!(step.filter_name(), "afade");
    }

    #[test]
    fn filter_step_afade_out_should_have_correct_filter_name() {
        let step = FilterStep::AFadeOut {
            start: 4.0,
            duration: 1.0,
        };
        assert_eq!(step.filter_name(), "afade");
    }

    #[test]
    fn filter_step_afade_in_should_produce_correct_args() {
        let step = FilterStep::AFadeIn {
            start: 0.0,
            duration: 1.0,
        };
        assert_eq!(step.args(), "type=in:start_time=0:duration=1");
    }

    #[test]
    fn filter_step_afade_out_should_produce_correct_args() {
        let step = FilterStep::AFadeOut {
            start: 4.0,
            duration: 1.0,
        };
        assert_eq!(step.args(), "type=out:start_time=4:duration=1");
    }

    #[test]
    fn builder_afade_in_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().afade_in(0.0, 1.0).build();
        assert!(
            result.is_ok(),
            "afade_in(0.0, 1.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_afade_out_with_valid_params_should_succeed() {
        let result = FilterGraph::builder().afade_out(4.0, 1.0).build();
        assert!(
            result.is_ok(),
            "afade_out(4.0, 1.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_afade_in_with_zero_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().afade_in(0.0, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for duration=0.0, got {result:?}"
        );
    }

    #[test]
    fn builder_afade_out_with_negative_duration_should_return_invalid_config() {
        let result = FilterGraph::builder().afade_out(4.0, -1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for duration=-1.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_areverse_should_produce_correct_filter_name_and_empty_args() {
        let step = FilterStep::AReverse;
        assert_eq!(step.filter_name(), "areverse");
        assert_eq!(step.args(), "");
    }

    #[test]
    fn builder_areverse_should_succeed() {
        let result = FilterGraph::builder().areverse().build();
        assert!(
            result.is_ok(),
            "areverse must build successfully, got {result:?}"
        );
    }

    #[test]
    fn filter_step_loudness_normalize_should_produce_correct_filter_name() {
        let step = FilterStep::LoudnessNormalize {
            target_lufs: -23.0,
            true_peak_db: -1.0,
            lra: 7.0,
        };
        assert_eq!(step.filter_name(), "ebur128");
    }

    #[test]
    fn filter_step_loudness_normalize_should_produce_correct_args() {
        let step = FilterStep::LoudnessNormalize {
            target_lufs: -23.0,
            true_peak_db: -1.0,
            lra: 7.0,
        };
        assert_eq!(step.args(), "peak=true:metadata=1");
    }

    #[test]
    fn builder_loudness_normalize_with_valid_params_should_succeed() {
        let result = FilterGraph::builder()
            .loudness_normalize(-23.0, -1.0, 7.0)
            .build();
        assert!(
            result.is_ok(),
            "loudness_normalize(-23.0, -1.0, 7.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_loudness_normalize_with_zero_target_lufs_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .loudness_normalize(0.0, -1.0, 7.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for target_lufs=0.0, got {result:?}"
        );
    }

    #[test]
    fn builder_loudness_normalize_with_positive_target_lufs_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .loudness_normalize(5.0, -1.0, 7.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for target_lufs=5.0, got {result:?}"
        );
    }

    #[test]
    fn builder_loudness_normalize_with_positive_true_peak_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .loudness_normalize(-23.0, 1.0, 7.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for true_peak_db=1.0, got {result:?}"
        );
    }

    #[test]
    fn builder_loudness_normalize_with_zero_lra_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .loudness_normalize(-23.0, -1.0, 0.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for lra=0.0, got {result:?}"
        );
    }

    #[test]
    fn builder_loudness_normalize_with_negative_lra_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .loudness_normalize(-23.0, -1.0, -7.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for lra=-7.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_normalize_peak_should_have_correct_filter_name() {
        let step = FilterStep::NormalizePeak { target_db: -1.0 };
        assert_eq!(step.filter_name(), "astats");
    }

    #[test]
    fn filter_step_normalize_peak_should_have_correct_args() {
        let step = FilterStep::NormalizePeak { target_db: -1.0 };
        assert_eq!(step.args(), "metadata=1");
    }

    #[test]
    fn builder_normalize_peak_valid_should_build_successfully() {
        let result = FilterGraph::builder().normalize_peak(-1.0).build();
        assert!(
            result.is_ok(),
            "normalize_peak(-1.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_normalize_peak_with_zero_target_db_should_build_successfully() {
        // 0.0 dBFS is the maximum allowed value (digital full scale).
        let result = FilterGraph::builder().normalize_peak(0.0).build();
        assert!(
            result.is_ok(),
            "normalize_peak(0.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_normalize_peak_with_positive_target_db_should_return_invalid_config() {
        let result = FilterGraph::builder().normalize_peak(1.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for target_db=1.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_agate_should_have_correct_filter_name() {
        let step = FilterStep::ANoiseGate {
            threshold_db: -40.0,
            attack_ms: 10.0,
            release_ms: 100.0,
        };
        assert_eq!(step.filter_name(), "agate");
    }

    #[test]
    fn filter_step_agate_should_produce_correct_args_for_minus_40_db() {
        let step = FilterStep::ANoiseGate {
            threshold_db: -40.0,
            attack_ms: 10.0,
            release_ms: 100.0,
        };
        // 10^(-40/20) = 10^(-2) = 0.01
        let args = step.args();
        assert!(
            args.starts_with("threshold=0.010000:"),
            "expected args to start with threshold=0.010000:, got {args}"
        );
        assert!(
            args.contains("attack=10:"),
            "expected attack=10: in args, got {args}"
        );
        assert!(
            args.contains("release=100"),
            "expected release=100 in args, got {args}"
        );
    }

    #[test]
    fn filter_step_agate_should_produce_correct_args_for_zero_db() {
        let step = FilterStep::ANoiseGate {
            threshold_db: 0.0,
            attack_ms: 5.0,
            release_ms: 50.0,
        };
        // 10^(0/20) = 1.0
        let args = step.args();
        assert!(
            args.starts_with("threshold=1.000000:"),
            "expected threshold=1.000000: in args, got {args}"
        );
    }

    #[test]
    fn builder_agate_valid_should_build_successfully() {
        let result = FilterGraph::builder().agate(-40.0, 10.0, 100.0).build();
        assert!(
            result.is_ok(),
            "agate(-40.0, 10.0, 100.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_agate_with_zero_attack_should_return_invalid_config() {
        let result = FilterGraph::builder().agate(-40.0, 0.0, 100.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for attack_ms=0.0, got {result:?}"
        );
    }

    #[test]
    fn builder_agate_with_negative_attack_should_return_invalid_config() {
        let result = FilterGraph::builder().agate(-40.0, -1.0, 100.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for attack_ms=-1.0, got {result:?}"
        );
    }

    #[test]
    fn builder_agate_with_zero_release_should_return_invalid_config() {
        let result = FilterGraph::builder().agate(-40.0, 10.0, 0.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for release_ms=0.0, got {result:?}"
        );
    }

    #[test]
    fn builder_agate_with_negative_release_should_return_invalid_config() {
        let result = FilterGraph::builder().agate(-40.0, 10.0, -50.0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for release_ms=-50.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_compressor_should_have_correct_filter_name() {
        let step = FilterStep::ACompressor {
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 6.0,
        };
        assert_eq!(step.filter_name(), "acompressor");
    }

    #[test]
    fn filter_step_compressor_should_produce_correct_args() {
        let step = FilterStep::ACompressor {
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 6.0,
        };
        assert_eq!(
            step.args(),
            "threshold=-20dB:ratio=4:attack=10:release=100:makeup=6dB"
        );
    }

    #[test]
    fn builder_compressor_valid_should_build_successfully() {
        let result = FilterGraph::builder()
            .compressor(-20.0, 4.0, 10.0, 100.0, 6.0)
            .build();
        assert!(
            result.is_ok(),
            "compressor(-20.0, 4.0, 10.0, 100.0, 6.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_compressor_with_unity_ratio_should_build_successfully() {
        // ratio=1.0 is the minimum valid value (no compression)
        let result = FilterGraph::builder()
            .compressor(-20.0, 1.0, 10.0, 100.0, 0.0)
            .build();
        assert!(
            result.is_ok(),
            "compressor with ratio=1.0 must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_compressor_with_ratio_below_one_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .compressor(-20.0, 0.5, 10.0, 100.0, 0.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for ratio=0.5, got {result:?}"
        );
    }

    #[test]
    fn builder_compressor_with_zero_attack_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .compressor(-20.0, 4.0, 0.0, 100.0, 0.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for attack_ms=0.0, got {result:?}"
        );
    }

    #[test]
    fn builder_compressor_with_zero_release_should_return_invalid_config() {
        let result = FilterGraph::builder()
            .compressor(-20.0, 4.0, 10.0, 0.0, 0.0)
            .build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for release_ms=0.0, got {result:?}"
        );
    }

    #[test]
    fn filter_step_stereo_to_mono_should_have_correct_filter_name() {
        assert_eq!(FilterStep::StereoToMono.filter_name(), "pan");
    }

    #[test]
    fn filter_step_stereo_to_mono_should_produce_correct_args() {
        assert_eq!(FilterStep::StereoToMono.args(), "mono|c0=0.5*c0+0.5*c1");
    }

    #[test]
    fn builder_stereo_to_mono_should_build_successfully() {
        let result = FilterGraph::builder().stereo_to_mono().build();
        assert!(
            result.is_ok(),
            "stereo_to_mono() must build successfully, got {result:?}"
        );
    }

    #[test]
    fn filter_step_channel_map_should_have_correct_filter_name() {
        let step = FilterStep::ChannelMap {
            mapping: "FR|FL".to_string(),
        };
        assert_eq!(step.filter_name(), "channelmap");
    }

    #[test]
    fn filter_step_channel_map_should_produce_correct_args() {
        let step = FilterStep::ChannelMap {
            mapping: "FR|FL".to_string(),
        };
        assert_eq!(step.args(), "map=FR|FL");
    }

    #[test]
    fn builder_channel_map_valid_should_build_successfully() {
        let result = FilterGraph::builder().channel_map("FR|FL").build();
        assert!(
            result.is_ok(),
            "channel_map(\"FR|FL\") must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_channel_map_with_empty_mapping_should_return_invalid_config() {
        let result = FilterGraph::builder().channel_map("").build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for empty mapping, got {result:?}"
        );
    }

    #[test]
    fn filter_step_audio_delay_positive_should_have_correct_filter_name() {
        let step = FilterStep::AudioDelay { ms: 100.0 };
        assert_eq!(step.filter_name(), "adelay");
    }

    #[test]
    fn filter_step_audio_delay_negative_should_have_correct_filter_name() {
        // filter_name() always returns "adelay" (used for validation only);
        // the build loop dispatches to "atrim" at runtime.
        let step = FilterStep::AudioDelay { ms: -100.0 };
        assert_eq!(step.filter_name(), "adelay");
    }

    #[test]
    fn filter_step_audio_delay_positive_should_produce_adelay_args() {
        let step = FilterStep::AudioDelay { ms: 100.0 };
        assert_eq!(step.args(), "delays=100:all=1");
    }

    #[test]
    fn filter_step_audio_delay_zero_should_produce_adelay_args() {
        let step = FilterStep::AudioDelay { ms: 0.0 };
        assert_eq!(step.args(), "delays=0:all=1");
    }

    #[test]
    fn filter_step_audio_delay_negative_should_produce_atrim_args() {
        let step = FilterStep::AudioDelay { ms: -100.0 };
        // -(-100) / 1000 = 0.1 seconds
        assert_eq!(step.args(), "start=0.1");
    }

    #[test]
    fn builder_audio_delay_positive_should_build_successfully() {
        let result = FilterGraph::builder().audio_delay(100.0).build();
        assert!(
            result.is_ok(),
            "audio_delay(100.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_audio_delay_zero_should_build_successfully() {
        let result = FilterGraph::builder().audio_delay(0.0).build();
        assert!(
            result.is_ok(),
            "audio_delay(0.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_audio_delay_negative_should_build_successfully() {
        let result = FilterGraph::builder().audio_delay(-100.0).build();
        assert!(
            result.is_ok(),
            "audio_delay(-100.0) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn filter_step_concat_audio_should_have_correct_filter_name() {
        let step = FilterStep::ConcatAudio { n: 2 };
        assert_eq!(step.filter_name(), "concat");
    }

    #[test]
    fn filter_step_concat_audio_should_produce_correct_args_for_n2() {
        let step = FilterStep::ConcatAudio { n: 2 };
        assert_eq!(step.args(), "n=2:v=0:a=1");
    }

    #[test]
    fn filter_step_concat_audio_should_produce_correct_args_for_n3() {
        let step = FilterStep::ConcatAudio { n: 3 };
        assert_eq!(step.args(), "n=3:v=0:a=1");
    }

    #[test]
    fn builder_concat_audio_valid_should_build_successfully() {
        let result = FilterGraph::builder().concat_audio(2).build();
        assert!(
            result.is_ok(),
            "concat_audio(2) must build successfully, got {result:?}"
        );
    }

    #[test]
    fn builder_concat_audio_with_n1_should_return_invalid_config() {
        let result = FilterGraph::builder().concat_audio(1).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for n=1, got {result:?}"
        );
    }

    #[test]
    fn builder_concat_audio_with_n0_should_return_invalid_config() {
        let result = FilterGraph::builder().concat_audio(0).build();
        assert!(
            matches!(result, Err(FilterError::InvalidConfig { .. })),
            "expected InvalidConfig for n=0, got {result:?}"
        );
    }
}
