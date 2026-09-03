//! The preview reaches its transition on a scene the *engine* derived (#1737).
//!
//! The runner arms a transition only while the outgoing clip is still producing frames
//! at the incoming clip's offset. Before ADR-0009 the engine derived clips that met
//! exactly (`overlap = 0 ns`), so the runner advanced to the incoming clip first and the
//! blend was never offered: measured 0 calls, against 13 for a hand-built overlapping
//! scene. The preview showed a hard cut wherever the export wrote a cross-fade, and
//! `ff-preview`'s seam suite passed the whole time because its fixture was a shape the
//! engine did not produce (RK-015).
//!
//! So this drives the **real** runner over a **`Timeline::to_scene`** derivation, and
//! counts. The injected compositor declines every blend (returns `None`), leaving the
//! runner on its own CPU path: what is asserted is that the transition *armed*, not what
//! it rendered. `xfade_reference_parity` owns the pixels.
//!
//! Probe-gated (RK-002): skips when the environment cannot encode or open the preview.
//! Its source files are keyed to this suite, so it is safe at default parallelism
//! (RK-019).

#![cfg(feature = "preview")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use avio::{Clip, PlayerHandle, PreviewCompositor, Timeline, TimelinePlayer};
use ff_encode::{VideoCodec, VideoEncoder};
use ff_filter::{RealtimeLayer, XfadeTransition};
use ff_format::{VideoFrame, VideoFrame as Frame};
use ff_preview::FrameSink;
use fixtures::{FileGuard, test_output_path};

const W: u32 = 64;
const H: u32 = 64;
const FPS: f64 = 30.0;
/// Each clip is trimmed to 1 s, and each source holds another second past that: the
/// handle the transition reads. A source cut flush to the clip would clamp the
/// transition to a hard cut and this suite would assert nothing; a source with exactly
/// 0.5 s to spare would clamp it by a frame, because a container reports its duration
/// one frame interval short of what was pushed.
const SOURCE_FRAMES: usize = 60;
const XFADE: Duration = Duration::from_millis(500);
/// Deliberately **not** `Fade`: the runner falls back to `Fade` when a placement carries
/// no kind (`next.xfade_kind.unwrap_or(XfadeTransition::Fade)`), so a suite that only
/// ever uses `Fade` would pass even if the kind never reached the placement at all.
const KIND: XfadeTransition = XfadeTransition::FadeBlack;
/// A spin guard: a healthy run delivers ~60 frames, so this only trips if the runner
/// fails to terminate (RK-019).
const MAX_FRAMES: u32 = 200;

/// Counts the blends the runner offers, and declines them all so the picture is
/// whatever the CPU path would have produced anyway.
struct CountingBlender {
    blends: Arc<Mutex<u32>>,
}

impl PreviewCompositor for CountingBlender {
    fn composite(
        &mut self,
        _layers: &[(&RealtimeLayer, &Frame)],
        _canvas: (u32, u32),
        _t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)> {
        None
    }

    fn blend(
        &mut self,
        kind: XfadeTransition,
        _a: &[u8],
        _b: &[u8],
        _progress: f32,
        _w: u32,
        _h: u32,
    ) -> Option<Vec<u8>> {
        assert_eq!(
            kind, KIND,
            "the runner must offer the kind the model authored, not its default"
        );
        *self
            .blends
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        None
    }
}

/// Stops the runner once it has delivered enough frames, so the test is bounded.
struct CountingSink {
    frames: u32,
    handle: PlayerHandle,
}

impl FrameSink for CountingSink {
    fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
        self.frames += 1;
        if self.frames >= MAX_FRAMES {
            self.handle.stop();
        }
    }
}

/// Writes `SOURCE_FRAMES` frames of a flat colour, or `None` when there is no encoder.
fn make_source(path: &std::path::Path, color: [u8; 3]) -> Option<()> {
    let mut enc = VideoEncoder::create(path)
        .video(W, H, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .build()
        .ok()?;
    let mut rgba = vec![255u8; (W * H * 4) as usize];
    for px in rgba.as_chunks_mut::<4>().0 {
        px[0] = color[0];
        px[1] = color[1];
        px[2] = color[2];
    }
    for _ in 0..SOURCE_FRAMES {
        enc.push_video(&VideoFrame::from_rgba(W, H, rgba.clone()).ok()?)
            .ok()?;
    }
    enc.finish().ok()?;
    Some(())
}

/// Two 1 s clips back to back, with a 0.5 s cross-fade into the second.
fn transitioned(a: &std::path::Path, b: &std::path::Path) -> Option<Timeline> {
    Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![
            Clip::new(a).trim(Duration::ZERO, Duration::from_secs(1)),
            Clip::new(b)
                .offset(Duration::from_secs(1))
                .trim(Duration::ZERO, Duration::from_secs(1))
                .with_transition(KIND, XFADE),
        ])
        .build()
        .ok()
}

/// Encodes the pair this suite runs on, or `None` to skip.
fn sources(tag: &str) -> Option<(std::path::PathBuf, std::path::PathBuf, Vec<FileGuard>)> {
    let a = test_output_path(&format!("preview_reach_{tag}_a.mp4"));
    let b = test_output_path(&format!("preview_reach_{tag}_b.mp4"));
    let guards = vec![FileGuard::new(a.clone()), FileGuard::new(b.clone())];
    make_source(&a, [200, 30, 30])?;
    make_source(&b, [30, 30, 200])?;
    Some((a, b, guards))
}

#[test]
fn a_derived_scene_should_give_the_outgoing_clip_the_handle_its_transition_needs() {
    // The projection on its own, before any runner: this is the shape #1737 was missing.
    let Some((a, b, _guards)) = sources("scene") else {
        return; // encoder unavailable -> skip
    };
    let Some(timeline) = transitioned(&a, &b) else {
        return;
    };
    let scene = timeline.to_scene();
    let placements = &scene.video_tracks[0].placements;
    assert_eq!(placements.len(), 2);

    assert_eq!(
        placements[0].video_handle, XFADE,
        "the outgoing clip must be allowed to produce frames across the whole \
         transition window, or the runner advances before it can arm the blend"
    );
    assert_eq!(
        placements[1].xfade_dur, XFADE,
        "the incoming clip carries the transition"
    );
    assert_eq!(
        placements[1].xfade_kind,
        Some(KIND),
        "the authored kind has to reach the placement: the runner defaults to Fade when \
         it is absent, so a lost kind renders as the wrong transition rather than none"
    );
    assert_eq!(
        placements[1].offset,
        Duration::from_secs(1),
        "no clip moves: the transition is fed by the handle, not by shifting the \
         timeline (ADR-0009)"
    );
    // The clips still meet rather than overlap. The handle is what bridges them, and it
    // is video-only, so the audio window is untouched.
    assert_eq!(
        placements[0].offset + Duration::from_secs(1),
        placements[1].offset,
        "the two clips must still tile the timeline"
    );
}

#[test]
fn the_preview_runner_should_reach_the_transition_on_a_derived_scene() {
    let Some((a, b, _guards)) = sources("run") else {
        return;
    };
    let Some(timeline) = transitioned(&a, &b) else {
        return;
    };

    let blends = Arc::new(Mutex::new(0u32));
    {
        let (mut runner, handle) = match TimelinePlayer::open(&timeline) {
            Ok(p) => p,
            Err(e) => {
                println!("skipping: preview open failed: {e}");
                return;
            }
        };
        runner.set_gpu_compositor(Box::new(CountingBlender {
            blends: Arc::clone(&blends),
        }));
        runner.set_sink(Box::new(CountingSink { frames: 0, handle }));
        let _ = runner.run();
    }

    let blends = *blends
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    println!("the runner offered {blends} blends");
    // Not a frame count: the runner is real-time and drops frames under load, so the
    // window can deliver fewer than its nominal 15. What is asserted is that it opened
    // at all, which is the whole of #1737.
    assert!(
        blends > 0,
        "the runner never armed the transition on an engine-derived scene: the preview \
         is showing a hard cut where the export writes a cross-fade (#1737)"
    );
}
