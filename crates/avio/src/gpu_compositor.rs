//! Shared GPU compositing core for the bridge (#1626 preview / #1627 export).
//!
//! Owns the `ff-render` context and a cached `Compositor`, and composites a set of
//! derived layers (any [`GpuLayerSource`]) with their decoded frames into an rgba
//! buffer. Both the preview executor ([`GpuPreviewCompositor`](crate::GpuPreviewCompositor))
//! and the export drain use it, so the mapping-to-GPU logic, the layer placement, the
//! letterbox, and the effect execution live in one place.
//!
//! It returns `None` on any unsupported layer ([`map_scene`] fallback, or a placement
//! with no GPU equivalent) or any GPU error, so the caller falls back to the CPU
//! compositor for that frame -- never a panic, never a partial result.
//!
//! **Placement (#1633):** `layer_transform` reproduces the CPU compositor's geometry,
//! which works in *base-layer space*: layer 0 defines the size and its own transform is
//! ignored, every other layer is stretched to that size, scaled by `base * scale`, and
//! overlaid with its top-left at `(x, y)`. Only the finished composite is fitted into the
//! canvas. The model's units are canvas pixels / clockwise degrees while
//! `ff_render::LayerTransform` is UV-space / counter-clockwise radians, so the conversion
//! lives there and is pinned by measurement against the CPU. A **rotated** non-base layer
//! still falls back: the CPU's `rotate` fills the corners it exposes with `fillcolor`
//! while the GPU transform leaves them transparent, so there is nothing to map it to
//! (RK-020).
//!
//! **Letterbox (#1661):** a frame whose aspect differs from the canvas is *fitted* into
//! it, matching the CPU compositor's `scale=…:force_original_aspect_ratio=decrease` +
//! `pad` pass instead of the compositor's default stretch (see [`fit_size`]). A scene
//! whose base aspect is extreme enough that a fitted side rounds away to nothing still
//! falls back.
//!
//! **Per-frame cost (#1634):** an effected layer's `RenderGraph` is cached per layer
//! position ([`CachedEffectGraph`]) and reused while its effect list compares equal, so
//! its node pipelines are compiled once instead of every frame; a changed effect list
//! (or layer count) rebuilds. Every effect is cacheable since the mask nodes stopped
//! baking a mask buffer (#1710), and
//! [`composite_owned`](GpuCompositor::composite_owned) moves owned frames so the
//! no-effects export path avoids a `VideoFrame::clone`.
//!
//! **Stateful effects (#1653):** a [`GpuEffect::MotionBlur`] node accumulates a trail
//! across a clip's frames, so its cross-frame reuse *is* the accumulation. A caller
//! that composites a sequence of clips at one layer position must call
//! [`reset_effect_cache`](GpuCompositor::reset_effect_cache) at each clip boundary so
//! the trail does not bleed across a cut (RK-025). The export drain does this per
//! clip, and the preview runner does it at each cut and on every seek (#1705).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ff_format::VideoFrame;
use ff_render::{
    ChromaKeyNode, ColorGradeNode, ColorWheelsNode, Compositor, CurvesNode, DipToColorNode,
    DissolveTransitionNode, FadeTransitionNode, FilmGrainNode, FrameLayer, GaussianBlurNode,
    GlowNode, HslNode, LayerTransform, LumaMaskNode, LutNode, MotionBlurNode, NodeParam,
    RenderContext, RenderGraph, ScaleNode, ShapeMaskNode, SharpenNode, VignetteNode,
    WipeTransitionNode,
};

use crate::gpu::{GpuEffect, GpuLayerPlan, GpuLayerSource, GpuMapping, map_scene};
use crate::gpu_transition::GpuTransition;

/// A per-layer effect [`RenderGraph`] cached across frames so an effected layer does
/// not recompile its node pipelines every frame (#1634).
///
/// Reused when the next frame's effects match the cached ones **or differ only in a
/// parameter a live node can take** (see [`param_update`]), and the input dimensions
/// are unchanged — a node sized to the frame (e.g. `OverlayNode`) or the read-back size
/// would otherwise be stale.
///
/// `effects` is what the graph's nodes currently hold, not what they were built with:
/// a parameter pushed into a live node updates it here too, so the next comparison is
/// against reality.
struct CachedEffectGraph {
    effects: Vec<GpuEffect>,
    graph: RenderGraph,
    in_w: u32,
    in_h: u32,
    out_w: u32,
    out_h: u32,
}

/// How a cached graph may be reused for the next frame's effect list.
enum Reuse {
    /// The lists are identical; run the graph as it stands.
    AsIs,
    /// They differ only in parameters live nodes take; apply these first.
    WithParams(Vec<NodeParam>),
}

/// Whether `next` can run on a graph built for `cached`, and with what updates.
///
/// The comparison is *explicit* about which parameters may differ rather than loose:
/// anything a node bakes in at build time (a mask sized to the frame, a LUT, the
/// sub-frame count) must force a rebuild, or the graph would be reused with a stale
/// bake (RK-025). Only parameters with a [`NodeParam`] — which a node applies to
/// itself, keeping whatever state it carries — are allowed to differ.
fn param_update(cached: &[GpuEffect], next: &[GpuEffect]) -> Option<Reuse> {
    if cached.len() != next.len() {
        return None;
    }
    let mut params = Vec::new();
    for (was, now) in cached.iter().zip(next) {
        match (was, now) {
            (
                GpuEffect::MotionBlur {
                    shutter_angle: old,
                    sub_frames: old_sub,
                },
                GpuEffect::MotionBlur {
                    shutter_angle: new,
                    sub_frames: new_sub,
                },
            ) if old_sub == new_sub => {
                // The trail lives in the node, so the shutter travels to it rather
                // than the node being rebuilt around the new value (#1705).
                //
                // Compared bit-exactly rather than against a tolerance: the cached
                // list is meant to be what the nodes *hold*, and skipping a
                // below-tolerance change would record a value that was never applied.
                if old.to_bits() != new.to_bits() {
                    params.push(NodeParam::MotionBlurShutter(*new));
                }
            }
            (
                GpuEffect::ShapeMask {
                    x,
                    y,
                    width,
                    height,
                    invert,
                },
                GpuEffect::ShapeMask {
                    x: nx,
                    y: ny,
                    width: nw,
                    height: nh,
                    invert: ninv,
                },
            ) => {
                // The shader evaluates the rectangle, so every field of it is a
                // parameter rather than something baked into the node at build time.
                if (x, y, width, height, invert) != (nx, ny, nw, nh, ninv) {
                    params.push(NodeParam::ShapeMaskRect {
                        x: *nx,
                        y: *ny,
                        width: *nw,
                        height: *nh,
                        invert: *ninv,
                    });
                }
            }
            _ if was == now => {}
            _ => return None,
        }
    }
    if params.is_empty() {
        Some(Reuse::AsIs)
    } else {
        Some(Reuse::WithParams(params))
    }
}

/// Composites derived layers on the GPU, returning `None` (CPU fallback) on
/// unsupported content or any GPU error.
pub struct GpuCompositor {
    ctx: Arc<RenderContext>,
    /// Compositor cached for its target canvas; rebuilt when the canvas changes.
    compositor: Option<(Compositor, (u32, u32))>,
    /// Per-layer effect-graph cache, keyed by `(layer count, layer position)`.
    ///
    /// Being a map at all is what fixes #1770: this was a `Vec` resized per
    /// composite, so any change in the layer count cleared every entry — twice per
    /// output frame on the multi-track export path, which alternates a one-layer solo
    /// composite with an N-layer stack.
    ///
    /// The layer count is in the key because a position means something different
    /// under a different stack, and a *stateful* node's state would otherwise be
    /// shared between two layers that merely happen to sit at the same index with the
    /// same effects. No arrangement in the tree exhibits that today (the export
    /// path's solo composite and stack pass do not collide, and preview composites a
    /// stable stack), and mutation injection confirms no test detects its removal.
    /// It is kept because the cost is a tuple and the failure it prevents is a silent
    /// one, not because a test demands it.
    effect_cache: HashMap<(usize, usize), CachedEffectGraph>,
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
                effect_cache: HashMap::new(),
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
    /// layer's placement has no GPU equivalent (a rotated overlay, or one hanging off
    /// the base layer's edge), or a GPU error occurred. The caller must never see a
    /// wrong-but-rendered frame.
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

        // Part of the cache key: a layer position means something different under a
        // different stack, so a graph is only reused within the same layout.
        let count = plan.layers.len();
        let mut processed = Vec::with_capacity(count);
        for (idx, (lp, (_, frame))) in plan.layers.iter().zip(layers.iter()).enumerate() {
            // The preview adapter does not own its frames, so a no-effects layer must
            // clone; `composite_owned` avoids this for the export drain.
            let out = if lp.effects.is_empty() {
                (*frame).clone()
            } else {
                self.apply_effects(count, idx, lp, frame)?
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

        // Part of the cache key: a layer position means something different under a
        // different stack, so a graph is only reused within the same layout.
        let count = plan.layers.len();
        let mut processed = Vec::with_capacity(count);
        for (idx, (lp, (_, frame))) in plan.layers.iter().zip(layers).enumerate() {
            // Owned: a no-effects layer moves its frame in, no clone.
            let out = if lp.effects.is_empty() {
                frame
            } else {
                self.apply_effects(count, idx, lp, &frame)?
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

    /// Drops every cached effect graph, so the next composite rebuilds each layer's
    /// graph from scratch.
    ///
    /// A stateful effect node (e.g. [`MotionBlurNode`], whose exposure trail
    /// accumulates across the frames of one clip) is embedded in the cached graph, so a
    /// caller that composites a sequence of clips at the same layer position must call
    /// this **at each clip boundary** — otherwise the previous clip's accumulated trail
    /// bleeds into the next clip's first frame (RK-025). Stateless effects are
    /// unaffected beyond a one-frame pipeline rebuild.
    pub fn reset_effect_cache(&mut self) {
        self.effect_cache.clear();
    }

    /// Applies a layer's mappable effects to its rgba frame via a `RenderGraph`, reusing
    /// the layer's cached graph when the effect list is unchanged and cacheable. `None`
    /// on a GPU error. Must not be called for an empty effect list (the caller handles
    /// that).
    fn apply_effects(
        &mut self,
        layer_count: usize,
        layer_idx: usize,
        plan: &GpuLayerPlan,
        frame: &VideoFrame,
    ) -> Option<VideoFrame> {
        let (in_w, in_h) = (frame.width(), frame.height());
        let rgba = frame.to_rgba()?;
        let key = (layer_count, layer_idx);

        // Reuse the cached graph when the input dimensions match (a node sized to the
        // frame or the read-back size would otherwise be stale) and the new effect
        // list either equals the cached one or differs only in parameters a live node
        // takes (see `param_update`).
        if let Some(cached) = self.effect_cache.get_mut(&key)
            && (cached.in_w, cached.in_h) == (in_w, in_h)
            && let Some(reuse) = param_update(&cached.effects, &plan.effects)
        {
            let applied = match reuse {
                Reuse::AsIs => true,
                Reuse::WithParams(params) => params
                    .into_iter()
                    .all(|param| cached.graph.set_param(param) > 0),
            };
            if applied {
                // The nodes now hold the new values, so record them: the next frame
                // has to be compared against what is in the graph, not against what
                // it was built with.
                cached.effects.clone_from(&plan.effects);
                let out = cached.graph.process_gpu(&rgba, in_w, in_h).ok()?;
                return VideoFrame::from_rgba(cached.out_w, cached.out_h, out).ok();
            }
            // A node declined a parameter `param_update` expected it to take. That
            // means the two are out of step, so rebuild rather than run a graph whose
            // contents are not what the comparison assumed.
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
                    let node = ScaleNode::new(*width, *height, *algorithm);
                    // Ask the node what it will actually produce rather than
                    // assuming `width` x `height`: `target_size` passes the input
                    // size through when either dimension is `0`, matching FFmpeg's
                    // `scale=0:0`. Recording the literal `0` instead would wrap the
                    // read-back at the wrong size, `VideoFrame::from_rgba` would
                    // reject the length, and the whole frame would fall back to the
                    // CPU for no reason. `ScaleAnimated` reaches zero on a zoom that
                    // starts from nothing, so this is not hypothetical (#1630).
                    (out_w, out_h) = node.target_size(out_w, out_h);
                    graph.push(node)
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
                // LumaMask multiplies alpha by the frame's own BT.709 luma. The node
                // samples the source frame itself (#1710), so nothing is built here
                // and nothing is uploaded per frame. When LumaMask follows another
                // effect the GPU mask is still the source luma while the CPU `geq`
                // sees the chained frame (a v1 limitation; parity uses it alone).
                GpuEffect::LumaMask { invert } => graph.push(LumaMaskNode::new(*invert)),
                // ShapeMask keeps a rectangle of the source frame. The rectangle is a
                // shader parameter rather than a baked full-frame mask (#1710).
                GpuEffect::ShapeMask {
                    x,
                    y,
                    width,
                    height,
                    invert,
                } => graph.push(ShapeMaskNode::new(*x, *y, *width, *height, *invert)),
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
        // so the next frame reuses it.
        //
        // Every effect is cacheable. There used to be an exclusion for `LumaMask`,
        // because its node baked the source frame's own pixels into a mask and
        // reusing the graph would have applied a stale one (RK-025). The node now
        // samples the source frame per frame instead of baking anything (#1710), so
        // there is nothing left that a reused graph could hold stale. A new node that
        // *does* bake frame content would need that exclusion back.
        let cached = self
            .effect_cache
            .entry(key)
            .insert_entry(CachedEffectGraph {
                effects: plan.effects.clone(),
                graph,
                in_w,
                in_h,
                out_w,
                out_h,
            });
        let out = cached.get().graph.process_gpu(&rgba, in_w, in_h).ok()?;
        VideoFrame::from_rgba(out_w, out_h, out).ok()
    }
}

/// Wraps each processed layer frame in a [`FrameLayer`] with the transform that places
/// it, or `None` when a layer's placement has no GPU equivalent.
///
/// The CPU compositor works in **base-layer space**: layer 0 defines the size, every
/// other layer is stretched to it and overlaid there, and only the finished composite is
/// fitted into the canvas (`composition_inner.rs:1199-1224` and `:1323`). This reproduces
/// that by giving layer 0 the fit and every other layer its placement *composed with* the
/// same fit, so the whole stack lands in one band exactly as the CPU's single composite
/// fit does. That is why the old shared-aspect requirement is gone: an overlay's own
/// aspect never survives the stretch, so it cannot disagree with the base's band.
fn assemble(
    processed: Vec<(VideoFrame, &GpuLayerPlan)>,
    canvas: (u32, u32),
) -> Option<Vec<FrameLayer>> {
    let (first, _) = processed.first()?;
    let base = (first.width(), first.height());
    let base_fit = letterbox_transform(base.0, base.1, canvas)?;
    let transforms = processed
        .iter()
        .enumerate()
        .map(|(idx, (_, lp))| layer_transform(idx, lp, &base_fit, base))
        .collect::<Option<Vec<_>>>()?;
    Some(
        processed
            .into_iter()
            .zip(transforms)
            .map(|((frame, lp), transform)| FrameLayer {
                frame,
                transform,
                blend_mode: lp.blend_mode,
                composite_op: lp.composite_op,
                opacity: lp.opacity,
                z_order: lp.z_order,
            })
            .collect(),
    )
}

/// The [`LayerTransform`] that places layer `index`, or `None` when its placement has no
/// GPU equivalent.
///
/// The rule is the CPU compositor's, **measured** rather than read off the model, because
/// the two do not say the same thing:
///
/// * **Layer 0 is the base.** Its `x` / `y` / `scale` / `rotation` are *ignored* — the CPU
///   builds no scale, rotate or overlay node for it (`composition_inner.rs:1181-1197`), so
///   it only ever receives the fit. Measured: a lone layer given `x=10, y=4` or
///   `scale=0.5` renders byte-identically to the same layer left alone. Applying the
///   transform here would make the GPU diverge from the correctness reference, so the fix
///   for a layer that carries one is to stop *falling back*, not to start drawing it.
///   Whether the model should honour a base transform at all is #1766.
/// * **Every other layer** is stretched to the base's size (its own aspect does not
///   survive), scaled by `base * (scale_x, scale_y)`, and overlaid with its **top-left**
///   at `(x, y)` in base-layer pixels. The finished composite is then fitted into the
///   canvas, which is `base_fit`.
///
/// Composing those: the layer covers `scale` of the base, its centre sits at
/// `x/bw + scale_x/2` in base UV, and `base_fit` maps base UV `u` to canvas UV
/// `0.5 + fit * (u - 0.5)`. `transform.wgsl` puts a layer's centre at
/// `0.5 + scale * translate`, so the fit cancels out of the translate and only multiplies
/// the scale.
///
/// Verified against the CPU on three fixtures, each pinning a different term:
///
/// * a 64x64 overlay at `(10, 4)` scaled `0.5` over a 64x64 base in a 64x64 canvas lands
///   at `(10, 4)..(41, 35)` — the offset and the scale;
/// * the same overlay scaled `0.5` with no offset over a **64x32** base lands at
///   `(0, 16)..(31, 31)` — that the multiplier is against the *base*, not the canvas;
/// * that overlay at `(10, 4)` over the 64x32 base lands at `(10, 20)..(41, 35)` — that
///   the *offset* is against the base too, which neither of the first two can tell apart
///   (a mutation check found this one missing).
///
/// Rotation on a non-base layer returns `None`: the CPU's `rotate` fills the corners it
/// exposes with `fillcolor` while the GPU transform leaves them transparent, so there is
/// nothing to map it to (RK-020).
fn layer_transform(
    index: usize,
    lp: &GpuLayerPlan,
    base_fit: &LayerTransform,
    base: (u32, u32),
) -> Option<LayerTransform> {
    if index == 0 {
        return Some(base_fit.clone());
    }
    if lp.rotation.abs() > 1e-6 {
        return None;
    }
    let (bw, bh) = (px(base.0), px(base.1));
    // A zero-sized base has no space to place into, and a non-positive scale has no
    // extent to draw; either would divide by zero below.
    if bw <= 0.0 || bh <= 0.0 || lp.scale_x <= 0.0 || lp.scale_y <= 0.0 {
        return None;
    }
    // The CPU's `overlay` writes into the base-sized accumulator, so a layer hanging off
    // its edge is **clipped** before the composite is fitted into the canvas. The GPU
    // draws straight into the canvas and has no such bound, so it would let the layer
    // spill outside the base's band -- picture where the CPU has none. Reject instead of
    // rendering the spill (RK-020); the cost is a fallback for a partly-offscreen
    // overlay, which is the same answer as before #1633 for every overlay.
    let (ox, oy) = (lp.x / bw, lp.y / bh);
    if ox < -1e-6 || oy < -1e-6 || ox + lp.scale_x > 1.0 + 1e-6 || oy + lp.scale_y > 1.0 + 1e-6 {
        return None;
    }
    Some(LayerTransform {
        // `base_fit`'s translate moves the whole base, so anything sitting on the base
        // moves with it. `letterbox_transform` returns a centred fit (translate 0) today,
        // which is why the term reads as dead -- but leaving it out would make that a
        // silent precondition of this formula, and the two functions live in one file.
        // With it, a fit that ever gains an offset carries its layers along.
        x: (lp.x / bw + lp.scale_x / 2.0 - 0.5 + base_fit.x) / lp.scale_x,
        y: (lp.y / bh + lp.scale_y / 2.0 - 0.5 + base_fit.y) / lp.scale_y,
        scale_x: base_fit.scale_x * lp.scale_x,
        scale_y: base_fit.scale_y * lp.scale_y,
        rotation: 0.0,
    })
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

/// A pixel count as an `f32`, for the base-space arithmetic in [`layer_transform`].
///
/// Distinct from [`ratio`]: `ratio(x, 1)` computes the same number but reads as a
/// proportion, which this is not.
#[allow(clippy::cast_precision_loss)] // pixel dimensions are far inside f32's exact range
fn px(v: u32) -> f32 {
    v as f32
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan layer with the given placement and nothing else.
    fn plan_layer(x: f32, y: f32, scale: (f32, f32), rotation: f32) -> GpuLayerPlan {
        GpuLayerPlan {
            z_order: 0,
            x,
            y,
            scale_x: scale.0,
            scale_y: scale.1,
            rotation,
            opacity: 1.0,
            blend_mode: ff_render::BlendMode::Normal,
            composite_op: ff_render::CompositeOp::Over,
            effects: Vec::new(),
        }
    }

    /// The canvas pixel box a [`LayerTransform`] draws into: `(x0, y0, x1, y1)`.
    ///
    /// Inverts `transform.wgsl`'s mapping — a layer's centre lands at
    /// `0.5 + scale * translate` and it covers `scale` of the canvas — so a test can be
    /// written against the pixel box the CPU was *measured* to produce rather than
    /// against the transform's own numbers, which would just restate the formula.
    fn canvas_box(t: &LayerTransform, canvas: (u32, u32)) -> (f32, f32, f32, f32) {
        let (cw, ch) = (ratio(canvas.0, 1), ratio(canvas.1, 1));
        let cx = 0.5 + t.scale_x * t.x;
        let cy = 0.5 + t.scale_y * t.y;
        (
            (cx - t.scale_x / 2.0) * cw,
            (cy - t.scale_y / 2.0) * ch,
            (cx + t.scale_x / 2.0) * cw,
            (cy + t.scale_y / 2.0) * ch,
        )
    }

    fn assert_box(got: (f32, f32, f32, f32), want: (f32, f32, f32, f32), what: &str) {
        for (g, w) in [
            (got.0, want.0),
            (got.1, want.1),
            (got.2, want.2),
            (got.3, want.3),
        ] {
            assert!((g - w).abs() < 0.01, "{what}: got {got:?}, want {want:?}");
        }
    }

    #[test]
    fn layer_transform_should_ignore_the_base_layers_own_transform() {
        // Measured: a lone layer given x=10, y=4 or scale=0.5 renders byte-identically
        // to the same layer left alone, because the CPU builds no scale/rotate/overlay
        // node for layer 0. So the base must get the fit and nothing else — applying its
        // transform is what would diverge (#1633).
        let fit = letterbox_transform(64, 32, (64, 64)).expect("a 2:1 fit has a band");
        let moved = plan_layer(10.0, 4.0, (0.5, 0.25), 30.0);
        let got = layer_transform(0, &moved, &fit, (64, 32)).expect("the base always places");
        assert_box(
            canvas_box(&got, (64, 64)),
            canvas_box(&fit, (64, 64)),
            "a base layer's own transform must not move it",
        );
    }

    #[test]
    fn layer_transform_should_place_an_overlay_at_the_measured_box() {
        // Measured against `RealtimeComposer`: a 64x64 overlay at (10, 4) scaled 0.5 over
        // a 64x64 base in a 64x64 canvas lit exactly (10, 4)..(41, 35) inclusive, i.e.
        // the half-open box (10, 4)..(42, 36).
        let fit = letterbox_transform(64, 64, (64, 64)).expect("a square fit is the identity");
        let over = plan_layer(10.0, 4.0, (0.5, 0.5), 0.0);
        let got = layer_transform(1, &over, &fit, (64, 64)).expect("an overlay places");
        assert_box(
            canvas_box(&got, (64, 64)),
            (10.0, 4.0, 42.0, 36.0),
            "overlay placement",
        );
    }

    #[test]
    fn layer_transform_should_scale_against_the_base_not_the_canvas() {
        // The fixture that distinguishes the two readings of the multiplier. Measured:
        // the same 0.5-scaled overlay over a **64x32** base in a 64x64 canvas lit
        // (0, 16)..(31, 31) inclusive — half-open (0, 16)..(32, 32). Against the *canvas*
        // it would have been 32 tall, not 16, and would not sit inside the base's band.
        let fit = letterbox_transform(64, 32, (64, 64)).expect("a 2:1 fit has a band");
        let over = plan_layer(0.0, 0.0, (0.5, 0.5), 0.0);
        let got = layer_transform(1, &over, &fit, (64, 32)).expect("an overlay places");
        assert_box(
            canvas_box(&got, (64, 64)),
            (0.0, 16.0, 32.0, 32.0),
            "the multiplier is against the base size",
        );
    }

    #[test]
    fn layer_transform_should_reject_a_rotated_overlay() {
        // The CPU's `rotate` fills the exposed corners with `fillcolor`; the GPU leaves
        // them transparent. Nothing to map it to, so it falls back (RK-020).
        let fit = letterbox_transform(64, 64, (64, 64)).expect("a square fit is the identity");
        let spun = plan_layer(0.0, 0.0, (1.0, 1.0), 30.0);
        assert!(layer_transform(1, &spun, &fit, (64, 64)).is_none());
        // ... but the same rotation on the base is ignored, not a fallback: measured, the
        // CPU renders a rotated lone layer unrotated.
        assert!(layer_transform(0, &spun, &fit, (64, 64)).is_some());
    }

    #[test]
    fn layer_transform_should_carry_a_base_fit_offset_through_to_its_overlays() {
        // `letterbox_transform` returns a centred fit today, so no other test moves an
        // overlay by way of the base. Without this, dropping `base_fit.x` from the
        // formula would pass every test and only break when the fit gains an offset --
        // exactly the kind of unstated precondition a formula should not carry.
        let centred = LayerTransform {
            scale_x: 0.5,
            scale_y: 0.5,
            ..LayerTransform::default()
        };
        let shifted = LayerTransform {
            x: 0.25,
            ..centred.clone()
        };
        let over = plan_layer(0.0, 0.0, (1.0, 1.0), 0.0);
        let a = layer_transform(1, &over, &centred, (64, 64)).expect("places");
        let b = layer_transform(1, &over, &shifted, (64, 64)).expect("places");
        let (ax, _, _, _) = canvas_box(&a, (64, 64));
        let (bx, _, _, _) = canvas_box(&b, (64, 64));
        // The base moved right by `scale_x * 0.25` of the canvas = 8 px; so must the overlay.
        assert!(
            (bx - ax - 8.0).abs() < 0.01,
            "an overlay must follow the base's fit offset: {ax} -> {bx}"
        );
    }

    #[test]
    fn layer_transform_should_reject_an_overlay_that_spills_outside_the_base() {
        // `overlay` clips to the base-sized accumulator on the CPU; the GPU would draw
        // the overhang into the canvas. Found by a mutation check: no parity fixture had
        // a layer crossing the base edge, so nothing was pinning this.
        let fit = letterbox_transform(64, 64, (64, 64)).expect("a square fit is the identity");
        let inside = plan_layer(10.0, 4.0, (0.5, 0.5), 0.0);
        assert!(layer_transform(1, &inside, &fit, (64, 64)).is_some());
        // Off the right edge: 10 + 0.9 * 64 > 64.
        let over_right = plan_layer(10.0, 0.0, (0.9, 0.5), 0.0);
        assert!(layer_transform(1, &over_right, &fit, (64, 64)).is_none());
        // Off the bottom, and off the top-left.
        assert!(
            layer_transform(1, &plan_layer(0.0, 40.0, (0.5, 0.5), 0.0), &fit, (64, 64)).is_none()
        );
        assert!(
            layer_transform(1, &plan_layer(-1.0, 0.0, (0.5, 0.5), 0.0), &fit, (64, 64)).is_none()
        );
        // Exactly flush with the edges is inside, not a spill.
        assert!(
            layer_transform(1, &plan_layer(32.0, 32.0, (0.5, 0.5), 0.0), &fit, (64, 64)).is_some()
        );
    }

    #[test]
    fn layer_transform_should_reject_a_degenerate_overlay() {
        let fit = letterbox_transform(64, 64, (64, 64)).expect("a square fit is the identity");
        // A non-positive scale has no extent to draw, and would divide by zero.
        assert!(
            layer_transform(1, &plan_layer(0.0, 0.0, (0.0, 1.0), 0.0), &fit, (64, 64)).is_none()
        );
        assert!(
            layer_transform(1, &plan_layer(0.0, 0.0, (1.0, -1.0), 0.0), &fit, (64, 64)).is_none()
        );
        // A zero-sized base has no space to place into.
        assert!(
            layer_transform(1, &plan_layer(0.0, 0.0, (1.0, 1.0), 0.0), &fit, (0, 64)).is_none()
        );
    }

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

    /// A `ShapeMask` whose rectangle differs must still reuse the cached graph, or an
    /// animated rectangle would rebuild the whole effect chain every frame (#1710).
    #[test]
    fn a_moved_rectangle_should_reuse_the_graph_with_a_parameter() {
        let was = [GpuEffect::ShapeMask {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            invert: false,
        }];
        let now = [GpuEffect::ShapeMask {
            x: 5,
            y: 2,
            width: 10,
            height: 10,
            invert: true,
        }];
        let Some(Reuse::WithParams(params)) = param_update(&was, &now) else {
            panic!("a moved rectangle must reuse the cached graph");
        };
        assert!(
            matches!(
                params.as_slice(),
                [NodeParam::ShapeMaskRect {
                    x: 5,
                    y: 2,
                    width: 10,
                    height: 10,
                    invert: true,
                }]
            ),
            "the whole rectangle must travel to the live node, got {params:?}"
        );
    }

    /// The other side of the same gate: an unchanged rectangle must not push a
    /// parameter it does not need.
    #[test]
    fn an_unchanged_rectangle_should_reuse_the_graph_as_is() {
        let effects = [GpuEffect::ShapeMask {
            x: 5,
            y: 2,
            width: 10,
            height: 10,
            invert: false,
        }];
        assert!(
            matches!(param_update(&effects, &effects), Some(Reuse::AsIs)),
            "an unchanged rectangle must run the graph as it stands"
        );
    }

    /// `LumaMask` used to be excluded from the cache because its node baked the source
    /// frame's own pixels. The shader samples the frame instead now, so it caches like
    /// any other effect (#1710).
    #[test]
    fn a_luma_mask_should_be_cacheable() {
        let effects = [GpuEffect::LumaMask { invert: false }];
        assert!(
            matches!(param_update(&effects, &effects), Some(Reuse::AsIs)),
            "a luma mask must reuse its cached graph"
        );
    }
}
