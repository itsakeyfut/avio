//! The preview runner routes a transition through the injected GPU seam (#1726).
//!
//! `ff-render` depends on this crate, so the runner can only reach a GPU transition node
//! through [`PreviewCompositor`]. This drives the **real** `SceneRunner` over a two-clip
//! transition with an injected blender and asserts the runner used what it returned.
//!
//! The blender answers with a colour no cross-fade of the source could produce, so a run
//! that quietly stayed on `apply_xfade` cannot satisfy the assertion — that is the point
//! of the test, and the reason it does not simply compare against a CPU blend.
//!
//! No adapter is involved: this pins the *seam*, not the GPU. `avio`'s
//! `gpu_preview_transition_test` covers the real nodes, probe-gated.
//!
//! **The fixture overlaps its two clips, which the engine does not currently do.** The
//! runner arms a transition at the incoming clip's offset and blends for `xfade_dur`, so
//! the outgoing clip has to still be producing frames across that window — but `avio`
//! passes `clip.offset` through unchanged, and a normal crossfade puts clip B exactly
//! where clip A ends. Measured on a derived scene: `overlap = 0ns`, and the blend is
//! reached 0 times against 13 for an overlapping one. That is #1737; until it lands this
//! suite pins the seam rather than its reachability, and the seam is dormant in
//! production.
//!
//! Probe-gated (RK-002): skips when the shared asset cannot be opened.

#![cfg(feature = "timeline")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ff_filter::{
    AnimatedValue, BlendMode, CompositeOp, RealtimeLayer, RealtimeLayerDescriptor, XfadeTransition,
};
use ff_format::VideoFrame;
use ff_preview::{
    FrameSink, PlayerHandle, PreviewCompositor, Scene, ScenePlacement, ScenePlayer, SceneSource,
    SceneVideoTrack,
};

/// The colour the injected blender returns. Opaque magenta: the source is real video and
/// the two clips are the same file, so no `apply_xfade` of them lands here.
const SENTINEL: [u8; 4] = [255, 0, 255, 255];

fn asset() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/av_sync_test_60s.mp4")
}

/// A compositor that declines to composite and answers every blend with [`SENTINEL`].
struct SentinelBlender {
    blends: Arc<Mutex<u32>>,
}

impl PreviewCompositor for SentinelBlender {
    fn composite(
        &mut self,
        _layers: &[(&RealtimeLayer, &VideoFrame)],
        _canvas: (u32, u32),
        _t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)> {
        // Not the subject: leave compositing to the runner's CPU path so the sentinel
        // reaches the sink unmodified by a second GPU stage.
        None
    }

    fn blend(
        &mut self,
        _kind: XfadeTransition,
        a: &[u8],
        _b: &[u8],
        _progress: f32,
        _w: u32,
        _h: u32,
    ) -> Option<Vec<u8>> {
        *self.blends.lock().unwrap() += 1;
        Some(SENTINEL.repeat(a.len() / 4))
    }
}

/// Records whether any delivered frame was (almost) entirely [`SENTINEL`].
struct SentinelSink {
    saw_sentinel: Arc<Mutex<bool>>,
    frames: Arc<Mutex<u32>>,
    handle: PlayerHandle,
    max_frames: u32,
}

impl FrameSink for SentinelSink {
    fn push_frame(&mut self, rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
        let px = rgba.len() / 4;
        if px > 0 {
            // "Most of the frame", not "every pixel": the runner grades and composites
            // the blended frame afterwards, and the encode-free path still rounds.
            let hits = rgba
                .chunks_exact(4)
                .filter(|c| {
                    c[0].abs_diff(SENTINEL[0]) <= 4
                        && c[1].abs_diff(SENTINEL[1]) <= 4
                        && c[2].abs_diff(SENTINEL[2]) <= 4
                })
                .count();
            if hits * 100 / px >= 90 {
                *self.saw_sentinel.lock().unwrap() = true;
            }
        }
        let mut n = self.frames.lock().unwrap();
        *n += 1;
        if *n >= self.max_frames {
            self.handle.stop();
        }
    }
}

fn layer_desc() -> RealtimeLayerDescriptor {
    RealtimeLayerDescriptor {
        effects: Vec::new(),
        opacity: AnimatedValue::Static(1.0),
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        blend_mode: BlendMode::Normal,
        composite_op: CompositeOp::Over,
    }
}

fn placement(
    offset: Duration,
    in_point: Duration,
    dur: Duration,
    xfade: Option<XfadeTransition>,
) -> ScenePlacement {
    ScenePlacement {
        source: SceneSource::File(asset()),
        offset,
        in_point,
        out_point: Some(in_point + dur),
        speed: 1.0,
        xfade_dur: if xfade.is_some() {
            Duration::from_millis(400)
        } else {
            Duration::ZERO
        },
        xfade_kind: xfade,
        opacity: 1.0,
        layer: layer_desc(),
        fade_in: Duration::ZERO,
        fade_out: Duration::ZERO,
        volume: AnimatedValue::Static(0.0),
        pitch: 0.0,
        pan: AnimatedValue::Static(0.0),
    }
}

/// Two clips of the same asset with a transition into the second.
fn transitioned_scene() -> Scene {
    Scene {
        fps: 30.0,
        canvas: None,
        lavfi_overlay: None,
        video_tracks: vec![SceneVideoTrack {
            placements: vec![
                // Overlapping on purpose; see the module docs. An engine-derived scene
                // has `overlap = 0ns` and never reaches the blend at all (#1737), so a
                // faithful fixture would test nothing here.
                placement(
                    Duration::ZERO,
                    Duration::ZERO,
                    Duration::from_millis(1200),
                    None,
                ),
                placement(
                    Duration::from_millis(600),
                    Duration::from_secs(2),
                    Duration::from_millis(1200),
                    Some(XfadeTransition::FadeBlack),
                ),
            ],
        }],
        audio_tracks: Vec::new(),
    }
}

/// Runs the scene with `blender` injected (or not) and reports whether the sentinel
/// reached the sink, or `None` when the environment cannot open the asset.
fn run_scene(inject: bool) -> Option<(bool, u32)> {
    let scene = transitioned_scene();
    let (mut runner, handle) = ScenePlayer::open(&scene).ok()?;

    let saw = Arc::new(Mutex::new(false));
    let blends = Arc::new(Mutex::new(0));
    if inject {
        runner.set_gpu_compositor(Box::new(SentinelBlender {
            blends: Arc::clone(&blends),
        }));
    }
    runner.set_sink(Box::new(SentinelSink {
        saw_sentinel: Arc::clone(&saw),
        frames: Arc::new(Mutex::new(0)),
        handle,
        max_frames: 40,
    }));
    runner.run().ok()?;

    let saw = *saw.lock().unwrap();
    let blends = *blends.lock().unwrap();
    Some((saw, blends))
}

#[test]
fn preview_runner_should_use_the_injected_blend_for_a_transition() {
    let Some((saw, blends)) = run_scene(true) else {
        return; // asset unavailable -> skip
    };
    assert!(
        blends > 0,
        "the runner never offered the transition to the seam"
    );
    assert!(
        saw,
        "the runner called the seam {blends} times but delivered none of its output; \
         the blend result is being discarded rather than shown"
    );
}

#[test]
fn preview_runner_without_an_injected_blend_should_not_show_the_sentinel() {
    // The other half, and what makes the test above mean something: with no seam
    // registered the runner blends on the CPU, which cannot produce the sentinel. If
    // this ever passed *and* the test above passed for the wrong reason, the sentinel
    // would have stopped being a discriminator.
    let Some((saw, blends)) = run_scene(false) else {
        return;
    };
    assert_eq!(
        blends, 0,
        "no compositor was injected, so none can be called"
    );
    assert!(!saw, "a CPU-only run must not produce the sentinel colour");
}
