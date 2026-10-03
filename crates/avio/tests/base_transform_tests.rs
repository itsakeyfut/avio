//! A base layer's placement renders the same on every route (#1766, ADR-0016).
//!
//! The four routes a timeline is rendered by are the CPU export (`render_forcing_cpu`),
//! the GPU export (`render`, with an adapter), the CPU preview (`open_forcing_cpu`) and
//! the GPU preview (`open`). Before ADR-0016 only the first placed a lone clip's `x` /
//! `y` / `scale`, and only the first left a clip smaller than the canvas at its native
//! size; the other three ignored the placement and letterboxed the clip. So the same
//! timeline rendered one way headless and another with an adapter, and one way in
//! export and another in preview. This pins all four to one lit box per fixture.
//!
//! Probe-gated on the environment: the fixture encode, the decoders and the adapter may
//! each be unavailable, and a route that cannot run is skipped with its reason printed.
//! A route that runs but lands elsewhere fails.

#![cfg(feature = "preview")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use avio::{
    Clip, EncoderConfig, Pacing, PixelFormat, PlayerHandle, Timeline, TimelineBuilder,
    TimelineError, TimelinePlayer,
};
use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoCodec};
use ff_format::Rational;
use ff_preview::FrameSink;
use fixtures::{FileGuard, make_source_file, test_output_path};

const CANVAS: u32 = 64;
const FPS: Rational = Rational::new(30, 1);
const FRAMES: usize = 10;
/// The frame inspected on every route: past any encoder warm-up, inside the clip.
const SAMPLE: usize = 3;

// Each route set builds two GPU adapters, and adapters built concurrently from several
// test threads can livelock the binary (#1718). The test runner serialises suites it
// recognises as GPU-building by their call sites; this one builds them through the
// engine's facade, so it serialises itself.
static ROUTES: Mutex<()> = Mutex::new(());

type Box4 = (u32, u32, u32, u32);
/// What a route reports for its sample frame: the frame's size and its lit box. The
/// size matters as much as the box: a preview that composites at the base's own size
/// instead of the canvas lands the picture on the same box in a smaller frame.
type Sample = (u32, u32, Option<Box4>);

/// The lit box of an rgba buffer as `(x0, y0, x1, y1)` inclusive: any channel above
/// 128. The fixtures are bright on black, so this reads the placement directly.
fn lit_bbox(rgba: &[u8], w: u32, h: u32) -> Option<Box4> {
    let mut b: Option<Box4> = None;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if rgba[i] > 128 || rgba[i + 1] > 128 || rgba[i + 2] > 128 {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

fn export_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(800_000))
        .build()
}

fn is_environment_unavailable(e: &TimelineError) -> bool {
    matches!(
        e,
        TimelineError::Filter(_) | TimelineError::Encode(_) | TimelineError::Decode(_)
    )
}

/// The sample of frame `sample` of an exported file, `Err(reason)` when the decoder
/// cannot run (skip).
fn decoded_sample(path: &Path, sample: usize) -> Result<Sample, String> {
    let mut dec = VideoDecoder::open(path)
        .output_format(PixelFormat::Rgba)
        .build()
        .map_err(|e| format!("decoder unavailable: {e}"))?;
    let mut n = 0;
    while let Ok(Some(f)) = dec.decode_one() {
        n += 1;
        if n == sample {
            let rgba = f.to_rgba().expect("rgba was requested");
            return Ok((
                f.width(),
                f.height(),
                lit_bbox(&rgba, f.width(), f.height()),
            ));
        }
    }
    Err(format!("the export decoded to only {n} frames"))
}

/// Exports `timeline` on one route and returns its sample frame, or `Err(reason)` when
/// the environment cannot run that route (skip).
fn export_sample(
    timeline: &Timeline,
    out: &Path,
    cpu: bool,
    sample: usize,
) -> Result<Sample, String> {
    let t = timeline.clone();
    let result = if cpu {
        t.render_forcing_cpu(out, export_config())
    } else {
        t.render(out, export_config())
    };
    match result {
        Ok(()) => decoded_sample(out, sample),
        Err(e) if is_environment_unavailable(&e) => Err(format!("export unavailable: {e}")),
        Err(e) => panic!("unexpected export error: {e}"),
    }
}

struct SampleSink {
    out: Arc<Mutex<Option<Sample>>>,
    handle: PlayerHandle,
    seen: usize,
    sample: usize,
}

impl FrameSink for SampleSink {
    fn push_frame(&mut self, rgba: &[u8], w: u32, h: u32, _pts: Duration) {
        self.seen += 1;
        if self.seen == self.sample {
            *self.out.lock().unwrap() = Some((w, h, lit_bbox(rgba, w, h)));
            self.handle.stop();
        }
    }
}

/// Plays `timeline` on one preview route, unpaced, and returns its sample frame, or
/// `Err(reason)` when the environment cannot run that route (skip).
fn preview_sample(timeline: &Timeline, cpu: bool, sample: usize) -> Result<Sample, String> {
    let opened = if cpu {
        TimelinePlayer::open_forcing_cpu(timeline)
    } else {
        TimelinePlayer::open(timeline)
    };
    let (mut runner, handle) = opened.map_err(|e| format!("preview unavailable: {e}"))?;
    let out = Arc::new(Mutex::new(None));
    runner.set_pacing(Pacing::Unpaced);
    runner.set_sink(Box::new(SampleSink {
        out: Arc::clone(&out),
        handle,
        seen: 0,
        sample,
    }));
    let _ = runner.run();
    let sampled = *out.lock().unwrap();
    sampled.ok_or_else(|| "the preview delivered fewer frames than the sample".to_string())
}

/// Renders `timeline` on all four routes and asserts each one that could run delivers a
/// canvas-sized frame whose lit box is `want`. At least one route must have run, or the
/// test is vacuous.
fn assert_all_routes(label: &str, timeline: &Timeline, sample: usize, want: Option<Box4>) {
    let _routes = ROUTES.lock().unwrap_or_else(|e| e.into_inner());
    let cpu_out = test_output_path(&format!("base_transform_{label}_cpu.mp4"));
    let gpu_out = test_output_path(&format!("base_transform_{label}_gpu.mp4"));
    let _g1 = FileGuard::new(cpu_out.clone());
    let _g2 = FileGuard::new(gpu_out.clone());
    let want = (timeline.canvas_width(), timeline.canvas_height(), want);
    let routes = [
        (
            "export cpu",
            export_sample(timeline, &cpu_out, true, sample),
        ),
        (
            "export gpu",
            export_sample(timeline, &gpu_out, false, sample),
        ),
        ("preview cpu", preview_sample(timeline, true, sample)),
        ("preview gpu", preview_sample(timeline, false, sample)),
    ];
    let mut ran = 0;
    for (route, got) in routes {
        match got {
            Ok(got) => {
                ran += 1;
                println!("{label}: {route} -> {got:?}");
                assert_eq!(
                    got, want,
                    "{label}: {route} must deliver a canvas-sized frame with the base placed \
                     like every route (width, height, lit box)"
                );
            }
            Err(reason) => println!("{label}: {route} skipped: {reason}"),
        }
    }
    // The route helpers panic on every error that is not the environment's, so a run
    // where no route could start is CI's FFmpeg (no decoders, no filters), not a broken
    // fixture: skip loudly rather than fail.
    if ran == 0 {
        println!("Skipping {label}: no route could run in this environment");
    }
}

/// A bright `w`x`h` source, or `None` when this environment cannot encode (skip).
fn source(label: &str, w: u32, h: u32) -> Option<(std::path::PathBuf, FileGuard)> {
    let path = test_output_path(&format!("base_transform_{label}_src.mp4"));
    let guard = FileGuard::new(path.clone());
    make_source_file(&path, w, h, FPS.as_f64(), FRAMES, 235, 128, 128)?;
    Some((path, guard))
}

/// The built timeline, or `None` (skip) when the environment cannot probe the fixture.
fn build_or_skip(label: &str, b: TimelineBuilder) -> Option<Timeline> {
    match b.build() {
        Ok(t) => Some(t),
        Err(e) if is_environment_unavailable(&e) => {
            println!("Skipping {label}: timeline build unavailable: {e}");
            None
        }
        Err(e) => panic!("{label}: timeline build failed: {e}"),
    }
}

fn one_clip(label: &str, canvas: Option<(u32, u32)>, clip: Clip) -> Option<Timeline> {
    let mut b = Timeline::builder().frame_rate(FPS).video_track(vec![clip]);
    if let Some((w, h)) = canvas {
        b = b.canvas(w, h);
    }
    build_or_skip(label, b)
}

/// One test for the four lit-box fixtures rather than four: each fixture opens a GPU
/// context on two of its routes, and four such tests in parallel is the shape that
/// livelocks a test binary (#1718). The label names the fixture in every message.
#[test]
fn a_base_layer_should_land_on_the_same_box_on_every_route() {
    // (label, source size, explicit canvas, clip, expected lit box)
    let cases: [(&str, (u32, u32), bool, fn(&Path) -> Clip, Box4); 4] = [
        // A positioned, scaled canvas-sized base.
        (
            "placed",
            (CANVAS, CANVAS),
            true,
            |p| Clip::new(p).with_position(10.0, 4.0).with_scale(0.5),
            (10, 4, 41, 35),
        ),
        // No implicit fit: a 64x32 clip on a 64x64 canvas is the top half, not a
        // letterboxed band in the middle. `FitMode::Fit` is the opt-in for a band.
        (
            "native",
            (CANVAS, CANVAS / 2),
            true,
            |p| Clip::new(p),
            (0, 0, 63, 31),
        ),
        // The export rule for the multiplier: a 64x32 clip scaled 0.5 is 32x32
        // (canvas * scale), placed at (10, 4).
        (
            "smaller_placed",
            (CANVAS, CANVAS / 2),
            true,
            |p| Clip::new(p).with_position(10.0, 4.0).with_scale(0.5),
            (10, 4, 41, 35),
        ),
        // No explicit canvas: the timeline probes 64x64 from the clip, and that canvas
        // is what every route places on. The preview used to derive its own from the
        // base frame and ignore the placement.
        (
            "implicit",
            (CANVAS, CANVAS),
            false,
            |p| Clip::new(p).with_position(10.0, 4.0).with_scale(0.5),
            (10, 4, 41, 35),
        ),
    ];
    for (label, (sw, sh), explicit, clip, want) in cases {
        let Some((src, _g)) = source(label, sw, sh) else {
            return;
        };
        let canvas = explicit.then_some((CANVAS, CANVAS));
        let Some(timeline) = one_clip(label, canvas, clip(&src)) else {
            return;
        };
        assert_eq!(
            timeline.explicit_canvas().is_some(),
            explicit,
            "{label}: the fixture's canvas mode"
        );
        assert_all_routes(label, &timeline, SAMPLE, Some(want));
    }
}

#[test]
fn a_second_clip_smaller_than_the_implicit_canvas_should_sit_on_it_on_every_route() {
    // The implicit canvas is probed from the first clip (64x64). The second clip is
    // 64x32, so on that clip the routes only agree if every one of them composites on the
    // probed canvas: a preview that took the base's own size as its canvas would deliver
    // a 64x32 frame instead of the top half of a 64x64 one. The first clip is five frames
    // long, so frame eight is inside the second.
    let Some((first, _g1)) = source("implicit_first", CANVAS, CANVAS) else {
        return;
    };
    let Some((second, _g2)) = source("implicit_second", CANVAS, CANVAS / 2) else {
        return;
    };
    let first_len = Duration::from_secs_f64(5.0 / FPS.as_f64());
    let builder = Timeline::builder().frame_rate(FPS).video_track(vec![
        Clip::new(&first).trim(Duration::ZERO, first_len),
        Clip::new(&second).offset(first_len),
    ]);
    let Some(timeline) = build_or_skip("implicit_second", builder) else {
        return;
    };
    assert_eq!(
        timeline.explicit_canvas(),
        None,
        "the fixture must keep the canvas implicit"
    );
    assert_eq!(
        (timeline.canvas_width(), timeline.canvas_height()),
        (CANVAS, CANVAS)
    );
    assert_all_routes("implicit_second", &timeline, 8, Some((0, 0, 63, 31)));
}

#[test]
fn a_rotated_base_should_render_the_same_on_every_route() {
    // A 64x64 clip rotated 45 degrees keeps its full bounding box (the rotated square's
    // diagonal exceeds the frame) but exposes black corners. The GPU routes decline
    // rotation and the CPU renders it, so the four must still agree; the corner is what
    // tells a rendered rotation from an ignored one.
    let Some((src, _g)) = source("rotated", CANVAS, CANVAS) else {
        return;
    };
    let Some(timeline) = one_clip(
        "rotated",
        Some((CANVAS, CANVAS)),
        Clip::new(&src).with_rotation(45.0),
    ) else {
        return;
    };
    let cpu_out = test_output_path("base_transform_rotated_cpu.mp4");
    let _g1 = FileGuard::new(cpu_out.clone());
    let corner = |rgba: &[u8], w: u32| u32::from(rgba[0]) + u32::from(rgba[((w - 1) * 4) as usize]);
    // The CPU export is the reference: decode its sample frame's corners.
    let reference = {
        let t = timeline.clone();
        match t.render_forcing_cpu(&cpu_out, export_config()) {
            Ok(()) => {
                let mut dec = match VideoDecoder::open(&cpu_out)
                    .output_format(PixelFormat::Rgba)
                    .build()
                {
                    Ok(d) => d,
                    Err(e) => {
                        println!("rotated: decoder unavailable: {e}");
                        return;
                    }
                };
                let mut n = 0;
                let mut c = None;
                while let Ok(Some(f)) = dec.decode_one() {
                    n += 1;
                    if n == SAMPLE {
                        c = Some(corner(&f.to_rgba().unwrap(), f.width()));
                        break;
                    }
                }
                c
            }
            Err(e) if is_environment_unavailable(&e) => {
                println!("rotated: export unavailable: {e}");
                return;
            }
            Err(e) => panic!("unexpected export error: {e}"),
        }
    };
    let Some(reference) = reference else {
        println!("rotated: the export decoded to too few frames");
        return;
    };
    assert!(
        reference < 32,
        "the reference corners must be black for a rendered rotation, got {reference}"
    );
    // Both preview routes must show the same black corners.
    for cpu in [true, false] {
        let opened = if cpu {
            TimelinePlayer::open_forcing_cpu(&timeline)
        } else {
            TimelinePlayer::open(&timeline)
        };
        let (mut runner, handle) = match opened {
            Ok(p) => p,
            Err(e) => {
                println!("rotated: preview unavailable: {e}");
                continue;
            }
        };
        let out = Arc::new(Mutex::new(None));
        runner.set_pacing(Pacing::Unpaced);
        runner.set_sink(Box::new(CornerSink {
            out: Arc::clone(&out),
            handle,
            seen: 0,
        }));
        let _ = runner.run();
        let got = out.lock().unwrap().unwrap_or(u32::MAX);
        assert!(
            got < 32,
            "preview {} must render the rotation's black corners like the CPU export, got {got}",
            if cpu { "cpu" } else { "gpu" }
        );
    }
}

struct CornerSink {
    out: Arc<Mutex<Option<u32>>>,
    handle: PlayerHandle,
    seen: usize,
}

impl FrameSink for CornerSink {
    fn push_frame(&mut self, rgba: &[u8], w: u32, _h: u32, _pts: Duration) {
        self.seen += 1;
        if self.seen == SAMPLE {
            let corner = u32::from(rgba[0]) + u32::from(rgba[((w - 1) * 4) as usize]);
            *self.out.lock().unwrap() = Some(corner);
            self.handle.stop();
        }
    }
}
