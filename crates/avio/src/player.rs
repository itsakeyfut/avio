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
///     .frame_rate(30.into())
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
    /// cannot be opened, or a clip cannot be probed. Returns
    /// [`PreviewError::NeedsGpuCompositor`] when no GPU compositor could be attached
    /// and a clip uses a composite operator the CPU compositor refuses
    /// (`In`/`Out`/`Atop`/`Xor`, #1753): without it the runner would show the base
    /// frame with that layer silently missing.
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
        let gpu_attached = Self::attach_gpu_compositor(&mut runner, force_cpu);
        if !gpu_attached && let Some(reason) = cpu_compositor_refusal(timeline) {
            return Err(PreviewError::NeedsGpuCompositor { reason });
        }
        Ok((runner, handle))
    }

    /// Attaches the GPU compositor when the `gpu` feature is built, an adapter is
    /// available, and CPU is not forced, returning whether it did. A no-op returning
    /// `false` otherwise (the runner uses its built-in CPU compositor).
    #[cfg(feature = "gpu")]
    fn attach_gpu_compositor(runner: &mut SceneRunner, force_cpu: bool) -> bool {
        if force_cpu {
            log::info!("preview compositor path=cpu reason=forced");
            return false;
        }
        match crate::gpu_preview::GpuPreviewCompositor::new() {
            Some(gpu) => {
                runner.set_gpu_compositor(Box::new(gpu));
                true
            }
            None => false,
        }
    }

    #[cfg(not(feature = "gpu"))]
    fn attach_gpu_compositor(_runner: &mut SceneRunner, _force_cpu: bool) -> bool {
        false
    }
}

/// Why the CPU compositor would refuse `timeline`, if it would.
///
/// The filter path refuses `In`/`Out`/`Atop`/`Xor` when its graph is built (#1753,
/// ADR-0014), and the runner then shows the base frame with that layer missing. So a
/// timeline that needs one of them is refused here, before any decoding starts,
/// whenever no GPU compositor is attached. Checked on the model rather than the
/// derived scene so the message can name the track and clip, but with the same
/// track-activity rule the derivation applies: a disabled, muted or solo-shadowed
/// track contributes no layer, so an operator there is never built and must not
/// refuse the timeline. Every active layer counts, the base included: the CPU
/// compositor used to ignore the base layer's operator while the GPU applies it
/// against an empty backdrop, which is its own silent divergence.
fn cpu_compositor_refusal(timeline: &Timeline) -> Option<String> {
    let any_solo = timeline.video_tracks().iter().any(|t| t.solo);
    timeline
        .video_tracks()
        .iter()
        .enumerate()
        .filter(|(_, track)| track.is_active(any_solo))
        .find_map(|(t, track)| {
            track.clips.iter().enumerate().find_map(|(c, clip)| {
                (!clip.composite_op.is_filter_path_supported()).then(|| {
                    format!(
                        "video track {t} clip {c} uses composite operator {:?}, which the \
                         CPU compositor does not implement (#1784)",
                        clip.composite_op
                    )
                })
            })
        })
}
