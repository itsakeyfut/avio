//! Shared GPU compositing core for the bridge (#1626 preview / #1627 export).
//!
//! Owns the `ff-render` context and a cached `Compositor`, and composites a set of
//! derived layers (any [`GpuLayerSource`]) with their decoded frames into an rgba
//! buffer. Both the preview executor ([`GpuPreviewCompositor`](crate::GpuPreviewCompositor))
//! and the export drain use it, so the mapping-to-GPU logic, the v1 identity/aspect
//! gate, and the effect execution live in one place.
//!
//! It returns `None` on any unsupported layer ([`map_scene`] fallback, a non-identity
//! transform, or an aspect mismatch) or any GPU error, so the caller falls back to the
//! CPU compositor for that frame -- never a panic, never a partial result.
//!
//! v1 renders only layers that need no geometric placement (an identity transform and
//! a frame whose aspect matches the canvas): the model's transform is in canvas pixels
//! / clockwise degrees while `ff_render::LayerTransform` is UV-space / counter-clockwise
//! radians, and the compositor stretches each layer to the canvas where the CPU path
//! letterboxes. Matching those exactly is parity work (Br5), so those cases fall back
//! to CPU rather than render wrong output.
//!
//! **Known v1 inefficiencies (deferred, tracked in #1634):** `apply_effects`
//! builds a fresh `RenderGraph` and fresh effect nodes per frame, so an effected layer
//! recompiles its pipeline each frame instead of reusing a cached one; and the
//! no-effects path deep-copies the source frame (`VideoFrame::clone`). Both are on the
//! export hot path but cost nothing for the common no-effect export beyond one copy;
//! a persistent per-effect node cache and an owned-frame `composite` entry point are
//! the fix.

use std::sync::Arc;
use std::time::Duration;

use ff_format::VideoFrame;
use ff_render::{
    ColorGradeNode, ColorWheelsNode, Compositor, CurvesNode, FilmGrainNode, FrameLayer,
    GaussianBlurNode, GlowNode, HslNode, LayerTransform, LutNode, RenderContext, RenderGraph,
    ScaleNode, SharpenNode, VignetteNode,
};

use crate::gpu::{GpuEffect, GpuLayerPlan, GpuLayerSource, GpuMapping, map_scene};

/// Composites derived layers on the GPU, returning `None` (CPU fallback) on
/// unsupported content or any GPU error.
pub struct GpuCompositor {
    ctx: Arc<RenderContext>,
    /// Compositor cached for its target canvas; rebuilt when the canvas changes.
    compositor: Option<(Compositor, (u32, u32))>,
}

impl GpuCompositor {
    /// Initialises a GPU context (best available adapter). Returns `None` when no
    /// adapter is available, so the caller keeps the CPU path.
    #[must_use]
    pub fn new() -> Option<Self> {
        match RenderContext::init_blocking() {
            Ok(ctx) => Some(Self {
                ctx: Arc::new(ctx),
                compositor: None,
            }),
            Err(e) => {
                // Info, not debug: the GPU->CPU fallback reason must stay visible at
                // the default log level (per docs/rules/logging.md), since callers
                // only log `path=cpu` without a reason.
                log::info!("gpu context unavailable reason={e}");
                None
            }
        }
    }

    /// Composites `layers` (bottom to top, each paired with its decoded frame) at
    /// time `t` into a single rgba buffer `(rgba, width, height)`, or `None` to fall
    /// back to the CPU compositor.
    ///
    /// `None` means: `map_scene` reported an unsupported blend/composite/effect, a
    /// layer has a non-identity transform or an aspect that does not match the canvas
    /// (v1 gate), or a GPU error occurred. The caller must never see a wrong-but-
    /// rendered frame.
    pub fn composite<L: GpuLayerSource>(
        &mut self,
        layers: &[(&L, &VideoFrame)],
        canvas: (u32, u32),
        t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let refs: Vec<&L> = layers.iter().map(|(l, _)| *l).collect();
        let plan = match map_scene(&refs, canvas, t) {
            GpuMapping::Gpu(plan) => plan,
            GpuMapping::Fallback(_) => return None,
        };

        let mut frame_layers = Vec::with_capacity(plan.layers.len());
        for (lp, (_, frame)) in plan.layers.iter().zip(layers.iter()) {
            // v1 renders only layers that need no geometric placement (see the module
            // docs); a non-identity transform falls back to CPU rather than render
            // wrong output.
            if !is_identity_transform(lp) {
                return None;
            }
            let processed = self.apply_effects(lp, frame)?;
            // The compositor would stretch a differently-shaped frame to fill the
            // canvas, which the CPU path letterboxes instead; only a canvas-aspect
            // frame composites without distortion.
            if u64::from(processed.width()) * u64::from(canvas.1)
                != u64::from(processed.height()) * u64::from(canvas.0)
            {
                return None;
            }
            frame_layers.push(FrameLayer {
                frame: processed,
                transform: LayerTransform::default(),
                blend_mode: lp.blend_mode,
                opacity: lp.opacity,
                z_order: lp.z_order,
            });
        }

        let rebuild = match &self.compositor {
            Some((_, cached)) => *cached != canvas,
            None => true,
        };
        if rebuild {
            self.compositor = Some((
                Compositor::new(self.ctx.clone(), canvas.0, canvas.1),
                canvas,
            ));
        }
        let (compositor, _) = self.compositor.as_mut()?;
        compositor.composite_to_rgba(&mut frame_layers).ok()
    }

    /// Applies a layer's mappable effects to its rgba frame via a `RenderGraph`, or
    /// returns the frame unchanged when it has none. `None` on a GPU error.
    fn apply_effects(&self, plan: &GpuLayerPlan, frame: &VideoFrame) -> Option<VideoFrame> {
        if plan.effects.is_empty() {
            return Some(frame.clone());
        }
        let (in_w, in_h) = (frame.width(), frame.height());
        let rgba = frame.to_rgba()?;
        let mut graph = RenderGraph::new(self.ctx.clone());
        // A `Scale` node resizes the frame; track the output dimensions so the
        // read-back buffer is wrapped at the right size.
        let (mut out_w, mut out_h) = (in_w, in_h);
        for effect in &plan.effects {
            graph = match effect {
                GpuEffect::ColorGrade {
                    brightness,
                    contrast,
                    saturation,
                    temperature,
                    tint,
                } => graph.push(ColorGradeNode::new(
                    *brightness,
                    *contrast,
                    *saturation,
                    *temperature,
                    *tint,
                )),
                GpuEffect::Scale {
                    width,
                    height,
                    algorithm,
                } => {
                    out_w = *width;
                    out_h = *height;
                    graph.push(ScaleNode::new(*width, *height, *algorithm))
                }
                // Blur preserves the frame dimensions, so out_w/out_h are unchanged.
                GpuEffect::Blur { sigma } => graph.push(GaussianBlurNode::new(*sigma)),
                // Sharpen preserves the frame dimensions too.
                GpuEffect::Sharpen { radius, strength } => {
                    graph.push(SharpenNode::new(*radius, *strength))
                }
                // Vignette preserves the frame dimensions too.
                GpuEffect::Vignette {
                    radius,
                    strength,
                    feather,
                } => graph.push(VignetteNode::new(*radius, *strength, *feather)),
                // FilmGrain preserves the frame dimensions too.
                GpuEffect::FilmGrain {
                    luma_strength,
                    chroma_strength,
                    frame_index,
                } => graph.push(FilmGrainNode::new(
                    *luma_strength,
                    *chroma_strength,
                    *frame_index,
                )),
                // Glow preserves the frame dimensions too.
                GpuEffect::Glow {
                    threshold,
                    radius,
                    intensity,
                } => graph.push(GlowNode::new(*threshold, *radius, *intensity)),
                // ColorWheels preserves the frame dimensions too.
                GpuEffect::ColorWheels {
                    shadows_lift,
                    midtones_gamma,
                    highlights_gain,
                } => graph.push(ColorWheelsNode::new(
                    *shadows_lift,
                    *midtones_gamma,
                    *highlights_gain,
                )),
                // Curves preserves the frame dimensions too.
                GpuEffect::Curves {
                    master,
                    red,
                    green,
                    blue,
                } => graph.push(CurvesNode::new(
                    master.clone(),
                    red.clone(),
                    green.clone(),
                    blue.clone(),
                )),
                // Hsl preserves the frame dimensions too.
                GpuEffect::Hsl {
                    hue_shift,
                    saturation,
                    lightness,
                } => graph.push(HslNode::new(*hue_shift, *saturation, *lightness)),
                // Lut preserves the frame dimensions too. A file the LutNode cannot
                // load (missing, malformed, or an unsupported extension) makes the
                // whole frame fall back to CPU rather than render wrong output (RK-020).
                GpuEffect::Lut { path } => graph.push(load_lut(path)?),
            };
        }
        let out = graph.process_gpu(&rgba, in_w, in_h).ok()?;
        VideoFrame::from_rgba(out_w, out_h, out).ok()
    }
}

/// Loads a `LutNode` from a `.cube` or `.3dl` file, or `None` when the extension is
/// unsupported or the file cannot be loaded. A `None` makes the layer fall back to
/// the CPU path (RK-020) rather than render wrong output.
fn load_lut(path: &str) -> Option<LutNode> {
    let p = std::path::Path::new(path);
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("cube") => LutNode::from_cube(p).ok(),
        Some(ext) if ext.eq_ignore_ascii_case("3dl") => LutNode::from_3dl(p).ok(),
        _ => None,
    }
}

/// Whether a plan layer's transform is the identity (no translate / scale / rotate),
/// within a small tolerance. Non-identity transforms fall back to CPU in v1.
fn is_identity_transform(lp: &GpuLayerPlan) -> bool {
    lp.x.abs() < 1e-6
        && lp.y.abs() < 1e-6
        && (lp.scale_x - 1.0).abs() < 1e-6
        && (lp.scale_y - 1.0).abs() < 1e-6
        && lp.rotation.abs() < 1e-6
}
