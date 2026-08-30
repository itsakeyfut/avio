//! End-to-end export on both routes (Br4, #1627): the GPU export path (composite
//! -> readback -> existing encoder) and the force-CPU `MultiTrackComposer` path must
//! each produce a decodable, canvas-sized video from the same single-clip timeline.
//!
//! Probe-gated (RK-002): the source encode needs `FFmpeg` codecs and the GPU leg needs
//! an adapter; both are skipped gracefully when unavailable so the suite is green on
//! a headless / minimal-`FFmpeg` CI. Only environment-unavailable errors are skipped;
//! a structural failure (e.g. `TimelineRenderFailed`) fails the test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::sync::atomic::{AtomicU32, Ordering};

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoCodec};
use fixtures::{FileGuard, make_source_file, test_output_path};

const SRC_FRAMES: usize = 15;
const CANVAS: u32 = 64;

/// Whether a render error means "this environment can't run the pipeline" (skip) as
/// opposed to a real regression (fail). Mirrors the `timeline_tests` convention: a
/// filter/encode/decode build failure on minimal-FFmpeg CI is a skip; anything else
/// (notably `TimelineRenderFailed` / `Cancelled`) is a genuine failure.
fn is_environment_unavailable(e: &TimelineError) -> bool {
    matches!(
        e,
        TimelineError::Filter(_) | TimelineError::Encode(_) | TimelineError::Decode(_)
    )
}

/// `(decoded frame count, dimensions of the first frame)` for an exported file, or
/// `(0, None)` when it cannot be opened/decoded.
fn decode_stats(path: &std::path::Path) -> (usize, Option<(u32, u32)>) {
    let Ok(mut d) = VideoDecoder::open(path).build() else {
        return (0, None);
    };
    let mut n = 0usize;
    let mut dims = None;
    while let Ok(Some(f)) = d.decode_one() {
        if dims.is_none() {
            dims = Some((f.width(), f.height()));
        }
        n += 1;
    }
    (n, dims)
}

/// Asserts an exported file is a valid ~`SRC_FRAMES`-frame, canvas-sized video. A
/// silently truncated export (e.g. an early loop break) or a wrong-sized frame fails
/// here rather than passing a bare "non-empty" check.
fn assert_valid_export(path: &std::path::Path, route: &str) {
    let (count, dims) = decode_stats(path);
    assert!(
        (SRC_FRAMES - 1..=SRC_FRAMES + 1).contains(&count),
        "{route} export should decode ~{SRC_FRAMES} frames, got {count}"
    );
    assert_eq!(
        dims,
        Some((CANVAS, CANVAS)),
        "{route} export frames should be the {CANVAS}x{CANVAS} canvas size"
    );
}

fn export_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(800_000))
        .build()
}

/// A square, single hard-cut, file-source, unity-speed timeline: the shape the GPU
/// export path handles (aspect matches the canvas, identity transform).
fn build_timeline(src: &std::path::Path) -> Option<Timeline> {
    Timeline::builder()
        .canvas(CANVAS, CANVAS)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(src)])
        .build()
        .ok()
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

#[test]
fn export_should_produce_frames_on_cpu_and_gpu_routes() {
    let src = test_output_path("gpuexport_src.mp4");
    let _gs = FileGuard::new(src.clone());
    if make_source_file(&src, CANVAS, CANVAS, 30.0, SRC_FRAMES, 120, 90, 160).is_none() {
        return; // encoder unavailable -> skip
    }

    // CPU route: force-CPU always uses the MultiTrackComposer path (compiles and runs
    // without the `gpu` feature).
    let out_cpu = test_output_path("gpuexport_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_timeline) = build_timeline(&src) else {
        return; // source codec unavailable -> skip
    };
    if !render_or_skip(cpu_timeline.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    assert_valid_export(&out_cpu, "force-CPU");

    // GPU route: the default `render` composites the eligible timeline on the GPU when
    // an adapter is present. Skip when there is no adapter.
    #[cfg(feature = "gpu")]
    {
        if avio::GpuCompositor::new().is_none() {
            return; // no GPU adapter -> the GPU leg is unreachable here
        }
        let out_gpu = test_output_path("gpuexport_gpu.mp4");
        let _gg = FileGuard::new(out_gpu.clone());
        let Some(gpu_timeline) = build_timeline(&src) else {
            return;
        };
        if !render_or_skip(gpu_timeline.render(&out_gpu, export_config())) {
            return;
        }
        assert_valid_export(&out_gpu, "GPU");
    }
}

#[test]
fn render_with_progress_forcing_cpu_should_report_progress_and_export() {
    let src = test_output_path("gpuexport_progress_src.mp4");
    let _gs = FileGuard::new(src.clone());
    if make_source_file(&src, CANVAS, CANVAS, 30.0, SRC_FRAMES, 60, 140, 200).is_none() {
        return;
    }
    let out = test_output_path("gpuexport_progress_cpu.mp4");
    let _go = FileGuard::new(out.clone());
    let Some(timeline) = build_timeline(&src) else {
        return;
    };

    // on_progress is Fn (not FnMut), so count through an atomic.
    let frames = AtomicU32::new(0);
    let result = timeline.render_with_progress_forcing_cpu(&out, export_config(), |_p| {
        frames.fetch_add(1, Ordering::Relaxed);
        true
    });
    if !render_or_skip(result) {
        return;
    }
    assert!(
        frames.load(Ordering::Relaxed) >= 1,
        "the force-CPU progress callback must fire at least once"
    );
    assert_valid_export(&out, "force-CPU (progress)");
}
