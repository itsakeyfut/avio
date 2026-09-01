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
//! **Per-frame cost (#1634):** an effected layer's `RenderGraph` is cached per layer
//! position ([`CachedEffectGraph`]) and reused while its effect list compares equal, so
//! its node pipelines are compiled once instead of every frame; a changed effect list
//! (or layer count) rebuilds. [`GpuEffect::LumaMask`] is excluded (its node embeds the
//! source pixels), and [`composite_owned`](GpuCompositor::composite_owned) moves owned
//! frames so the no-effects export path avoids a `VideoFrame::clone`.

use std::sync::Arc;
use std::time::Duration;

use ff_format::VideoFrame;
use ff_render::{
    ChromaKeyNode, ColorGradeNode, ColorWheelsNode, Compositor, CurvesNode, FilmGrainNode,
    FrameLayer, GaussianBlurNode, GlowNode, HslNode, LayerTransform, LumaMaskNode, LutNode,
    RenderContext, RenderGraph, ScaleNode, ShapeMaskNode, SharpenNode, VignetteNode,
};

use crate::gpu::{GpuEffect, GpuLayerPlan, GpuLayerSource, GpuMapping, map_scene};

/// A per-layer effect [`RenderGraph`] cached across frames so an effected layer does
/// not recompile its node pipelines every frame (#1634). Keyed by the exact effect
/// list **and the input frame dimensions**; reused only when the next frame's effects
/// compare equal (const params) and its size matches, so the graph's baked node params
/// (and any dimension-sized mask, e.g. `ShapeMaskNode`) stay correct and the read-back
/// is wrapped at the right size.
struct CachedEffectGraph {
    effects: Vec<GpuEffect>,
    graph: RenderGraph,
    in_w: u32,
    in_h: u32,
    out_w: u32,
    out_h: u32,
}

/// Composites derived layers on the GPU, returning `None` (CPU fallback) on
/// unsupported content or any GPU error.
pub struct GpuCompositor {
    ctx: Arc<RenderContext>,
    /// Compositor cached for its target canvas; rebuilt when the canvas changes.
    compositor: Option<(Compositor, (u32, u32))>,
    /// Per-layer effect-graph cache, indexed by layer position; reset when the layer
    /// count changes (positions shift). `None` where a layer has no cached graph yet.
    effect_cache: Vec<Option<CachedEffectGraph>>,
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
                effect_cache: Vec::new(),
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
        self.ensure_cache_size(layers.len());

        let mut frame_layers = Vec::with_capacity(plan.layers.len());
        for (idx, (lp, (_, frame))) in plan.layers.iter().zip(layers.iter()).enumerate() {
            // v1 renders only layers that need no geometric placement (see the module
            // docs); a non-identity transform falls back to CPU rather than render
            // wrong output.
            if !is_identity_transform(lp) {
                return None;
            }
            // The preview adapter does not own its frames, so a no-effects layer must
            // clone; `composite_owned` avoids this for the export drain.
            let processed = if lp.effects.is_empty() {
                (*frame).clone()
            } else {
                self.apply_effects(idx, lp, frame)?
            };
            let layer = make_frame_layer(processed, lp, canvas)?;
            frame_layers.push(layer);
        }

        self.finish(frame_layers, canvas)
    }

    /// Like [`composite`](Self::composite) but takes ownership of the layer frames, so a
    /// no-effects layer moves its frame into the compositor instead of cloning it (the
    /// export drain owns each freshly-decoded frame; #1634).
    pub fn composite_owned<L: GpuLayerSource>(
        &mut self,
        layers: Vec<(&L, VideoFrame)>,
        canvas: (u32, u32),
        t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let refs: Vec<&L> = layers.iter().map(|(l, _)| *l).collect();
        let plan = match map_scene(&refs, canvas, t) {
            GpuMapping::Gpu(plan) => plan,
            GpuMapping::Fallback(_) => return None,
        };
        self.ensure_cache_size(layers.len());

        let mut frame_layers = Vec::with_capacity(plan.layers.len());
        for (idx, (lp, (_, frame))) in plan.layers.iter().zip(layers).enumerate() {
            if !is_identity_transform(lp) {
                return None;
            }
            // Owned: a no-effects layer moves its frame in, no clone.
            let processed = if lp.effects.is_empty() {
                frame
            } else {
                self.apply_effects(idx, lp, &frame)?
            };
            let layer = make_frame_layer(processed, lp, canvas)?;
            frame_layers.push(layer);
        }

        self.finish(frame_layers, canvas)
    }

    /// Composites the built `frame_layers` on the (canvas-cached) `Compositor` to rgba.
    fn finish(
        &mut self,
        mut frame_layers: Vec<FrameLayer>,
        canvas: (u32, u32),
    ) -> Option<(Vec<u8>, u32, u32)> {
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

    /// Resets the per-layer effect cache when the layer count changes (positions
    /// shift, so cached entries would misalign). Mirrors `Compositor`'s layer-count
    /// invalidation.
    fn ensure_cache_size(&mut self, n: usize) {
        if self.effect_cache.len() != n {
            self.effect_cache.clear();
            self.effect_cache.resize_with(n, || None);
        }
    }

    /// Applies a layer's mappable effects to its rgba frame via a `RenderGraph`, reusing
    /// the layer's cached graph when the effect list is unchanged and cacheable. `None`
    /// on a GPU error. Must not be called for an empty effect list (the caller handles
    /// that).
    fn apply_effects(
        &mut self,
        layer_idx: usize,
        plan: &GpuLayerPlan,
        frame: &VideoFrame,
    ) -> Option<VideoFrame> {
        let (in_w, in_h) = (frame.width(), frame.height());
        let rgba = frame.to_rgba()?;

        // Reuse the cached graph when this layer's effects are byte-identical, the input
        // dimensions match (a dimension-sized mask or the read-back size would otherwise
        // be stale), and every effect is fully determined by its `GpuEffect` value (see
        // `is_cacheable`).
        if is_cacheable(&plan.effects)
            && let Some(Some(cached)) = self.effect_cache.get(layer_idx)
            && cached.effects == plan.effects
            && (cached.in_w, cached.in_h) == (in_w, in_h)
        {
            let out = cached.graph.process_gpu(&rgba, in_w, in_h).ok()?;
            return VideoFrame::from_rgba(cached.out_w, cached.out_h, out).ok();
        }

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
                // ChromaKey preserves the frame dimensions (it only rewrites alpha).
                GpuEffect::ChromaKey {
                    key_color,
                    tolerance,
                    softness,
                } => graph.push(ChromaKeyNode::new(*key_color, *tolerance, *softness)),
                // LumaMask multiplies alpha by the frame's own BT.709 luma, so the
                // mask is built from the source frame here (preserves dimensions).
                // The mask is baked from the pre-graph frame; when LumaMask follows
                // another effect the GPU mask is the source luma while the CPU `geq`
                // sees the chained frame (a v1 limitation; parity uses it alone).
                GpuEffect::LumaMask { invert } => graph.push(LumaMaskNode::new(
                    build_luma_mask(&rgba, *invert),
                    in_w,
                    in_h,
                )),
                // ShapeMask builds a rectangular alpha mask from the pixel bounds
                // (preserves dimensions; the mask is sized to the source frame).
                GpuEffect::ShapeMask {
                    x,
                    y,
                    width,
                    height,
                    invert,
                } => graph.push(ShapeMaskNode::new(
                    build_shape_mask(in_w, in_h, *x, *y, *width, *height, *invert),
                    in_w,
                    in_h,
                )),
            };
        }

        // Store the built graph, then run it (`process_gpu` borrows, does not consume),
        // so the next frame with identical effects reuses it.
        if is_cacheable(&plan.effects)
            && let Some(slot) = self.effect_cache.get_mut(layer_idx)
        {
            *slot = Some(CachedEffectGraph {
                effects: plan.effects.clone(),
                graph,
                in_w,
                in_h,
                out_w,
                out_h,
            });
            let cached = slot.as_ref()?;
            let out = cached.graph.process_gpu(&rgba, in_w, in_h).ok()?;
            return VideoFrame::from_rgba(out_w, out_h, out).ok();
        }
        // Not cacheable (a frame-content-dependent node): drop any stale entry and run
        // the freshly-built graph without storing it.
        if let Some(slot) = self.effect_cache.get_mut(layer_idx) {
            *slot = None;
        }
        let out = graph.process_gpu(&rgba, in_w, in_h).ok()?;
        VideoFrame::from_rgba(out_w, out_h, out).ok()
    }
}

/// Whether an effect list may be cached and reused across frames. Every mapped node is
/// fully determined by its [`GpuEffect`] value **except** [`GpuEffect::LumaMask`], whose
/// node embeds the source frame's own pixels (`build_luma_mask`) not represented in the
/// `GpuEffect` — reusing it would apply a stale mask (RK-020: never wrong output).
fn is_cacheable(effects: &[GpuEffect]) -> bool {
    !effects
        .iter()
        .any(|e| matches!(e, GpuEffect::LumaMask { .. }))
}

/// Wraps a processed layer frame in a [`FrameLayer`], or `None` when its aspect does not
/// match the canvas (the compositor would stretch it where the CPU path letterboxes, so
/// fall back to CPU rather than render distorted output).
fn make_frame_layer(
    processed: VideoFrame,
    lp: &GpuLayerPlan,
    canvas: (u32, u32),
) -> Option<FrameLayer> {
    if u64::from(processed.width()) * u64::from(canvas.1)
        != u64::from(processed.height()) * u64::from(canvas.0)
    {
        return None;
    }
    Some(FrameLayer {
        frame: processed,
        transform: LayerTransform::default(),
        blend_mode: lp.blend_mode,
        opacity: lp.opacity,
        z_order: lp.z_order,
    })
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

/// Builds the mask [`ff_render::LumaMaskNode`] consumes for the self-luma mask.
///
/// Non-inverted, the mask is the frame itself: the node multiplies the base alpha by
/// `bt709_luma(mask) = bt709_luma(frame)`. Inverted, each pixel becomes a grey of
/// `255 - bt709_luma`, so the node's `bt709_luma(grey) = 255 - luma` gives
/// `alpha *= 1 - luma`. Matches the CPU `geq` self-luma expression.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn build_luma_mask(rgba: &[u8], invert: bool) -> Vec<u8> {
    if !invert {
        return rgba.to_vec();
    }
    let mut mask = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        let luma =
            0.2126 * f32::from(px[0]) + 0.7152 * f32::from(px[1]) + 0.0722 * f32::from(px[2]);
        let grey = (255.0 - luma).clamp(0.0, 255.0).round() as u8;
        mask.extend_from_slice(&[grey, grey, grey, 255]);
    }
    mask
}

/// Builds the mask [`ff_render::ShapeMaskNode`] consumes: alpha `255` inside the
/// rectangle `[x, x+width) x [y, y+height)` and `0` outside (swapped when `invert`).
/// The node keeps a pixel where the mask alpha is `> 1`, so this exactly matches the
/// CPU `RectMask` `geq` (`between(X, x, x+width-1)`), which is inclusive of the far
/// edge. RGB is unused by the node, so it is left `0`.
fn build_shape_mask(
    w: u32,
    h: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    invert: bool,
) -> Vec<u8> {
    let (inside, outside) = if invert { (0u8, 255u8) } else { (255u8, 0u8) };
    let (x_end, y_end) = (x.saturating_add(width), y.saturating_add(height));
    let mut mask = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for py in 0..h {
        for px in 0..w {
            let alpha = if px >= x && px < x_end && py >= y && py < y_end {
                inside
            } else {
                outside
            };
            mask.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    mask
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
