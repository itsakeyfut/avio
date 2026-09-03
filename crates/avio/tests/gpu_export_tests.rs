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

/// #1660: a source whose native rate differs from the timeline rate used to force the
/// CPU path; the GPU drain now conforms it. The export must keep the clip's on-screen
/// duration — i.e. the output carries the *timeline's* frame count, not the source's.
#[cfg(feature = "gpu")]
#[test]
fn gpu_export_should_conform_a_slower_source_to_the_timeline_rate() {
    const SRC_FPS: f64 = 24.0;
    const OUT_FPS: f64 = 30.0;
    const SRC_24_FRAMES: usize = 24; // ~1 s of 24 fps source

    let src = test_output_path("gpuexport_24fps_src.mp4");
    let _gs = FileGuard::new(src.clone());
    if make_source_file(&src, CANVAS, CANVAS, SRC_FPS, SRC_24_FRAMES, 120, 90, 160).is_none() {
        return; // encoder unavailable -> skip
    }
    if avio::GpuCompositor::new().is_none() {
        return; // no GPU adapter -> the GPU leg is unreachable here
    }
    // Expect against what the source *actually decodes*, not the nominal frame count:
    // an encode/decode round trip can lose a frame, and that shortfall would otherwise
    // read as a conform bug. The conformed output should cover the same wall-clock span
    // at the timeline rate, i.e. `src_frames * OUT_FPS / SRC_FPS`.
    let (src_frames, _) = decode_stats(&src);
    if src_frames == 0 {
        return; // source decoder unavailable -> skip
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    let expected = (src_frames as f64 * OUT_FPS / SRC_FPS).round() as usize;
    let Some(timeline) = Timeline::builder()
        .canvas(CANVAS, CANVAS)
        .frame_rate(OUT_FPS)
        .video_track(vec![Clip::new(&src)])
        .build()
        .ok()
    else {
        return; // source codec unavailable -> skip
    };
    let out = test_output_path("gpuexport_24to30.mp4");
    let _gg = FileGuard::new(out.clone());
    if !render_or_skip(timeline.render(&out, export_config())) {
        return;
    }
    let (count, dims) = decode_stats(&out);
    assert_eq!(
        dims,
        Some((CANVAS, CANVAS)),
        "conformed export frames should be the {CANVAS}x{CANVAS} canvas size"
    );
    // The load-bearing assertion: up-conform must *add* frames. Without it the drain
    // took one source frame per output and the clip came out at the source's own count,
    // far below this.
    assert!(
        count > src_frames,
        "conform must add frames: {src_frames} source frames became {count} outputs"
    );
    // The count lands near the conformed duration, but not exactly: n frames span n-1
    // intervals, so the file is 23/24 s rather than 1 s, and PTS quantisation and frame
    // reordering move the last output's boundary by a frame between platforms. The
    // window is wide enough to absorb that and still far from the un-conformed count.
    assert!(
        (expected - 2..=expected + 2).contains(&count),
        "a {SRC_FPS} fps source ({src_frames} frames) in a {OUT_FPS} fps timeline should \
         export ~{expected} frames, got {count}"
    );
}

/// Per-frame mean absolute RGB difference between the two routes' exports.
///
/// Calibrated (#1659, widened in #1732): this pipeline's floor is a **hard cut**, where
/// the routes still differ because the GPU one round-trips yuv -> rgba -> yuv while the
/// CPU one stays in yuv throughout the filter graph. Measured on the structured sources
/// below, a hard cut comes out at mean 1.4 / max 7, and the transitions the export
/// renders land at 1.9-2.3. The bound clears those with room for platform variation and
/// is still far from anything a real divergence would produce: the wrong blend direction reads as mean ~127, and a
/// transition rendered as the wrong kind as ~50.
const TOL_TRANSITION_MEAN: f64 = 6.0;

/// Encodes a spatially structured, colourful source: a horizontal ramp in R, a vertical
/// one in G, and `phase` shifting B so the two clips differ everywhere.
///
/// Deliberately not `make_source_file`'s flat fill. On a solid colour a fade, a wipe and
/// a dissolve produce near-identical frames, so a flat fixture cannot tell a correct
/// blend from a mirrored or mis-keyed one (RK-022).
fn make_structured_source(path: &std::path::Path, frames: usize, phase: u8) -> Option<()> {
    use ff_encode::VideoEncoder;
    use ff_format::VideoFrame;

    let mut enc = VideoEncoder::create(path)
        .video(CANVAS, CANVAS, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .build()
        .ok()?;
    for _ in 0..frames {
        let mut rgba = vec![0u8; (CANVAS * CANVAS * 4) as usize];
        for y in 0..CANVAS {
            for x in 0..CANVAS {
                let o = ((y * CANVAS + x) * 4) as usize;
                rgba[o] = u8::try_from(x * 255 / CANVAS)
                    .unwrap_or(255)
                    .wrapping_add(phase);
                rgba[o + 1] = u8::try_from(y * 255 / CANVAS).unwrap_or(255);
                rgba[o + 2] = 128u8.wrapping_sub(phase);
                rgba[o + 3] = 255;
            }
        }
        enc.push_video(&VideoFrame::from_rgba(CANVAS, CANVAS, rgba).ok()?)
            .ok()?;
    }
    enc.finish().ok()?;
    Some(())
}

/// Every decoded frame of `path` as rgba.
fn decode_rgba(path: &std::path::Path) -> Vec<Vec<u8>> {
    let Ok(mut d) = VideoDecoder::open(path)
        .output_format(ff_format::PixelFormat::Rgba)
        .build()
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(f)) = d.decode_one() {
        if let Some(plane) = f.plane(0) {
            out.push(plane.to_vec());
        }
    }
    out
}

/// Mean absolute difference over the RGB channels (alpha is meaningless here).
fn mean_abs_diff_rgb(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()) / 4;
    if n == 0 {
        return f64::MAX;
    }
    let mut sum = 0f64;
    for i in 0..n {
        for c in 0..3 {
            sum += f64::from(a[i * 4 + c].abs_diff(b[i * 4 + c]));
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let denom = (n * 3) as f64;
    sum / denom
}

/// #1659 / #1732: a transition into the track's last clip exports on the GPU and lands
/// on the CPU export's pixels, for every kind the export renders.
///
/// **Why the frame count is the load-bearing assertion.** The CPU route's `xfade`
/// overlaps the two clips, so the track comes out `transition` shorter: two 1 s clips at
/// 30 fps with a 0.5 s fade give 45 frames, not the hard cut's 60. An export that
/// silently dropped the transition would produce 60 and fail here, which no pixel
/// tolerance could catch on its own.
///
/// **Why this test does not prove the route on its own.** `render()` falls back to CPU
/// without saying so, so if the timeline were ineligible both legs here would be CPU
/// exports and agree perfectly -- the false green that made #1660's export test exercise
/// the CPU path for months. What closes it is
/// `gpu_export::tests::eligible_track_should_accept_a_fade_into_the_last_clip`, which
/// asserts this exact shape is eligible; with an adapter present (checked below) those
/// two facts leave no other route.
///
/// The kinds beyond `Fade` are only here because #1732 brought each node onto FFmpeg's
/// own formula. Before that the GPU route declined them, and this loop would have been
/// comparing two CPU exports.
#[cfg(feature = "gpu")]
#[test]
fn gpu_export_should_match_the_cpu_export_for_every_rendered_transition() {
    use std::time::Duration;

    use ff_filter::XfadeTransition;

    const CLIP_FRAMES: usize = 30; // 1 s at 30 fps
    const WINDOW: usize = 15; // 0.5 s at 30 fps
    // The blend reads the outgoing clip's handle -- its frames past the out-point
    // (ADR-0009) -- so the source has to hold more than the clip trims out of it.
    const SOURCE_FRAMES: usize = CLIP_FRAMES + 2 * WINDOW;

    let a = test_output_path("gpuexport_tr_a.mp4");
    let b = test_output_path("gpuexport_tr_b.mp4");
    let _ga = FileGuard::new(a.clone());
    let _gb = FileGuard::new(b.clone());
    if make_structured_source(&a, SOURCE_FRAMES, 0).is_none()
        || make_structured_source(&b, SOURCE_FRAMES, 100).is_none()
    {
        return; // encoder unavailable -> skip
    }
    if avio::GpuCompositor::new().is_none() {
        return; // no GPU adapter -> the GPU leg is unreachable here
    }

    // `Dissolve` is absent on purpose: the export declines it (its pixel set depends on
    // libm agreement between Rust and FFmpeg), so both legs here would be CPU renders and
    // the comparison would pass without exercising anything. The rejection itself is
    // asserted by `gpu_export::tests::export_maps_to_gpu_should_reject_dissolve_despite_it_mapping`.
    for kind in [
        XfadeTransition::Fade,
        XfadeTransition::WipeLeft,
        XfadeTransition::WipeRight,
        XfadeTransition::WipeUp,
        XfadeTransition::WipeDown,
        XfadeTransition::FadeBlack,
        XfadeTransition::FadeWhite,
    ] {
        let build = || {
            Timeline::builder()
                .canvas(CANVAS, CANVAS)
                .frame_rate(30.0)
                .video_track(vec![
                    Clip::new(&a).trim(Duration::ZERO, Duration::from_secs(1)),
                    Clip::new(&b)
                        .offset(Duration::from_secs(1))
                        .trim(Duration::ZERO, Duration::from_secs(1))
                        .with_transition(kind, Duration::from_millis(500)),
                ])
                .build()
                .ok()
        };
        let (Some(gpu_timeline), Some(cpu_timeline)) = (build(), build()) else {
            return; // source codec unavailable -> skip
        };

        let out_gpu = test_output_path("gpuexport_tr_gpu.mp4");
        let out_cpu = test_output_path("gpuexport_tr_cpu.mp4");
        let _gg = FileGuard::new(out_gpu.clone());
        let _gc = FileGuard::new(out_cpu.clone());
        if !render_or_skip(gpu_timeline.render(&out_gpu, export_config()))
            || !render_or_skip(cpu_timeline.render_forcing_cpu(&out_cpu, export_config()))
        {
            return;
        }

        let gpu = decode_rgba(&out_gpu);
        let cpu = decode_rgba(&out_cpu);
        if gpu.is_empty() || cpu.is_empty() {
            return; // decoder unavailable -> skip
        }

        // The transition preserves the timeline length (ADR-0009): both clips are
        // trimmed to a second, so the track runs for two whatever the transition does.
        // Its own length is what the two routes must agree on frame for frame below;
        // this is the guard that the drain did not silently drop or double the window.
        let expected = CLIP_FRAMES * 2;
        assert!(
            (expected - 1..=expected + 1).contains(&gpu.len()),
            "a {WINDOW}-frame transition must leave the {expected}-frame timeline its \
         length, got {}",
            gpu.len()
        );
        assert_eq!(
            gpu.len(),
            cpu.len(),
            "both routes must export the same number of frames"
        );

        let worst = gpu
            .iter()
            .zip(cpu.iter())
            .enumerate()
            .map(|(i, (g, c))| (mean_abs_diff_rgb(g, c), i))
            .fold((0f64, 0usize), |acc, x| if x.0 > acc.0 { x } else { acc });
        println!("{kind:?}: worst frame {} mean={:.3}", worst.1, worst.0);
        assert!(
            worst.0 <= TOL_TRANSITION_MEAN,
            "{kind:?}: GPU and CPU exports diverged at frame {}: mean={:.3} \
         (tolerance {TOL_TRANSITION_MEAN})",
            worst.1,
            worst.0
        );
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
