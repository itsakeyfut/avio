//! Timeline-facing real-time preview entry point.
//!
//! [`TimelinePlayer`] is the engine-level bridge from the editing model to the
//! primitive preview runner: it derives a [`Scene`](ff_preview::Scene) from a
//! [`Timeline`] and hands it to [`ff_preview::ScenePlayer`], which owns the
//! decode pipelines and audio mixer. The runner itself
//! ([`SceneRunner`](ff_preview::SceneRunner)) stays in `ff-preview` as a
//! model-agnostic `Scene` consumer.

use ff_preview::{PlayerHandle, PreviewError, ScenePlayer, SceneRunner};

use crate::timeline::Timeline;

/// Thin builder for a ([`SceneRunner`], [`PlayerHandle`]) pair backed by a
/// [`Timeline`].
///
/// # Example
///
/// ```ignore
/// use avio::{Timeline, Clip, TimelinePlayer};
/// use ff_preview::RgbaSink;
/// use std::time::Duration;
///
/// let timeline = Timeline::builder()
///     .canvas(1920, 1080)
///     .frame_rate(30.0)
///     .video_track(vec![
///         Clip::new("intro.mp4").trim(Duration::ZERO, Duration::from_secs(5)),
///     ])
///     .build()?;
///
/// let (mut runner, handle) = TimelinePlayer::open(&timeline)?;
/// runner.set_sink(Box::new(RgbaSink::new()));
/// std::thread::spawn(move || { let _ = runner.run(); });
/// handle.play();
/// ```
pub struct TimelinePlayer;

impl TimelinePlayer {
    /// Open `timeline` for real-time preview playback.
    ///
    /// Derives a [`Scene`](ff_preview::Scene) from the timeline via
    /// [`Timeline::to_scene`] and opens it with
    /// [`ScenePlayer::open`](ff_preview::ScenePlayer::open), which probes each
    /// clip's source, opens a decode buffer per V1 clip, and builds the audio
    /// mixer.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] when the scene has no video tracks, a source file
    /// cannot be opened, or a clip cannot be probed.
    pub fn open(timeline: &Timeline) -> Result<(SceneRunner, PlayerHandle), PreviewError> {
        ScenePlayer::open(&timeline.to_scene())
    }
}
