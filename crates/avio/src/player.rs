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
    /// With the `gpu` feature, the runner composites on the GPU by default when an
    /// adapter is available, falling back to the CPU compositor automatically when
    /// it is not (or per frame on unsupported content / a GPU error). Use
    /// [`open_forcing_cpu`](Self::open_forcing_cpu) to keep the CPU path even when a
    /// GPU is present.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError`] when the scene has no video tracks, a source file
    /// cannot be opened, or a clip cannot be probed.
    pub fn open(timeline: &Timeline) -> Result<(SceneRunner, PlayerHandle), PreviewError> {
        Self::open_inner(timeline, false)
    }

    /// Open `timeline` forcing the CPU compositor even when a GPU adapter is
    /// available (deterministic playback / testing).
    ///
    /// # Errors
    ///
    /// Same as [`open`](Self::open).
    pub fn open_forcing_cpu(
        timeline: &Timeline,
    ) -> Result<(SceneRunner, PlayerHandle), PreviewError> {
        Self::open_inner(timeline, true)
    }

    fn open_inner(
        timeline: &Timeline,
        force_cpu: bool,
    ) -> Result<(SceneRunner, PlayerHandle), PreviewError> {
        let (mut runner, handle) = ScenePlayer::open(&timeline.to_scene())?;
        Self::attach_gpu_compositor(&mut runner, force_cpu);
        Ok((runner, handle))
    }

    /// Attaches the GPU compositor when the `gpu` feature is built, an adapter is
    /// available, and CPU is not forced. A no-op otherwise (the runner uses its
    /// built-in CPU compositor).
    #[cfg(feature = "gpu")]
    fn attach_gpu_compositor(runner: &mut SceneRunner, force_cpu: bool) {
        if force_cpu {
            log::info!("preview compositor path=cpu reason=forced");
            return;
        }
        if let Some(gpu) = crate::gpu_preview::GpuPreviewCompositor::new() {
            runner.set_gpu_compositor(Box::new(gpu));
        }
    }

    #[cfg(not(feature = "gpu"))]
    fn attach_gpu_compositor(_runner: &mut SceneRunner, _force_cpu: bool) {}
}
