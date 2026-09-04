//! Shared GPU compositing core for the bridge (#1626 preview / #1627 export).
//!
//! Owns the `ff-render` context and a cached `Compositor`, and composites a set of
//! derived layers (any [`GpuLayerSource`]) with their decoded frames into an rgba
//! buffer. Both the preview executor ([`GpuPreviewCompositor`](crate::GpuPreviewCompositor))
//! and the export drain use it, so the mapping-to-GPU logic, the v1 identity gate, the
//! letterbox, and the effect execution live in one place.
//!
//! It returns `None` on any unsupported layer ([`map_scene`] fallback, a non-identity
//! model transform, or layers of mixed aspect) or any GPU error, so the caller falls
//! back to the CPU compositor for that frame -- never a panic, never a partial result.
//!
//! v1 renders only layers that need no geometric placement (an identity transform): the
//! model's transform is in canvas pixels / clockwise degrees while
//! `ff_render::LayerTransform` is UV-space / counter-clockwise radians, so a positioned
//! or rotated layer falls back to CPU rather than render wrong output (RK-020).
//!
//! **Letterbox (#1661):** a frame whose aspect differs from the canvas is *fitted* into
//! it, matching the CPU compositor's `scale=…:force_original_aspect_ratio=decrease` +
//! `pad` pass instead of the compositor's default stretch (see [`fit_size`]). Since the
//! CPU fits the *composited* result while this fits each layer before compositing, the
//! two agree only when every layer lands in the same band, so a scene whose layers do
//! not all share one aspect still falls back -- as does one whose aspect is extreme
//! enough that a fitted side rounds away to nothing.
//!
//! **Per-frame cost (#1634):** an effected layer's `RenderGraph` is cached per layer
//! position ([`CachedEffectGraph`]) and reused while its effect list compares equal, so
//! its node pipelines are compiled once instead of every frame; a changed effect list
//! (or layer count) rebuilds. [`GpuEffect::LumaMask`] is excluded (its node embeds the
//! source pixels), and [`composite_owned`](GpuCompositor::composite_owned) moves owned
//! frames so the no-effects export path avoids a `VideoFrame::clone`.
//!
//! **Stateful effects (#1653):** a [`GpuEffect::MotionBlur`] node accumulates a trail
//! across a clip's frames, so its cross-frame reuse *is* the accumulation. A caller
//! that composites a sequence of clips at one layer position must call
//! [`reset_effect_cache`](GpuCompositor::reset_effect_cache) at each clip boundary so
//! the trail does not bleed across a cut (RK-025); the export drain does this. The
//! preview runner's clip-boundary reset is a documented follow-up.

use std::sync::Arc;
use std::time::Duration;

use ff_format::VideoFrame;
use ff_render::{
    ChromaKeyNode, ColorGradeNode, ColorWheelsNode, Compositor, CurvesNode, DipToColorNode,
    DissolveTransitionNode, FadeTransitionNode, FilmGrainNode, FrameLayer, GaussianBlurNode,
    GlowNode, HslNode, LayerTransform, LumaMaskNode, LutNode, MotionBlurNode, RenderContext,
    RenderGraph, ScaleNode, ShapeMaskNode, SharpenNode, VignetteNode, WipeTransitionNode,
};

use crate::gpu::{GpuEffect, GpuLayerPlan, GpuLayerSource, GpuMapping, map_scene};
use crate::gpu_transition::GpuTransition;

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
    /// layer has a non-identity model transform (v1 gate), the layers do not all share
    /// one aspect, or a GPU error occurred. The caller must never see a wrong-but-
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

        let mut processed = Vec::with_capacity(plan.layers.len());
        for (idx, (lp, (_, frame))) in plan.layers.iter().zip(layers.iter()).enumerate() {
            // v1 renders only layers that need no geometric placement (see the module
            // docs); a non-identity transform falls back to CPU rather than render
            // wrong output.
            if !is_identity_transform(lp) {
                return None;
            }
            // The preview adapter does not own its frames, so a no-effects layer must
            // clone; `composite_owned` avoids this for the export drain.
            let out = if lp.effects.is_empty() {
                (*frame).clone()
            } else {
                self.apply_effects(idx, lp, frame)?
            };
            processed.push((out, lp));
        }

        self.finish(assemble(processed, canvas)?, canvas)
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

        let mut processed = Vec::with_capacity(plan.layers.len());
        for (idx, (lp, (_, frame))) in plan.layers.iter().zip(layers).enumerate() {
            if !is_identity_transform(lp) {
                return None;
            }
            // Owned: a no-effects layer moves its frame in, no clone.
            let out = if lp.effects.is_empty() {
                frame
            } else {
                self.apply_effects(idx, lp, &frame)?
            };
            processed.push((out, lp));
        }

        self.finish(assemble(processed, canvas)?, canvas)
    }

    /// Runs `transition` over the composited canvas frames `a` and `b` at `progress`
    /// (`0` = all `a`, `1` = all `b`), returning the blended rgba or `None` on a GPU
    /// error.
    ///
    /// Both buffers are already-composited `w` x `h` canvases, so the transition sits
    /// *after* compositing -- matching the CPU route, where `xfade` is the trailing step
    /// of the incoming layer's chain (`composition_inner.rs`).
    ///
    /// Every node here reproduces `FFmpeg`'s own formula for its kind (#1732), which is
    /// what lets the export use them at all: the export replaces `FFmpeg`'s `xfade`, so a
    /// node that merely looks similar would ship a different picture than the CPU route.
    /// `gpu_export`'s `export_maps_to_gpu` decides which kinds are allowed through.
    ///
    /// Each call builds a node, which compiles its pipeline in a per-instance `OnceLock`
    /// -- accepted for v1 since a transition is `duration x fps` frames of an offline
    /// export (#1659).
    pub(crate) fn transition(
        &mut self,
        transition: GpuTransition,
        progress: f32,
        a: &[u8],
        b: Vec<u8>,
        w: u32,
        h: u32,
    ) -> Option<Vec<u8>> {
        let graph = RenderGraph::new(Arc::clone(&self.ctx));
        let graph = match transition {
            GpuTransition::Fade => graph.push(FadeTransitionNode::new(progress, b, w, h)),
            // The mask is built here rather than in the shader: `FFmpeg`'s dissolve noise
            // outgrows `f32` well before 1080p, so a `WGSL` copy would reveal a different
            // set of pixels than the CPU reference (`ff_filter::xfade_frand`).
            GpuTransition::Dissolve => graph.push(DissolveTransitionNode::new(
                ff_filter::dissolve_mask(w, h, progress),
                b,
                w,
                h,
            )),
            // Zero softness: `FFmpeg`'s wipes have a hard edge, and that is also what
            // switches the node onto its integer-column rule.
            GpuTransition::Wipe { angle } => {
                graph.push(WipeTransitionNode::new(progress, 0.0, angle, b, w, h))
            }
            GpuTransition::Dip { color } => {
                graph.push(DipToColorNode::new(progress, color, b, w, h))
            }
        };
        graph.process_gpu(a, w, h).ok()
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

    /// Drops every cached effect graph (keeping the slot count), so the next composite
    /// rebuilds each layer's graph from scratch.
    ///
    /// A stateful effect node (e.g. [`MotionBlurNode`], whose exposure trail
    /// accumulates across the frames of one clip) is embedded in the cached graph, so a
    /// caller that composites a sequence of clips at the same layer position must call
    /// this **at each clip boundary** — otherwise the previous clip's accumulated trail
    /// bleeds into the next clip's first frame (RK-025). Stateless effects are
    /// unaffected beyond a one-frame pipeline rebuild.
    pub fn reset_effect_cache(&mut self) {
        for slot in &mut self.effect_cache {
            *slot = None;
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
                // MotionBlur is stateful (the trail accumulates across frames on this
                // node), so it depends on the cached graph being *reused* across a
                // clip's frames. It stays cacheable; the accumulation is reset at a
                // clip boundary via `reset_effect_cache` so a trail never bleeds across
                // a cut (RK-025). Preserves the frame dimensions.
                GpuEffect::MotionBlur {
                    shutter_angle,
                    sub_frames,
                } => graph.push(MotionBlurNode::new(*shutter_angle, *sub_frames)),
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

/// Wraps each processed layer frame in a [`FrameLayer`], letterboxing it into the canvas
/// (#1661), or `None` when the frames do not all share one aspect or the fit degenerates.
///
/// The CPU compositor fits the **composited** result into the canvas, while this fits
/// each layer *before* compositing. The two coincide exactly when every layer ends up in
/// the same band, which is what the shared-aspect requirement enforces; a mixed-aspect
/// scene falls back to CPU rather than render a band per layer (RK-020).
fn assemble(
    processed: Vec<(VideoFrame, &GpuLayerPlan)>,
    canvas: (u32, u32),
) -> Option<Vec<FrameLayer>> {
    let (first, _) = processed.first()?;
    let (w0, h0) = (u64::from(first.width()), u64::from(first.height()));
    if processed
        .iter()
        .any(|(f, _)| u64::from(f.width()) * h0 != u64::from(f.height()) * w0)
    {
        return None;
    }

    let transform = letterbox_transform(first.width(), first.height(), canvas)?;
    Some(
        processed
            .into_iter()
            .map(|(frame, lp)| FrameLayer {
                frame,
                transform: transform.clone(),
                blend_mode: lp.blend_mode,
                composite_op: lp.composite_op,
                opacity: lp.opacity,
                z_order: lp.z_order,
            })
            .collect(),
    )
}

/// The [`LayerTransform`] that fits an `fw` x `fh` frame inside `canvas`, letterboxing or
/// pillarboxing the remainder, or `None` when the fit leaves no band to draw.
///
/// `transform.wgsl` samples `(uv - 0.5) / scale` and returns transparent outside `[0, 1]`,
/// so a scale of `fit / canvas` draws the frame as a centred band of exactly that fraction
/// of the canvas; `blend.wgsl` then leaves the canvas' black in the bars, matching the CPU
/// path's `pad=…:color=black`. A frame that already matches the canvas aspect yields the
/// identity, which `ff_render`'s compositor skips entirely -- so that (previously the only
/// supported) case keeps its exact pixels and its per-frame cost.
///
/// Note that an **odd** canvas dimension never yields the identity: the fit rounds down to
/// an even size, so a matching-aspect frame still scales (by one pixel) and centres half a
/// pixel off the CPU's `pad=(ow-iw)/2`. That is `FFmpeg`'s own `force_divisible_by=2`
/// behaviour, so the two paths still agree on geometry.
fn letterbox_transform(fw: u32, fh: u32, canvas: (u32, u32)) -> Option<LayerTransform> {
    let (fit_w, fit_h) = fit_size(fw, fh, canvas);
    // An aspect extreme enough for one fitted side to round away to nothing has no band
    // to draw. Passing that on as a zero scale would not fail loudly: `transform.wgsl`
    // floors the divisor at 1e-4, so every sample lands outside `[0, 1]` and the layer
    // disappears into a silently black frame -- exactly the wrong output this module
    // must never produce (RK-020). The CPU leg cannot build its `scale` at that size
    // either, so falling back is the honest answer.
    if fit_w == 0 || fit_h == 0 {
        return None;
    }
    if (fit_w, fit_h) == canvas {
        return Some(LayerTransform::default());
    }
    Some(LayerTransform {
        scale_x: ratio(fit_w, canvas.0),
        scale_y: ratio(fit_h, canvas.1),
        ..LayerTransform::default()
    })
}

/// `a / b` as an `f32`, for the pixel counts this module deals in.
#[allow(clippy::cast_precision_loss)] // pixel dimensions are far inside f32's exact range
fn ratio(a: u32, b: u32) -> f32 {
    a as f32 / b as f32
}

/// The size an `fw` x `fh` frame scales to when fitted inside `canvas`, mirroring the CPU
/// path's `scale=w:h:force_original_aspect_ratio=decrease:force_divisible_by=2`
/// (`composition_inner.rs`): each side is the aspect-preserving candidate clamped to the
/// canvas, then rounded **down** to a multiple of two.
///
/// Reproducing the rounding rather than using the exact real-valued ratio is what keeps
/// the band boundary within a pixel of the CPU leg; the parity test measures the residue.
/// A degenerate zero-sized frame yields the canvas, i.e. the compositor's plain stretch.
#[allow(clippy::cast_possible_truncation)] // each side is clamped to the canvas, a u32
fn fit_size(fw: u32, fh: u32, canvas: (u32, u32)) -> (u32, u32) {
    if fw == 0 || fh == 0 {
        return canvas;
    }
    let (cw, ch) = (u64::from(canvas.0), u64::from(canvas.1));
    let (fw, fh) = (u64::from(fw), u64::from(fh));
    // `av_rescale` rounds to nearest, half away from zero.
    let rescale = |num: u64, den: u64| (num + den / 2) / den;
    let even = |v: u64| v / 2 * 2;
    (
        even(rescale(ch * fw, fh).min(cw)) as u32,
        even(rescale(cw * fh, fw).min(ch)) as u32,
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_should_letterbox_a_wide_frame() {
        // 16:9 into a square canvas: full width, bars top and bottom.
        assert_eq!(fit_size(64, 36, (64, 64)), (64, 36));
    }

    #[test]
    fn fit_size_should_pillarbox_a_tall_frame() {
        // The mirror case, so the fit is not accidentally width-only.
        assert_eq!(fit_size(36, 64, (64, 64)), (36, 64));
    }

    #[test]
    fn fit_size_should_round_down_to_an_even_size() {
        // 3:2 into a square: the exact fit is 66.67 rows, which `av_rescale` rounds to
        // 67 and `force_divisible_by=2` then takes down to 66. Drives that branch on its
        // own, since every other case here is already even.
        assert_eq!(fit_size(30, 20, (100, 100)), (100, 66));
    }

    #[test]
    fn letterbox_transform_should_scale_only_the_fitted_axis() {
        let t = letterbox_transform(64, 36, (64, 64)).expect("a 16:9 fit has a band");
        assert!((t.scale_x - 1.0).abs() < 1e-6, "full width: {}", t.scale_x);
        assert!((t.scale_y - 0.5625).abs() < 1e-6, "36/64: {}", t.scale_y);
        assert!(t.x.abs() < 1e-6 && t.y.abs() < 1e-6 && t.rotation.abs() < 1e-6);
    }

    #[test]
    fn letterbox_transform_should_be_identity_for_a_canvas_aspect_frame() {
        // The previously-only-supported shape must stay on the compositor's
        // transform-free path, at both the canvas size and a larger one.
        let same = letterbox_transform(64, 48, (64, 48)).expect("a matching aspect fits");
        assert!(same.is_identity());
        let larger = letterbox_transform(1920, 1080, (64, 36)).expect("a matching aspect fits");
        assert!(larger.is_identity());
    }

    #[test]
    fn letterbox_transform_should_reject_a_fit_that_rounds_away_to_nothing() {
        // 64:1 into a square canvas: the fitted height is one row, which the
        // even-rounding takes to zero. `transform.wgsl` floors the divisor at 1e-4, so a
        // zero scale would not error -- it would sample the whole layer out of range and
        // render a silently black frame. Falling back is the only correct answer.
        assert_eq!(fit_size(4096, 64, (64, 64)), (64, 0));
        assert!(letterbox_transform(4096, 64, (64, 64)).is_none());
        // The mirror case, so the guard is not accidentally height-only.
        assert_eq!(fit_size(64, 4096, (64, 64)), (0, 64));
        assert!(letterbox_transform(64, 4096, (64, 64)).is_none());
    }
}
