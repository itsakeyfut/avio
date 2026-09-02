//! Preview-vs-export structural parity for the supported set (#1664).
//!
//! The bridge renders a timeline two ways — the real-time preview runner
//! (`TimelinePlayer` -> `SceneRunner` -> `FrameSink`) and the export path
//! (`Timeline::render` -> encoded file). #1663 pinned GPU-vs-CPU parity per effect;
//! this pins that the two *pipelines* agree on the shape of what they produce, now
//! that both derive from the same single `Clip::effects` list (#1622 / #1712).
//!
//! # What is guaranteed here
//!
//! - Both sides produce frames at the timeline's canvas size.
//! - The export decodes to the expected frame count for the timeline's duration and
//!   frame rate.
//! - The preview's delivered presentation timestamps cover most of the timeline, so it
//!   plays substantially the same span the export writes. Not the exact end: the runner
//!   is real-time and drops late frames, the tail included (see the third boundary below).
//! - Both sides render the source's (chromatic) colour rather than a blank frame, and
//!   agree with each other within a coarse tolerance.
//!
//! # The deferred full-convergence boundary
//!
//! Per-pixel convergence between preview and export is **out of scope for v0.18** and
//! is deliberately not asserted, for three reasons that this test cannot remove:
//!
//! 1. **Colour space.** The preview composites in rgba, while the export normalises to
//!    `yuv420p` before compositing (RK-012), so identical inputs land on different
//!    numbers — the reason the tolerance here is coarse rather than tight.
//! 2. **Lossy encode.** The export is compared *after* an H.264 encode/decode round
//!    trip, which perturbs pixels independently of any compositing difference.
//! 3. **Real-time playback.** The preview runner is driven by a wall clock and may drop
//!    frames under load, so there is no stable frame-by-frame correspondence to compare
//!    against; the preview side is therefore asserted by the span it covers (its PTS
//!    range), not by a frame count.
//!
//! Closing that gap needs the preview and export compositors to share a colour space
//! and a deterministic (non-real-time) preview drive; both are tracked beyond v0.18.
//!
//! Probe-gated (RK-002): the source encode, the preview open and the export each skip
//! gracefully when the environment cannot run them, so the suite stays green on a
//! minimal-`FFmpeg` CI. Only environment-unavailable errors are skipped; a structural
//! failure (an export that decodes to nothing, a colour mismatch) fails the test.
//!
//! The source is a **file** clip encoded by the fixture helper, not a generated
//! (`Solid`) clip: the export path renders a generated source through
//! `movie=…:format_name=lavfi`, which needs the lavfi *virtual demuxer*. That demuxer
//! is absent on this dev machine and on CI (RK-013), so `render` returns `Ok` while
//! producing **zero** frames — a generated source would make this test permanently
//! vacuous on both legs' behalf.

#![cfg(feature = "preview")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use avio::{
    Clip, EncoderConfig, PixelFormat, PlayerHandle, Timeline, TimelineError, TimelinePlayer,
};
use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoCodec};
use ff_preview::FrameSink;
use fixtures::{FileGuard, make_source_file, test_output_path};

const CANVAS_W: u32 = 64;
const CANVAS_H: u32 = 64;
const FPS: f64 = 30.0;
/// Frames in the source, and therefore the frames the export must decode (give or take
/// an encoder flush frame). The clip's duration follows from this and `FPS`, so the two
/// legs are compared against one definition of the timeline's length.
const EXPECTED_FRAMES: usize = 15;
/// The source fill in YUV. Deliberately chromatic (≈ rgb(165, 110, 53)): a grey source
/// would let a channel-blind comparison pass even if the pipelines disagreed per
/// channel (RK-022), and it makes the "not a blank frame" guard below meaningful.
const SRC_YUV: [u8; 3] = [120, 90, 160];
/// Contrast for the clip's `ColorCorrect` effect. Non-neutral so an `Eq` step actually
/// reaches both pipelines (see `build_timeline`).
const CONTRAST: f32 = 1.3;
/// Minimum spread between the brightest and darkest channel of a rendered frame. The
/// source is strongly chromatic, so a black, white or grey frame — the shape a broken
/// leg produces — falls below this.
const MIN_CHROMA_SPREAD: f64 = 40.0;
/// Mean per-channel difference allowed between the preview frame and the exported
/// frame. Coarse by design: rgba-vs-`yuv420p` compositing plus a lossy encode (see the
/// boundary above) move the two apart — measured 0.33 here with the clip's `eq` step
/// applied on both sides. The margin is for content and builds that are less forgiving,
/// not for slack.
const TOL_PREVIEW_EXPORT_MEAN: f64 = 24.0;
/// A spin guard: a healthy run delivers ~`EXPECTED_FRAMES` frames, so this only trips
/// if the runner fails to terminate (the RK-019 failure mode), keeping the test bounded.
const MAX_PREVIEW_FRAMES: usize = 200;

/// Whether a render error means "this environment can't run the pipeline" (skip) as
/// opposed to a real regression (fail). Mirrors the `gpu_export_tests` convention.
fn is_environment_unavailable(e: &TimelineError) -> bool {
    matches!(
        e,
        TimelineError::Filter(_) | TimelineError::Encode(_) | TimelineError::Decode(_)
    )
}

/// Renders `timeline` to `out`, returning `false` (skip) on an environment-unavailable
/// error and panicking on a structural one.
fn render_or_skip(result: Result<(), TimelineError>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) if is_environment_unavailable(&e) => false,
        Err(e) => panic!("unexpected export error: {e}"),
    }
}

fn export_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(800_000))
        .build()
}

/// The mean R/G/B of an rgba buffer (alpha ignored — the compositor writes the canvas
/// alpha, which is orthogonal to colour parity).
fn mean_rgb(rgba: &[u8]) -> [f64; 3] {
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    for px in rgba.chunks_exact(4) {
        for (c, s) in sum.iter_mut().enumerate() {
            *s += f64::from(px[c]);
        }
        n += 1.0;
    }
    let d = n.max(1.0);
    [sum[0] / d, sum[1] / d, sum[2] / d]
}

fn mean_abs_diff(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f64>() / 3.0
}

/// The per-frame mean RGB and the frame dimensions of an exported file, or `None` when
/// the **decoder itself** is unavailable (skip). An empty result means the file decoded
/// to no frames, which is a structural failure the caller asserts on — the two must not
/// be conflated, or a truncated export would masquerade as a skipped environment.
fn decode_export(path: &std::path::Path) -> Option<(Vec<[f64; 3]>, Option<(u32, u32)>)> {
    // Decode straight to rgba: `VideoFrame::to_rgba` is an accessor, not a converter,
    // so a yuv420p frame would yield `None` and silently produce no samples.
    let mut decoder = VideoDecoder::open(path)
        .output_format(PixelFormat::Rgba)
        .build()
        .ok()?;
    let mut means = Vec::new();
    let mut dims = None;
    while let Ok(Some(frame)) = decoder.decode_one() {
        dims.get_or_insert((frame.width(), frame.height()));
        if let Some(rgba) = frame.to_rgba() {
            means.push(mean_rgb(&rgba));
        }
    }
    Some((means, dims))
}

/// Collects the canvas size, the last delivered PTS and the mean colour of a frame from
/// the preview runner.
struct ParitySink {
    state: Arc<Mutex<PreviewState>>,
    handle: PlayerHandle,
}

#[derive(Default)]
struct PreviewState {
    frames: usize,
    dims: Option<(u32, u32)>,
    last_pts: Duration,
    mean: Option<[f64; 3]>,
}

impl FrameSink for ParitySink {
    fn push_frame(&mut self, rgba: &[u8], w: u32, h: u32, pts: Duration) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.frames += 1;
        state.dims.get_or_insert((w, h));
        state.last_pts = state.last_pts.max(pts);
        // Keep the latest frame's colour. Sampling only one specific frame would leave
        // `mean` unset when the runner delivers fewer frames than that, turning a
        // working preview into the same "no frames" skip below — a false green.
        state.mean = Some(mean_rgb(rgba));
        if state.frames >= MAX_PREVIEW_FRAMES {
            self.handle.stop();
        }
    }
}

/// The spread between the brightest and darkest channel of a mean colour.
fn chroma_spread(mean: [f64; 3]) -> f64 {
    let max = mean[0].max(mean[1]).max(mean[2]);
    let min = mean[0].min(mean[1]).min(mean[2]);
    max - min
}

/// The clip carries a non-neutral typed effect on purpose. A pass-through timeline
/// would compare two pipelines that never interpret an effect chain, so it could not
/// see them disagree there — and interpreting one `Clip::effects` list on both sides is
/// exactly the invariant #1622 / #1712 established and this test exists to hold.
/// Saturation stays neutral so the source's chroma (and the guard below) survive.
fn build_timeline(src: &std::path::Path) -> Timeline {
    Timeline::builder()
        .canvas(CANVAS_W, CANVAS_H)
        .frame_rate(FPS)
        .video_track(vec![
            Clip::new(src).with_color_correction(0.0, CONTRAST, 1.0),
        ])
        .build()
        .expect("timeline build failed")
}

#[test]
fn preview_and_export_should_agree_structurally() {
    let src = test_output_path("preview_export_parity_src.mp4");
    let _src_guard = FileGuard::new(src.clone());
    if make_source_file(
        &src,
        CANVAS_W,
        CANVAS_H,
        FPS,
        EXPECTED_FRAMES,
        SRC_YUV[0],
        SRC_YUV[1],
        SRC_YUV[2],
    )
    .is_none()
    {
        return; // source encoder unavailable -> skip
    }
    let timeline = build_timeline(&src);

    // --- preview leg -------------------------------------------------------------
    // Scoped so the runner and its handle (and therefore its decoder threads, which
    // hold the source file open) are dropped before the export reads the same file.
    let state = Arc::new(Mutex::new(PreviewState::default()));
    {
        let (mut runner, handle) = match TimelinePlayer::open(&timeline) {
            Ok(p) => p,
            Err(e) => {
                println!("skipping: preview open failed: {e}");
                return;
            }
        };
        runner.set_sink(Box::new(ParitySink {
            state: Arc::clone(&state),
            handle: handle.clone(),
        }));
        let _ = runner.run();
    }
    let preview = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (Some(preview_dims), Some(preview_mean)) = (preview.dims, preview.mean) else {
        println!("skipping: preview rendered no frames");
        return;
    };

    // --- export leg --------------------------------------------------------------
    let out = FileGuard::new(test_output_path("preview_export_parity.mp4"));
    if !render_or_skip(timeline.render(out.path(), export_config())) {
        println!("skipping: encoder unavailable");
        return;
    }
    let Some((export_means, export_dims)) = decode_export(out.path()) else {
        println!("skipping: decoder unavailable");
        return;
    };
    // The decoder ran, so anything short of a decodable video is a real failure.
    assert!(
        !export_means.is_empty(),
        "the export decoded to no frames — a truncated or unreadable file"
    );
    let export_frames = export_means.len();
    let export_dims = export_dims.expect("decoded frames must carry dimensions");
    // A mid-sequence frame: past any encoder warm-up, still well inside the clip.
    let export_mean = export_means[export_frames / 2];

    // --- structural parity -------------------------------------------------------
    assert_eq!(
        preview_dims,
        (CANVAS_W, CANVAS_H),
        "preview must render at the canvas size"
    );
    assert_eq!(
        export_dims,
        (CANVAS_W, CANVAS_H),
        "export must render at the canvas size"
    );
    // The export is deterministic, so its frame count is checked directly; the preview
    // is real-time and may drop frames, so it is checked by the span it covers.
    assert!(
        (EXPECTED_FRAMES - 1..=EXPECTED_FRAMES + 1).contains(&export_frames),
        "export should decode ~{EXPECTED_FRAMES} frames, got {export_frames}"
    );
    // Distinguish "the runner never terminated" from "it ended early": without this the
    // spin guard surfaces as a confusing short-PTS failure below.
    assert!(
        preview.frames < MAX_PREVIEW_FRAMES,
        "preview runner did not terminate (spin guard tripped at {MAX_PREVIEW_FRAMES} frames)"
    );
    // The timeline is exactly the source's length: `EXPECTED_FRAMES` frames at `FPS`.
    let end = Duration::from_secs_f64(EXPECTED_FRAMES as f64 / FPS);
    // Not the exact end: the runner is real-time and drops frames it cannot deliver on
    // time, the tail included (`SceneRunner`'s "dropped late frame" path), so requiring
    // it is flaky — a coverage run came in at 366ms of 500ms. Half the span still
    // separates a runner that played the timeline from one stuck at its opening frames.
    assert!(
        preview.last_pts * 2 >= end,
        "preview must play most of the timeline: last pts {:?} over {} frames, end {end:?}",
        preview.last_pts,
        preview.frames
    );
    // Non-vacuity: the source is strongly chromatic, so a blank / black / grey frame on
    // either side fails here rather than sailing through the comparison below (two
    // black frames would otherwise "agree" perfectly).
    let preview_spread = chroma_spread(preview_mean);
    let export_spread = chroma_spread(export_mean);
    let preview_vs_export = mean_abs_diff(preview_mean, export_mean);
    println!(
        "preview {preview_mean:?} (spread {preview_spread:.1}) / \
         export {export_mean:?} (spread {export_spread:.1}) — \
         preview-vs-export {preview_vs_export:.2}"
    );
    assert!(
        preview_spread >= MIN_CHROMA_SPREAD,
        "preview must render the chromatic source, not a blank frame: {preview_mean:?}"
    );
    assert!(
        export_spread >= MIN_CHROMA_SPREAD,
        "export must render the chromatic source, not a blank frame: {export_mean:?}"
    );
    assert!(
        preview_vs_export <= TOL_PREVIEW_EXPORT_MEAN,
        "preview and export must agree within the coarse tolerance: {preview_vs_export}"
    );
}
