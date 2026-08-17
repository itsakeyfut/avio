//! Model-agnostic description of a timeline for the real-time preview runner.
//!
//! A [`Scene`] is the primitive seam between an editing engine and
//! [`SceneRunner`](super::runner::SceneRunner): it carries only the
//! model's primitivised fields (paths, durations, opacity, blend, animation
//! tracks), never the editing model itself. Resolving a `Scene` against the
//! media — probing for duration, audio presence, and frame size — happens in
//! [`ScenePlayer::open`](super::ScenePlayer::open), so a `Scene` re-derived on
//! every edit needs no re-probe and behaviour is preserved. An engine derives a
//! `Scene` from its own editing model; `Scene` is the only input the runner accepts.

use std::path::PathBuf;
use std::time::Duration;

use ff_filter::{AnimatedValue, RealtimeLayerDescriptor, XfadeTransition};

// ── Scene ─────────────────────────────────────────────────────────────────────

/// A whole timeline's worth of playback work, described without the editing model.
#[derive(Debug, Clone)]
pub struct Scene {
    /// Presentation frame rate. Clamped to at least `1.0` by the runner.
    pub fps: f64,
    /// Explicit output canvas `(width, height)`, or `None` to size from the base track.
    pub canvas: Option<(u32, u32)>,
    /// Optional timeline-global `lavfi` filtergraph string (e.g.
    /// `color=s=1920x1080:c=black@0.0,drawtext=text='Title'`) generated and composited
    /// as the **topmost** video layer, matching the export path. `None` = no overlay.
    pub lavfi_overlay: Option<String>,
    /// Video tracks, composited bottom-up: index `0` is the V1 base, `1..` are overlays.
    pub video_tracks: Vec<SceneVideoTrack>,
    /// Dedicated audio-only tracks (A1, A2, …).
    pub audio_tracks: Vec<SceneAudioTrack>,
}

// ── SceneVideoTrack ─────────────────────────────────────────────────────────────

/// One video track: an ordered list of clip placements along the timeline. The
/// track's index in [`Scene::video_tracks`] is its compositing order (`0` = base).
#[derive(Debug, Clone)]
pub struct SceneVideoTrack {
    /// Placements in timeline order.
    pub placements: Vec<ScenePlacement>,
}

// ── ScenePlacement ──────────────────────────────────────────────────────────────

/// One video clip placed on the timeline.
///
/// Fields are pre-resolved model projections (e.g. `in_point` defaulted, `speed`
/// clamped, `xfade_dur` computed); media-dependent resolution (the effective
/// clip duration when `out_point` is `None`) is done by the runner at open time.
#[derive(Debug, Clone)]
pub struct ScenePlacement {
    /// Source media path.
    pub source: PathBuf,
    /// Global timeline position where this placement starts.
    pub offset: Duration,
    /// Source-file PTS at which playback starts (`Clip::in_point`, defaulted to zero).
    pub in_point: Duration,
    /// Source-file PTS at which playback ends (`None` = play to EOF).
    pub out_point: Option<Duration>,
    /// Playback speed multiplier (`1.0` = normal), clamped to at least `0.01`.
    pub speed: f64,
    /// Crossfade duration from the previous placement into this one. Meaningful on
    /// the V1 base track only; `Duration::ZERO` = hard cut.
    pub xfade_dur: Duration,
    /// The `xfade` transition kind for this crossfade (V1 base track only). `None`
    /// when there is no transition; the runner defaults to `Fade` if a duration is
    /// set without a kind.
    pub xfade_kind: Option<XfadeTransition>,
    /// Per-clip opacity in `[0.0, 1.0]`.
    pub opacity: f32,
    /// The dimension-free compositing description (effects, blend, position, and the
    /// opacity/position animation tracks). The runner realises it per frame via
    /// [`RealtimeLayer::with_dimensions`](ff_filter::RealtimeLayer::with_dimensions).
    pub layer: RealtimeLayerDescriptor,
    /// Audio fade-in duration (`Duration::ZERO` = none).
    pub fade_in: Duration,
    /// Audio fade-out duration (`Duration::ZERO` = none).
    pub fade_out: Duration,
    /// Resolved audio gain (dB), static or animated. A static value is applied once at
    /// open; an animated one is evaluated per tick.
    pub volume: AnimatedValue<f64>,
}

// ── SceneAudioTrack ─────────────────────────────────────────────────────────────

/// One dedicated audio-only track (A1, A2, …).
#[derive(Debug, Clone)]
pub struct SceneAudioTrack {
    /// Placements in timeline order.
    pub placements: Vec<SceneAudioPlacement>,
}

// ── SceneAudioPlacement ─────────────────────────────────────────────────────────

/// One audio-only clip placed on the timeline.
#[derive(Debug, Clone)]
pub struct SceneAudioPlacement {
    /// Source media path.
    pub source: PathBuf,
    /// Global timeline position where this placement starts.
    pub offset: Duration,
    /// Source-file PTS at which playback starts (defaulted to zero).
    pub in_point: Duration,
    /// Source-file PTS at which playback ends (`None` = play to EOF).
    pub out_point: Option<Duration>,
    /// Playback speed multiplier (`1.0` = normal), clamped to at least `0.01`,
    /// applied by resampling.
    pub speed: f64,
    /// Audio fade-in duration (`Duration::ZERO` = none).
    pub fade_in: Duration,
    /// Audio fade-out duration (`Duration::ZERO` = none).
    pub fade_out: Duration,
    /// Resolved audio gain (dB), static or animated. A static value is applied once at
    /// open; an animated one is evaluated per tick.
    pub volume: AnimatedValue<f64>,
}
