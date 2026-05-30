//! Timeline clip data type.
//!
//! This module provides [`Clip`], a plain Rust value type representing a single
//! media clip on a timeline. `Clip` holds no `FFmpeg` context; it is interpreted
//! by `Timeline::render()` at call time to build filter graphs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ff_filter::{BlendMode, XfadeTransition};

/// A single media clip on a timeline.
///
/// `Clip` is a plain Rust value type — it holds no `FFmpeg` context. All fields
/// are public so callers can inspect them directly. `Timeline::render()` interprets
/// the clip's fields to build filter graphs at call time.
///
/// # Examples
///
/// ```
/// use ff_pipeline::Clip;
/// use std::time::Duration;
///
/// let clip = Clip::new("intro.mp4")
///     .trim(Duration::from_secs(2), Duration::from_secs(10))
///     .offset(Duration::from_secs(5));
///
/// assert_eq!(clip.duration(), Some(Duration::from_secs(8)));
/// ```
#[derive(Debug, Clone)]
pub struct Clip {
    /// Path to the source media file.
    pub source: PathBuf,
    /// Start point within the source file. `None` = beginning of file.
    pub in_point: Option<Duration>,
    /// End point within the source file. `None` = end of file.
    pub out_point: Option<Duration>,
    /// Start offset on the timeline (`Duration::ZERO` = beginning of composition).
    pub timeline_offset: Duration,
    /// Arbitrary key/value metadata attached to this clip.
    pub metadata: HashMap<String, String>,
    /// Transition applied at the start of this clip (from the previous clip on the same track).
    /// `None` = hard cut. Ignored for the first clip on a track.
    pub transition: Option<XfadeTransition>,
    /// Duration of the transition overlap. Ignored when `transition` is `None`.
    pub transition_duration: Duration,
    /// Per-clip volume adjustment in dB applied during audio mixing (`0.0` = unity gain).
    ///
    /// This value is independent of any track-level volume animation. When non-zero
    /// the clip's own gain overrides the track-level value; set to `0.0` to defer
    /// to the track level.
    ///
    /// Defaults to `0.0`.
    pub volume_db: f64,
    /// Audio fade-in duration at the start of the clip (`Duration::ZERO` = no fade).
    ///
    /// When non-zero, a linear ramp from silence to the clip's volume level is
    /// applied over this duration, starting at the clip's in-point.
    ///
    /// Defaults to `Duration::ZERO`.
    pub fade_in: Duration,
    /// Audio fade-out duration at the end of the clip (`Duration::ZERO` = no fade).
    ///
    /// When non-zero, a linear ramp from the clip's volume level to silence is
    /// applied over this duration, ending at the clip's out-point.
    /// Requires `out_point` to be set or the source file to be probeable so the
    /// `afade` start offset can be computed. Omitted with `log::warn!` at render
    /// time if the clip duration cannot be determined.
    ///
    /// Defaults to `Duration::ZERO`.
    pub fade_out: Duration,
    /// Per-clip brightness adjustment. Range: −1.0..=1.0. Default: 0.0 (no change).
    ///
    /// Applied via the `eq` video filter during `Timeline::render()`.
    /// Neutral value (`0.0`) produces bit-identical output to the no-eq path.
    pub brightness: f32,
    /// Per-clip contrast adjustment. Range: 0.0..=3.0. Default: 1.0 (no change).
    ///
    /// Applied via the `eq` video filter during `Timeline::render()`.
    /// Neutral value (`1.0`) produces bit-identical output to the no-eq path.
    pub contrast: f32,
    /// Per-clip saturation adjustment. Range: 0.0..=3.0. Default: 1.0 (no change).
    ///
    /// Applied via the `eq` video filter during `Timeline::render()`.
    /// Neutral value (`1.0`) produces bit-identical output to the no-eq path.
    pub saturation: f32,
    /// Per-clip overlay opacity applied when this clip is composited over a lower layer.
    /// Range: `0.0` (fully transparent) to `1.0` (fully opaque). Default: `1.0`.
    ///
    /// For [`BlendMode::Normal`], opacity is applied via a `colorchannelmixer` filter before
    /// the overlay. For photographic blend modes, it is forwarded to the `blend` filter's
    /// `all_opacity` parameter.
    ///
    /// Neutral value (`1.0`) produces bit-identical output to the no-opacity path.
    pub opacity: f32,
    /// Blend mode for compositing this clip over the layer(s) below it.
    /// Default: [`BlendMode::Normal`] (standard alpha-over composite).
    ///
    /// [`BlendMode::Normal`] uses `FFmpeg`'s `overlay` filter. All other variants use `FFmpeg`'s
    /// `blend` filter with the corresponding `all_mode`.
    pub blend_mode: BlendMode,
    /// Per-clip playback speed multiplier. Range: 0.1..=100.0. Default: 1.0 (normal speed).
    ///
    /// Applied via `setpts=PTS/{speed}` on the video stream and a chain of `atempo` filters
    /// on the audio stream during `Timeline::render()`.
    /// Neutral value (`1.0`) produces bit-identical output to the no-speed path.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_pipeline::Clip;
    ///
    /// let clip = Clip::new("scene.mp4").with_speed(2.0);
    /// assert_eq!(clip.speed, 2.0);
    /// ```
    pub speed: f64,
    /// Optional low-resolution proxy file to decode from instead of `source`.
    ///
    /// When `Some`, `Timeline::render()` decodes video frames from this proxy and
    /// scales them up to the original `source` resolution, producing full-resolution
    /// output while rendering from a smaller, faster-to-decode file. The original
    /// `source` must still be probeable so its resolution can be determined; if the
    /// probe fails, the proxy is ignored and `source` is used directly.
    ///
    /// Defaults to `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ff_pipeline::Clip;
    ///
    /// let clip = Clip::new("scene.mp4").proxy("scene_proxy_quarter.mp4");
    /// assert!(clip.proxy.is_some());
    /// ```
    pub proxy: Option<PathBuf>,
}

impl Clip {
    /// Creates a new clip from a source path with no trim points and zero timeline offset.
    pub fn new(source: impl AsRef<Path>) -> Self {
        Self {
            source: source.as_ref().to_path_buf(),
            in_point: None,
            out_point: None,
            timeline_offset: Duration::ZERO,
            metadata: HashMap::new(),
            transition: None,
            transition_duration: Duration::ZERO,
            volume_db: 0.0,
            fade_in: Duration::ZERO,
            fade_out: Duration::ZERO,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            speed: 1.0,
            proxy: None,
        }
    }

    /// Sets a low-resolution proxy file to decode from and returns the updated clip.
    ///
    /// During `Timeline::render()` frames are decoded from `proxy` and scaled up to
    /// the original `source` resolution. See [`Clip::proxy`](Self::proxy).
    #[must_use]
    pub fn proxy(self, proxy: impl AsRef<Path>) -> Self {
        Self {
            proxy: Some(proxy.as_ref().to_path_buf()),
            ..self
        }
    }

    /// Sets the in/out trim points and returns the updated clip.
    #[must_use]
    pub fn trim(self, in_point: Duration, out_point: Duration) -> Self {
        Self {
            in_point: Some(in_point),
            out_point: Some(out_point),
            ..self
        }
    }

    /// Sets the timeline start offset and returns the updated clip.
    #[must_use]
    pub fn offset(self, timeline_offset: Duration) -> Self {
        Self {
            timeline_offset,
            ..self
        }
    }

    /// Sets the visual transition from the previous clip into this one and returns
    /// the updated clip.
    ///
    /// The transition is applied at the boundary where the preceding clip ends and
    /// this clip begins. For the first clip on a track `transition` is ignored.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    /// use ff_filter::XfadeTransition;
    /// use std::time::Duration;
    ///
    /// let clip = Clip::new("b.mp4")
    ///     .with_transition(XfadeTransition::Fade, Duration::from_millis(500));
    ///
    /// assert_eq!(clip.transition, Some(XfadeTransition::Fade));
    /// assert_eq!(clip.transition_duration, Duration::from_millis(500));
    /// ```
    #[must_use]
    pub fn with_transition(self, kind: XfadeTransition, duration: Duration) -> Self {
        Self {
            transition: Some(kind),
            transition_duration: duration,
            ..self
        }
    }

    /// Sets the per-clip volume adjustment in dB and returns the updated clip.
    ///
    /// `0.0` is unity gain (no change). Positive values increase volume; negative
    /// values reduce it. When set to a non-zero value this overrides the track-level
    /// volume animation for this clip during rendering.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    ///
    /// let clip = Clip::new("narration.wav").volume(-6.0);
    /// assert_eq!(clip.volume_db, -6.0);
    /// ```
    #[must_use]
    pub fn volume(self, db: f64) -> Self {
        Self {
            volume_db: db,
            ..self
        }
    }

    /// Sets the audio fade-in duration and returns the updated clip.
    ///
    /// The fade starts at the beginning of the clip and ramps from silence to the
    /// clip's volume level over `duration`. `Duration::ZERO` disables the fade.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    /// use std::time::Duration;
    ///
    /// let clip = Clip::new("narration.wav").with_fade_in(Duration::from_secs(2));
    /// assert_eq!(clip.fade_in, Duration::from_secs(2));
    /// ```
    #[must_use]
    pub fn with_fade_in(self, duration: Duration) -> Self {
        Self {
            fade_in: duration,
            ..self
        }
    }

    /// Sets the audio fade-out duration and returns the updated clip.
    ///
    /// The fade starts `duration` before the end of the clip and ramps to silence.
    /// Requires `out_point` to be set or the source file to be probeable; omitted
    /// with `log::warn!` at render time if the clip duration cannot be determined.
    /// `Duration::ZERO` disables the fade.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    /// use std::time::Duration;
    ///
    /// let clip = Clip::new("narration.wav")
    ///     .trim(Duration::from_secs(0), Duration::from_secs(10))
    ///     .with_fade_out(Duration::from_secs(1));
    /// assert_eq!(clip.fade_out, Duration::from_secs(1));
    /// ```
    #[must_use]
    pub fn with_fade_out(self, duration: Duration) -> Self {
        Self {
            fade_out: duration,
            ..self
        }
    }

    /// Sets per-clip color correction and returns the updated clip.
    ///
    /// The three parameters map directly to the `FFmpeg` `eq` filter:
    /// - `brightness`: −1.0..=1.0, where `0.0` is no change.
    /// - `contrast`:    0.0..=3.0, where `1.0` is no change.
    /// - `saturation`:  0.0..=3.0, where `1.0` is no change.
    ///
    /// Neutral values (`brightness = 0.0`, `contrast = 1.0`, `saturation = 1.0`)
    /// produce bit-identical output to the no-eq render path — the `eq` filter is
    /// only inserted when at least one value differs from its neutral default.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    ///
    /// let clip = Clip::new("scene.mp4").with_color_correction(0.1, 1.2, 0.9);
    /// assert_eq!(clip.brightness, 0.1);
    /// assert_eq!(clip.contrast, 1.2);
    /// assert_eq!(clip.saturation, 0.9);
    /// ```
    #[must_use]
    pub fn with_color_correction(self, brightness: f32, contrast: f32, saturation: f32) -> Self {
        Self {
            brightness,
            contrast,
            saturation,
            ..self
        }
    }

    /// Sets the overlay opacity and returns the updated clip.
    ///
    /// `opacity` is clamped to `[0.0, 1.0]`.  The neutral value (`1.0`) produces
    /// bit-identical output to the no-opacity path.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    ///
    /// let clip = Clip::new("overlay.mp4").with_opacity(0.5);
    /// assert_eq!(clip.opacity, 0.5);
    /// ```
    #[must_use]
    pub fn with_opacity(self, opacity: f32) -> Self {
        Self {
            opacity: opacity.clamp(0.0, 1.0),
            ..self
        }
    }

    /// Sets the blend mode for compositing this clip over the layer below and returns
    /// the updated clip.
    ///
    /// [`BlendMode::Normal`] (the default) uses `FFmpeg`'s `overlay` filter.  All other
    /// variants use `FFmpeg`'s `blend` filter with the corresponding `all_mode`.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    /// use ff_filter::BlendMode;
    ///
    /// let clip = Clip::new("overlay.mp4").with_blend_mode(BlendMode::Multiply);
    /// assert_eq!(clip.blend_mode, BlendMode::Multiply);
    /// ```
    #[must_use]
    pub fn with_blend_mode(self, mode: BlendMode) -> Self {
        Self {
            blend_mode: mode,
            ..self
        }
    }

    /// Sets the per-clip playback speed multiplier and returns the updated clip.
    ///
    /// Values greater than `1.0` produce fast motion; values less than `1.0` produce slow
    /// motion. The speed is applied via `setpts=PTS/{speed}` on the video stream and a chain
    /// of `atempo` filters on the audio stream during `Timeline::render()`.
    ///
    /// The neutral value (`1.0`) produces bit-identical output to the no-speed path.
    ///
    /// # Example
    ///
    /// ```
    /// use ff_pipeline::Clip;
    ///
    /// let clip = Clip::new("scene.mp4").with_speed(2.0);
    /// assert_eq!(clip.speed, 2.0);
    /// ```
    #[must_use]
    pub fn with_speed(self, speed: f64) -> Self {
        Self { speed, ..self }
    }

    /// Returns `out_point - in_point` when both are `Some`, otherwise `None`.
    ///
    /// Does not open the source file.
    pub fn duration(&self) -> Option<Duration> {
        match (self.in_point, self.out_point) {
            (Some(in_pt), Some(out_pt)) => out_pt.checked_sub(in_pt),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_new_should_have_zero_offset() {
        let clip = Clip::new("video.mp4");
        assert_eq!(clip.timeline_offset, Duration::ZERO);
        assert!(clip.in_point.is_none());
        assert!(clip.out_point.is_none());
        assert!(clip.metadata.is_empty());
    }

    #[test]
    fn clip_new_should_default_transition_to_none() {
        let clip = Clip::new("video.mp4");
        assert!(clip.transition.is_none());
        assert_eq!(clip.transition_duration, Duration::ZERO);
    }

    #[test]
    fn clip_with_transition_should_set_fields() {
        use ff_filter::XfadeTransition;
        let clip = Clip::new("video.mp4")
            .with_transition(XfadeTransition::Fade, Duration::from_millis(500));
        assert_eq!(clip.transition, Some(XfadeTransition::Fade));
        assert_eq!(clip.transition_duration, Duration::from_millis(500));
    }

    #[test]
    fn clip_trim_should_set_in_out_points() {
        let clip = Clip::new("video.mp4").trim(Duration::from_secs(3), Duration::from_secs(9));
        assert_eq!(clip.in_point, Some(Duration::from_secs(3)));
        assert_eq!(clip.out_point, Some(Duration::from_secs(9)));
    }

    #[test]
    fn clip_duration_should_return_none_when_out_point_unset() {
        let clip = Clip::new("video.mp4");
        assert!(clip.duration().is_none());
    }

    #[test]
    fn clip_duration_should_return_difference_when_both_points_set() {
        let clip = Clip::new("video.mp4").trim(Duration::from_secs(2), Duration::from_secs(10));
        assert_eq!(clip.duration(), Some(Duration::from_secs(8)));
    }

    #[test]
    fn clip_new_should_default_volume_db_to_zero() {
        let clip = Clip::new("audio.wav");
        assert_eq!(clip.volume_db, 0.0);
    }

    #[test]
    fn clip_volume_should_set_volume_db() {
        let clip = Clip::new("audio.wav").volume(-6.0);
        assert_eq!(clip.volume_db, -6.0);
    }

    #[test]
    fn clip_volume_positive_should_set_volume_db() {
        let clip = Clip::new("audio.wav").volume(3.0);
        assert_eq!(clip.volume_db, 3.0);
    }

    #[test]
    fn clip_new_should_default_fade_fields_to_zero() {
        let clip = Clip::new("audio.wav");
        assert_eq!(clip.fade_in, Duration::ZERO);
        assert_eq!(clip.fade_out, Duration::ZERO);
    }

    #[test]
    fn clip_with_fade_in_should_set_fade_in() {
        let clip = Clip::new("audio.wav").with_fade_in(Duration::from_secs(2));
        assert_eq!(clip.fade_in, Duration::from_secs(2));
        assert_eq!(clip.fade_out, Duration::ZERO);
    }

    #[test]
    fn clip_with_fade_out_should_set_fade_out() {
        let clip = Clip::new("audio.wav")
            .trim(Duration::ZERO, Duration::from_secs(10))
            .with_fade_out(Duration::from_secs(1));
        assert_eq!(clip.fade_out, Duration::from_secs(1));
        assert_eq!(clip.fade_in, Duration::ZERO);
    }

    #[test]
    fn clip_fade_in_and_fade_out_can_be_chained() {
        let clip = Clip::new("audio.wav")
            .trim(Duration::ZERO, Duration::from_secs(10))
            .with_fade_in(Duration::from_millis(500))
            .with_fade_out(Duration::from_millis(500));
        assert_eq!(clip.fade_in, Duration::from_millis(500));
        assert_eq!(clip.fade_out, Duration::from_millis(500));
    }

    #[test]
    fn clip_new_should_default_color_correction_to_neutral() {
        let clip = Clip::new("video.mp4");
        assert_eq!(clip.brightness, 0.0);
        assert_eq!(clip.contrast, 1.0);
        assert_eq!(clip.saturation, 1.0);
    }

    #[test]
    fn clip_with_color_correction_should_set_fields() {
        let clip = Clip::new("scene.mp4").with_color_correction(0.1, 1.2, 0.9);
        assert_eq!(clip.brightness, 0.1);
        assert_eq!(clip.contrast, 1.2);
        assert_eq!(clip.saturation, 0.9);
    }

    #[test]
    fn clip_new_should_default_speed_to_one() {
        let clip = Clip::new("video.mp4");
        assert_eq!(clip.speed, 1.0);
    }

    #[test]
    fn clip_with_speed_should_set_speed() {
        let clip = Clip::new("video.mp4").with_speed(2.0);
        assert_eq!(clip.speed, 2.0);
    }

    #[test]
    fn clip_with_speed_slow_motion_should_set_speed() {
        let clip = Clip::new("video.mp4").with_speed(0.5);
        assert_eq!(clip.speed, 0.5);
    }

    #[test]
    fn clip_new_should_default_opacity_to_one() {
        let clip = Clip::new("video.mp4");
        assert_eq!(clip.opacity, 1.0);
    }

    #[test]
    fn clip_with_opacity_should_set_opacity() {
        let clip = Clip::new("overlay.mp4").with_opacity(0.5);
        assert_eq!(clip.opacity, 0.5);
    }

    #[test]
    fn clip_with_opacity_should_clamp_above_one() {
        let clip = Clip::new("overlay.mp4").with_opacity(1.5);
        assert_eq!(clip.opacity, 1.0);
    }

    #[test]
    fn clip_with_opacity_should_clamp_below_zero() {
        let clip = Clip::new("overlay.mp4").with_opacity(-0.5);
        assert_eq!(clip.opacity, 0.0);
    }

    #[test]
    fn clip_new_should_default_blend_mode_to_normal() {
        use ff_filter::BlendMode;
        let clip = Clip::new("video.mp4");
        assert_eq!(clip.blend_mode, BlendMode::Normal);
    }

    #[test]
    fn clip_with_blend_mode_should_set_blend_mode() {
        use ff_filter::BlendMode;
        let clip = Clip::new("overlay.mp4").with_blend_mode(BlendMode::Multiply);
        assert_eq!(clip.blend_mode, BlendMode::Multiply);
    }

    #[test]
    fn clip_with_blend_mode_screen_should_set_blend_mode() {
        use ff_filter::BlendMode;
        let clip = Clip::new("overlay.mp4").with_blend_mode(BlendMode::Screen);
        assert_eq!(clip.blend_mode, BlendMode::Screen);
    }
}
