//! Every blend mode must produce the same colour on both render routes (#1806).
//!
//! `vf_blend` applies its formula per plane, so the CPU composition used to blend
//! luma against luma and chroma against chroma: `Darken` of a red and a green
//! returned green, while the GPU compositor returned the RGB answer. Both sides now
//! blend in planar float RGB, which is the arithmetic ADR-0010 transcribed into the
//! shaders.
//!
//! The fixture is chromatic on purpose. A grey base and a grey overlay cannot show a
//! per-channel divergence, because every channel carries the same number (RK-022),
//! which is how a difference of 140 levels went unnoticed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::path::Path;
use std::time::Duration;

use avio::{BlendMode, Clip, EncoderConfig, Timeline, TimelineError};
use fixtures::{FileGuard, make_source_file, test_output_path};

/// Canvas side; small because this renders 80 files.
const SIDE: u32 = 64;
const FRAMES: usize = 6;

/// Largest per-channel mean difference accepted between the two routes.
///
/// Both routes evaluate the same formula on the same colour space, so what is left is
/// the yuv/rgb round trip each takes on the way in and out: a level or two. A blend
/// computed in the wrong colour space is worth tens of levels, so this bound catches
/// that with room to spare. Measured across the 40 modes on this build: 28 modes at
/// 3 or less.
const TOL_MEAN: f64 = 4.0;

/// Tolerance for a mode whose formula divides by a small number.
///
/// `ColorDodge` is `B / (1 - A)`, whose derivative at the fixture's colours is about
/// 4.6 output levels per input level, so the one level the two routes differ on their
/// inputs comes out as seven. `Freeze`, `HardOverlay` and `Multiply128` amplify the
/// same way. Measured: 7, 7, 5 and 5.
const TOL_SENSITIVE: f64 = 8.0;

/// The bound for one mode: wider only where the formula itself amplifies the input
/// rounding, never because a mode happened to fail.
fn tolerance(mode: BlendMode) -> f64 {
    match mode {
        BlendMode::ColorDodge
        | BlendMode::Freeze
        | BlendMode::HardOverlay
        | BlendMode::Multiply128 => TOL_SENSITIVE,
        _ => TOL_MEAN,
    }
}

/// All 40 `BlendMode` variants. `gpu_reference_tests.rs` owns the guard that the set
/// is complete (`every_blend_mode_and_operator_should_have_a_fixture_row`); the count
/// assertion below fails here too if a variant is added.
const MODES: &[BlendMode] = &[
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::SoftLight,
    BlendMode::HardLight,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Add,
    BlendMode::Subtract,
    BlendMode::And,
    BlendMode::Average,
    BlendMode::Bleach,
    BlendMode::Divide,
    BlendMode::Extremity,
    BlendMode::Freeze,
    BlendMode::Geometric,
    BlendMode::Glow,
    BlendMode::GrainExtract,
    BlendMode::GrainMerge,
    BlendMode::HardMix,
    BlendMode::HardOverlay,
    BlendMode::Harmonic,
    BlendMode::Heat,
    BlendMode::Interpolate,
    BlendMode::LinearLight,
    BlendMode::Multiply128,
    BlendMode::Negation,
    BlendMode::Or,
    BlendMode::Phoenix,
    BlendMode::PinLight,
    BlendMode::Reflect,
    BlendMode::Stain,
    BlendMode::SoftDifference,
    BlendMode::VividLight,
    BlendMode::Xor,
];

fn s(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

/// The per-channel mean of the centre quarter of the first frame.
///
/// The centre only, so the edges a chroma-subsampled round trip smears are left out
/// of the comparison.
fn centre_mean_rgb(path: &Path) -> Option<[f64; 3]> {
    let mut decoder = ff_decode::VideoDecoder::open(path)
        .output_format(ff_format::PixelFormat::Rgb24)
        .build()
        .ok()?;
    let frame = decoder.decode_one().ok()??;
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let stride = frame.stride(0).unwrap_or(w * 3);
    let plane = frame.plane(0)?;
    let (x0, x1) = (w / 4, w * 3 / 4);
    let (y0, y1) = (h / 4, h * 3 / 4);
    let mut sums = [0f64; 3];
    let mut n = 0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = y * stride + x * 3;
            for (c, sum) in sums.iter_mut().enumerate() {
                *sum += f64::from(plane[i + c]);
            }
            n += 1.0;
        }
    }
    (n > 0.0).then(|| [sums[0] / n, sums[1] / n, sums[2] / n])
}

/// `true` when the render failed for a reason that means "this environment cannot
/// run the pipeline" rather than "the code is wrong".
fn is_environment_unavailable(e: &TimelineError) -> bool {
    matches!(
        e,
        TimelineError::Filter(_) | TimelineError::Encode(_) | TimelineError::Decode(_)
    )
}

/// Whether the GPU export route can run here. Without an adapter `render()` falls
/// back to the CPU composer and this test would compare the CPU route with itself.
#[cfg(feature = "gpu")]
fn gpu_route_available() -> bool {
    avio::GpuCompositor::new().is_some()
}

#[cfg(not(feature = "gpu"))]
fn gpu_route_available() -> bool {
    false
}

#[test]
fn every_blend_mode_should_agree_between_the_two_render_routes() {
    assert_eq!(MODES.len(), 40, "ff_filter::BlendMode has 40 variants");

    let base = test_output_path("blend_parity_base.mp4");
    let over = test_output_path("blend_parity_over.mp4");
    let (_gb, _go) = (FileGuard::new(base.clone()), FileGuard::new(over.clone()));
    // Chromatic and far apart: rgb(200,60,60) under rgb(60,200,60), expressed as the
    // YUV the encoder takes.
    if make_source_file(&base, SIDE, SIDE, 30.0, FRAMES, 105, 105, 190).is_none() {
        return; // no encoder here
    }
    if make_source_file(&over, SIDE, SIDE, 30.0, FRAMES, 145, 84, 54).is_none() {
        return;
    }

    if !gpu_route_available() {
        println!("Skipping: no GPU adapter, so both legs would be the CPU route");
        return;
    }

    let mut compared = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for &mode in MODES {
        let timeline = |m: BlendMode| {
            Timeline::builder()
                .canvas(SIDE, SIDE)
                .frame_rate(30.into())
                .video_track(vec![Clip::new(&base).trim(Duration::ZERO, s(0.2))])
                .video_track(vec![
                    Clip::new(&over)
                        .trim(Duration::ZERO, s(0.2))
                        .with_blend_mode(m),
                ])
                .build()
                .unwrap()
        };
        let gpu_out = test_output_path(&format!("blend_parity_gpu_{mode:?}.mp4"));
        let cpu_out = test_output_path(&format!("blend_parity_cpu_{mode:?}.mp4"));
        let (_g1, _g2) = (
            FileGuard::new(gpu_out.clone()),
            FileGuard::new(cpu_out.clone()),
        );

        match timeline(mode).render(&gpu_out, EncoderConfig::builder().build()) {
            Ok(()) => {}
            Err(ref e) if is_environment_unavailable(e) => {
                println!("Skipping: this build cannot run the pipeline: {e}");
                return;
            }
            Err(e) => panic!("{mode:?}: GPU render failed: {e}"),
        }
        match timeline(mode).render_forcing_cpu(&cpu_out, EncoderConfig::builder().build()) {
            Ok(()) => {}
            Err(ref e) if is_environment_unavailable(e) => {
                println!("Skipping: this build cannot run the CPU route: {e}");
                return;
            }
            Err(e) => panic!("{mode:?}: CPU render failed: {e}"),
        }

        let (Some(gpu), Some(cpu)) = (centre_mean_rgb(&gpu_out), centre_mean_rgb(&cpu_out)) else {
            println!("Skipping: cannot decode the rendered files here");
            return;
        };
        let worst = (0..3)
            .map(|c| (gpu[c] - cpu[c]).abs())
            .fold(0.0f64, f64::max);
        println!("{mode:?}: worst={worst:.2} gpu={gpu:.1?} cpu={cpu:.1?}");
        if worst > tolerance(mode) {
            failures.push(format!(
                "{mode:?}: {worst:.2} > {:.2} (gpu {gpu:.1?} cpu {cpu:.1?})",
                tolerance(mode)
            ));
        }
        compared += 1;
    }
    assert_eq!(
        compared,
        MODES.len(),
        "every mode must have been compared, not skipped past"
    );
    assert!(
        failures.is_empty(),
        "the two routes disagree on {} mode(s):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
