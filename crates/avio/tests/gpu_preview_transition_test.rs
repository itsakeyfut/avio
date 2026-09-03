//! The preview's GPU blend renders the same picture as the CPU one (#1726).
//!
//! `GpuPreviewCompositor::blend` is what the runner reaches when a transition routes to
//! the GPU. AC1 asks for "no visible difference at the seam", so this measures it against
//! `ff_preview::apply_xfade` — the path the runner falls back to — rather than asserting
//! it from the fact that #1732 pinned the two together.
//!
//! Both sides work in rgba with no encode in between, so the bound is tight: this is
//! GPU-versus-CPU arithmetic, not a round trip.
//!
//! Probe-gated (RK-002): skips without an adapter.

#![cfg(all(feature = "gpu", feature = "preview"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use avio::GpuPreviewCompositor;
use ff_filter::XfadeTransition;
use ff_preview::PreviewCompositor;

const W: u32 = 64;
const H: u32 = 64;

/// The kinds the preview routes to the GPU. Kept beside the assertion rather than
/// derived from the policy, so a change to the policy shows up here as a decision to
/// make rather than a test that silently covers less.
const ROUTED: &[XfadeTransition] = &[XfadeTransition::FadeBlack, XfadeTransition::FadeWhite];

/// Everything else maps to a node but is faster on the CPU, so `blend` must decline it
/// and leave the runner on `apply_xfade`.
const NOT_ROUTED: &[XfadeTransition] = &[
    XfadeTransition::Fade,
    XfadeTransition::Dissolve,
    XfadeTransition::WipeLeft,
    XfadeTransition::WipeRight,
    XfadeTransition::WipeUp,
    XfadeTransition::WipeDown,
    XfadeTransition::SlideLeft,
    XfadeTransition::Pixelize,
];

/// Mean absolute RGB difference. Both sides are rgba with no encode between them.
const TOL_MEAN: f64 = 1.0;

/// Structured, colourful frames: on a flat fill a dip looks the same whatever it does to
/// the colour channels, so a mis-keyed one would pass unnoticed (RK-022).
fn structured(phase: u8) -> Vec<u8> {
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

#[test]
fn gpu_preview_blend_should_match_the_cpu_reference_for_every_routed_kind() {
    let Some(mut gpu) = GpuPreviewCompositor::new() else {
        return; // no adapter -> skip
    };
    let (a, b) = (structured(0), structured(100));

    for kind in ROUTED {
        // The endpoints and the midpoint: 0 and 1 are what pin direction, and the dips
        // differ from a linear blend mainly in between.
        for progress in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let out = gpu
                .blend(*kind, &a, &b, progress, W, H)
                .unwrap_or_else(|| panic!("{kind:?} routes to the GPU and must blend"));
            assert_eq!(out.len(), a.len(), "{kind:?} @{progress}: size mismatch");

            let mut reference = Vec::new();
            ff_preview::apply_xfade(*kind, &a, &b, progress, W, H, &mut reference);
            let mean = mean_abs_diff_rgb(&out, &reference);
            println!("{kind:?} @{progress}: mean={mean:.3}");
            assert!(
                mean <= TOL_MEAN,
                "{kind:?} @{progress}: the GPU blend diverged from the CPU one by \
                 {mean:.3} (tolerance {TOL_MEAN}); the preview would show a seam"
            );
        }
    }
}

#[test]
fn gpu_preview_blend_should_decline_the_kinds_it_does_not_render() {
    // The other half of AC1: a kind the preview keeps on the CPU must return `None` so
    // the runner falls back, rather than render an approximation of it.
    let Some(mut gpu) = GpuPreviewCompositor::new() else {
        return;
    };
    let (a, b) = (structured(0), structured(100));
    for kind in NOT_ROUTED {
        assert!(
            gpu.blend(*kind, &a, &b, 0.5, W, H).is_none(),
            "{kind:?} must be left to the runner's CPU path"
        );
    }
}
