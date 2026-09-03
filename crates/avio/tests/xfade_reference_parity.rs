//! `ff_preview::apply_xfade` against the transition the export actually writes (#1732).
//!
//! `apply_xfade` is what the preview shows for a transition, and the export runs
//! `FFmpeg`'s `xfade` filter. If the two disagree, the preview lies about the finished
//! video — which is exactly what this suite was written to catch, after `Dissolve` was
//! found choosing a different set of pixels (mean 54) and the dips following a different
//! curve entirely (mean 78).
//!
//! # Why neither existing suite covers this
//!
//! - `gpu_parity_tests` compares each `ff-render` node against `apply_xfade`. Both could
//!   be wrong in the same way and it would still pass — and for four kinds, both were.
//! - `gpu_export_tests` compares the two *export* routes. For a kind the GPU path
//!   declines, both legs are `FFmpeg` and it passes regardless.
//!
//! Only comparing the reference against a real export closes that, so this suite renders
//! one and reads its pixels back.
//!
//! # Why there are two legs
//!
//! They fail on different mistakes and neither subsumes the other:
//!
//! - **Flat sources** make both clips a single colour, so the chroma planes are identical
//!   and 4:2:0 subsampling cannot influence the comparison. That isolates each kind's
//!   *formula* — the dip's curve, the dissolve's pixel set, the wipe's edge column — and
//!   is where the bound is tight. It cannot see direction: a mirrored wipe over a uniform
//!   field looks the same.
//! - **Structured sources** carry a horizontal and a vertical ramp, so geometry and
//!   direction are visible (a mirrored wipe reads ~127 here). The bound is looser because
//!   `FFmpeg` selects per plane and chroma is half-resolution, while `apply_xfade` works
//!   in rgba — a gap inherent to the two representations, not to the formulas (RK-022).
//!
//! `Dissolve` is deliberately absent from the structured leg: its selection is per-pixel
//! scatter, the worst case for 4:2:0, so it reads ~34 there no matter how exact the
//! selection is. It has no direction to check either, so the flat leg is the whole of its
//! coverage here — and on a bound of its own, because it is the one kind whose agreement
//! depends on two libms matching rather than on arithmetic (see
//! `TOL_FLAT_DISSOLVE_MEAN`). That same dependence is why the GPU *export* declines
//! `Dissolve` outright.
//!
//! Probe-gated (RK-002): the source encode and the export skip gracefully when the
//! environment cannot run them.
//!
//! Every export writes to a path keyed by its leg and kind. The two legs run in parallel
//! under `cargo test`, so a shared output file would have them decoding each other's
//! render — and that fails deterministically while reporting a mean of 107, which reads
//! as "the formula is wrong" rather than "the file was overwritten" (RK-019).

#![cfg(feature = "preview")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoCodec, VideoEncoder};
use ff_filter::XfadeTransition;
use ff_format::{PixelFormat, VideoFrame};
use fixtures::{FileGuard, test_output_path};

const W: u32 = 64;
const H: u32 = 64;
/// Frames per source clip: 1 s at 30 fps.
const CLIP_FRAMES: usize = 30;
/// Transition length in output frames: 0.5 s at 30 fps.
const WINDOW: usize = 15;

/// Bound for the flat leg. Measured worst-frame means: wipes 1.0, `Dissolve` 1.0,
/// `Fade` 2.0, `FadeWhite` 2.3, `FadeBlack` 2.7 — the encode round trip's own floor.
/// A wrong formula is nowhere near it: the linear dip this replaced read 18.7 here and
/// the previous dissolve hash read 54.
const TOL_FLAT_MEAN: f64 = 3.5;

/// Bound for `Dissolve`, which is the one kind whose agreement is not pure arithmetic.
///
/// Its selection is `xfade_frand`, i.e. `sinf` of an argument large enough that the
/// result depends on the libm evaluating it — Rust's here, `FFmpeg`'s in the export. They
/// agree exactly on some platforms (0% of pixels disagreeing, measured on Windows) and
/// approximately on others, so this leg pins that the formula is *right* while leaving
/// room for the platform to disagree about a few pixels. It is still an order of
/// magnitude below the 54 an unrelated hash produces, which is what this guards against.
const TOL_FLAT_DISSOLVE_MEAN: f64 = 12.0;

/// Bound for the structured leg. Measured worst-frame means: `FadeBlack` 2.8, `Fade` 3.0,
/// `FadeWhite` 3.0, wipes 3.4–4.7. The wipes sit higher because their seam falls on a
/// chroma boundary; a *mirrored* wipe would read ~127.
const TOL_STRUCTURED_MEAN: f64 = 6.0;

/// Every kind whose GPU node and CPU reference are pinned to `FFmpeg`'s formula.
const MAPPED_KINDS: &[XfadeTransition] = &[
    XfadeTransition::Fade,
    XfadeTransition::Dissolve,
    XfadeTransition::WipeLeft,
    XfadeTransition::WipeRight,
    XfadeTransition::WipeUp,
    XfadeTransition::WipeDown,
    XfadeTransition::FadeBlack,
    XfadeTransition::FadeWhite,
];

fn export_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        // Generous, so the comparison measures the transition rather than the encoder.
        .bitrate_mode(BitrateMode::Cbr(4_000_000))
        .build()
}

/// Encodes `CLIP_FRAMES` frames of `rgba`, or `None` when the environment has no usable
/// encoder (skip).
fn encode_source(path: &std::path::Path, rgba: &[u8]) -> Option<()> {
    let mut enc = VideoEncoder::create(path)
        .video(W, H, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .build()
        .ok()?;
    for _ in 0..CLIP_FRAMES {
        enc.push_video(&VideoFrame::from_rgba(W, H, rgba.to_vec()).ok()?)
            .ok()?;
    }
    enc.finish().ok()?;
    Some(())
}

/// A single-colour frame: both clips share their chroma, so 4:2:0 subsampling cannot
/// affect which source a pixel came from.
fn flat_rgba(level: u8) -> Vec<u8> {
    vec![level; (W * H * 4) as usize]
}

/// A frame with a horizontal ramp in R and a vertical one in G, offset per clip by
/// `phase`, so direction and geometry are both visible.
fn structured_rgba(phase: u8) -> Vec<u8> {
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let o = ((y * W + x) * 4) as usize;
            rgba[o] = u8::try_from(x * 255 / W).unwrap_or(255).wrapping_add(phase);
            rgba[o + 1] = u8::try_from(y * 255 / H)
                .unwrap_or(255)
                .wrapping_add(phase.wrapping_mul(2));
            rgba[o + 2] = 128u8.wrapping_sub(phase);
            rgba[o + 3] = 255;
        }
    }
    rgba
}

/// Every decoded frame of `path` as rgba, or an empty vec when it cannot be decoded.
fn decode_rgba(path: &std::path::Path) -> Vec<Vec<u8>> {
    let Ok(mut d) = VideoDecoder::open(path)
        .output_format(PixelFormat::Rgba)
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

/// Mean absolute difference over the RGB channels (alpha carries no meaning here).
fn mean_abs_diff_rgb(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()) / 4;
    assert!(n > 0, "compared an empty frame");
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

fn render_or_skip(result: Result<(), TimelineError>) -> bool {
    match result {
        Ok(()) => true,
        Err(TimelineError::Filter(_) | TimelineError::Encode(_) | TimelineError::Decode(_)) => {
            false
        }
        Err(e) => panic!("unexpected export error: {e}"),
    }
}

/// Exports `a` then `b` with `kind` between them and returns the worst per-frame mean
/// difference between the export's transition frames and `apply_xfade` at the same
/// progress, or `None` when the environment cannot run the export.
///
/// `leg` names the caller so the export lands on its own path. The two legs run in
/// parallel under `cargo test`, and a shared output file has them decoding each other's
/// render -- which fails *deterministically*, reporting a mean of 107 as though the
/// formula were wrong.
fn worst_window_divergence(
    leg: &str,
    kind: XfadeTransition,
    a: &std::path::Path,
    b: &std::path::Path,
) -> Option<(f64, usize)> {
    let timeline = Timeline::builder()
        .canvas(W, H)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::new(a).trim(Duration::ZERO, Duration::from_secs(1)),
            Clip::new(b)
                .offset(Duration::from_secs(1))
                .trim(Duration::ZERO, Duration::from_secs(1))
                .with_transition(kind, Duration::from_millis(500)),
        ])
        .build()
        .ok()?;

    let out = test_output_path(&format!("xfade_ref_{leg}_{kind:?}.mp4"));
    let _guard = FileGuard::new(out.clone());
    let _ = std::fs::remove_file(&out);
    // Force-CPU: the reference is FFmpeg's own filter, which is what this route runs.
    if !render_or_skip(timeline.render_forcing_cpu(&out, export_config())) {
        return None;
    }

    let (fa, fb, exported) = (decode_rgba(a), decode_rgba(b), decode_rgba(&out));
    if fa.len() < CLIP_FRAMES || fb.len() < WINDOW || exported.len() < CLIP_FRAMES {
        return None; // decoder unavailable or a short round trip -> skip
    }

    // The transition occupies outputs `CLIP_FRAMES - WINDOW ..< CLIP_FRAMES`, showing
    // clip A's tail against clip B's head (#1659 measured this mapping against the CPU
    // export, and `gpu_export`'s drain reproduces it).
    let mut worst = (0f64, 0usize);
    for j in 0..WINDOW {
        #[allow(clippy::cast_precision_loss)]
        let progress = j as f32 / WINDOW as f32;
        let mut reference = Vec::new();
        ff_preview::apply_xfade(
            kind,
            &fa[(CLIP_FRAMES - WINDOW) + j],
            &fb[j],
            progress,
            W,
            H,
            &mut reference,
        );
        let mean = mean_abs_diff_rgb(&exported[(CLIP_FRAMES - WINDOW) + j], &reference);
        if mean > worst.0 {
            worst = (mean, j);
        }
    }
    Some(worst)
}

#[test]
fn apply_xfade_should_match_the_export_formula_for_every_mapped_kind() {
    // Flat sources: identical chroma in both clips, so this measures the formula alone.
    let a = test_output_path("xfade_ref_flat_a.mp4");
    let b = test_output_path("xfade_ref_flat_b.mp4");
    let _ga = FileGuard::new(a.clone());
    let _gb = FileGuard::new(b.clone());
    if encode_source(&a, &flat_rgba(40)).is_none() || encode_source(&b, &flat_rgba(210)).is_none() {
        return; // encoder unavailable -> skip
    }

    for kind in MAPPED_KINDS {
        let Some((mean, frame)) = worst_window_divergence("flat", *kind, &a, &b) else {
            return; // environment cannot run the export -> skip the whole suite
        };
        println!("flat {kind:?}: worst mean={mean:.3} (window frame {frame})");
        let bound = if *kind == XfadeTransition::Dissolve {
            TOL_FLAT_DISSOLVE_MEAN
        } else {
            TOL_FLAT_MEAN
        };
        assert!(
            mean <= bound,
            "{kind:?}: the preview reference diverges from the export by {mean:.3} at \
             window frame {frame} (tolerance {TOL_FLAT_MEAN}); the formula does not match \
             FFmpeg's"
        );
    }
}

#[test]
fn apply_xfade_should_match_the_export_geometry_on_colourful_sources() {
    // Structured sources: this is the leg that sees direction. `Dissolve` is excluded --
    // see the module docs.
    let a = test_output_path("xfade_ref_struct_a.mp4");
    let b = test_output_path("xfade_ref_struct_b.mp4");
    let _ga = FileGuard::new(a.clone());
    let _gb = FileGuard::new(b.clone());
    if encode_source(&a, &structured_rgba(0)).is_none()
        || encode_source(&b, &structured_rgba(100)).is_none()
    {
        return; // encoder unavailable -> skip
    }

    for kind in MAPPED_KINDS
        .iter()
        .filter(|k| **k != XfadeTransition::Dissolve)
    {
        let Some((mean, frame)) = worst_window_divergence("struct", *kind, &a, &b) else {
            return;
        };
        println!("structured {kind:?}: worst mean={mean:.3} (window frame {frame})");
        assert!(
            mean <= TOL_STRUCTURED_MEAN,
            "{kind:?}: the preview reference diverges from the export by {mean:.3} at \
             window frame {frame} (tolerance {TOL_STRUCTURED_MEAN}); a mirrored or \
             mis-keyed transition reads far above this"
        );
    }
}
