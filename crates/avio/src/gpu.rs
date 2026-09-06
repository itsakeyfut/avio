//! Mapping from the derived scene to an `ff-render` GPU plan (bridge Br2, #1625).
//!
//! This is the pure, device-free half of the GPU compositing bridge (ADR-0007,
//! `docs/specs/gpu-compositing-bridge.md`): it translates avio's derived per-frame
//! layer set into a [`GpuScenePlan`] describing how to composite the frame on the
//! GPU, or reports a [`GpuMapping::Fallback`] when the frame contains anything the
//! `ff-render` node set does not cover. It does **not** touch a GPU: building the
//! actual `ff_render::Compositor` / `RenderGraph` from a plan and running it against
//! decoded frames is the preview (Br3) and export (Br4) work.
//!
//! The mapping is the single source of truth shared by both paths: it is written
//! once over [`GpuLayerSource`], implemented for the export `VideoLayer` and the
//! preview `RealtimeLayerDescriptor`.
//!
//! Fallback is **whole-frame**: if any layer carries an unsupported blend mode,
//! composite op, or effect step, the entire frame falls back to the existing CPU
//! compositor. The mapping never silently drops a step.
//!
//! `classify_step` is the authority on what is covered — the node-coverage table in
//! `docs/specs/gpu-compositing-bridge.md` describes it, but read the code when the
//! two disagree.

use std::time::Duration;

use ff_filter::{
    AnimatedValue, BlendMode, CompositeOp, FilterStep, RealtimeLayer, RealtimeLayerDescriptor,
    ScaleAlgorithm, VideoLayer,
};
use ff_render::{
    BlendMode as RenderBlendMode, CompositeOp as RenderCompositeOp,
    ScaleAlgorithm as RenderScaleAlgorithm,
};

/// A derived layer the GPU mapping can read. Implemented for the export
/// [`VideoLayer`], the preview [`RealtimeLayerDescriptor`], the runner's realized
/// [`RealtimeLayer`], and `&T` for any of them (so a `&[&RealtimeLayer]` maps like a
/// `&[RealtimeLayer]`), keeping the mapping one shared implementation.
pub trait GpuLayerSource {
    /// Per-clip opacity animation.
    fn opacity(&self) -> &AnimatedValue<f64>;
    /// Horizontal position animation.
    fn x(&self) -> &AnimatedValue<f64>;
    /// Vertical position animation.
    fn y(&self) -> &AnimatedValue<f64>;
    /// Horizontal scale animation.
    fn scale_x(&self) -> &AnimatedValue<f64>;
    /// Vertical scale animation.
    fn scale_y(&self) -> &AnimatedValue<f64>;
    /// Rotation animation.
    fn rotation(&self) -> &AnimatedValue<f64>;
    /// Layer-to-layer blend mode.
    fn blend_mode(&self) -> BlendMode;
    /// Porter-Duff composite operator.
    fn composite_op(&self) -> CompositeOp;
    /// Ordered per-clip effect chain.
    fn effects(&self) -> &[FilterStep];
}

impl GpuLayerSource for VideoLayer {
    fn opacity(&self) -> &AnimatedValue<f64> {
        &self.opacity
    }
    fn x(&self) -> &AnimatedValue<f64> {
        &self.x
    }
    fn y(&self) -> &AnimatedValue<f64> {
        &self.y
    }
    fn scale_x(&self) -> &AnimatedValue<f64> {
        &self.scale_x
    }
    fn scale_y(&self) -> &AnimatedValue<f64> {
        &self.scale_y
    }
    fn rotation(&self) -> &AnimatedValue<f64> {
        &self.rotation
    }
    fn blend_mode(&self) -> BlendMode {
        self.blend_mode
    }
    fn composite_op(&self) -> CompositeOp {
        self.composite_op
    }
    fn effects(&self) -> &[FilterStep] {
        &self.effects
    }
}

impl GpuLayerSource for RealtimeLayerDescriptor {
    fn opacity(&self) -> &AnimatedValue<f64> {
        &self.opacity
    }
    fn x(&self) -> &AnimatedValue<f64> {
        &self.x
    }
    fn y(&self) -> &AnimatedValue<f64> {
        &self.y
    }
    fn scale_x(&self) -> &AnimatedValue<f64> {
        &self.scale_x
    }
    fn scale_y(&self) -> &AnimatedValue<f64> {
        &self.scale_y
    }
    fn rotation(&self) -> &AnimatedValue<f64> {
        &self.rotation
    }
    fn blend_mode(&self) -> BlendMode {
        self.blend_mode
    }
    fn composite_op(&self) -> CompositeOp {
        self.composite_op
    }
    fn effects(&self) -> &[FilterStep] {
        &self.effects
    }
}

impl GpuLayerSource for RealtimeLayer {
    fn opacity(&self) -> &AnimatedValue<f64> {
        &self.opacity
    }
    fn x(&self) -> &AnimatedValue<f64> {
        &self.x
    }
    fn y(&self) -> &AnimatedValue<f64> {
        &self.y
    }
    fn scale_x(&self) -> &AnimatedValue<f64> {
        &self.scale_x
    }
    fn scale_y(&self) -> &AnimatedValue<f64> {
        &self.scale_y
    }
    fn rotation(&self) -> &AnimatedValue<f64> {
        &self.rotation
    }
    fn blend_mode(&self) -> BlendMode {
        self.blend_mode
    }
    fn composite_op(&self) -> CompositeOp {
        self.composite_op
    }
    fn effects(&self) -> &[FilterStep] {
        &self.effects
    }
}

// So a `&[&L]` (e.g. the preview runner's borrowed layers) maps like a `&[L]`.
impl<T: GpuLayerSource + ?Sized> GpuLayerSource for &T {
    fn opacity(&self) -> &AnimatedValue<f64> {
        (**self).opacity()
    }
    fn x(&self) -> &AnimatedValue<f64> {
        (**self).x()
    }
    fn y(&self) -> &AnimatedValue<f64> {
        (**self).y()
    }
    fn scale_x(&self) -> &AnimatedValue<f64> {
        (**self).scale_x()
    }
    fn scale_y(&self) -> &AnimatedValue<f64> {
        (**self).scale_y()
    }
    fn rotation(&self) -> &AnimatedValue<f64> {
        (**self).rotation()
    }
    fn blend_mode(&self) -> BlendMode {
        (**self).blend_mode()
    }
    fn composite_op(&self) -> CompositeOp {
        (**self).composite_op()
    }
    fn effects(&self) -> &[FilterStep] {
        (**self).effects()
    }
}

/// A per-layer effect the GPU path can apply, mapped from a [`FilterStep`].
/// `#[non_exhaustive]`: more kinds are added as node coverage grows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GpuEffect {
    /// `ff_render::ColorGradeNode` (brightness / contrast / saturation / temperature /
    /// tint). Temperature and tint come from the typed `ColorCorrect` effect and are a
    /// GPU-only enrichment (the CPU `eq` fallback does not apply them).
    ColorGrade {
        /// Brightness offset.
        brightness: f32,
        /// Contrast multiplier.
        contrast: f32,
        /// Saturation multiplier.
        saturation: f32,
        /// Colour temperature.
        temperature: f32,
        /// Colour tint.
        tint: f32,
    },
    /// `ff_render::ScaleNode` (resize to a fixed target).
    Scale {
        /// Target width in pixels.
        width: u32,
        /// Target height in pixels.
        height: u32,
        /// Resampling algorithm.
        algorithm: RenderScaleAlgorithm,
    },
    /// `ff_render::GaussianBlurNode` (two-pass separable Gaussian blur).
    Blur {
        /// Gaussian standard deviation (blur radius) in pixels; the same value the
        /// `gblur` filter uses on the CPU path.
        sigma: f32,
    },
    /// `ff_render::SharpenNode` (unsharp-mask sharpen).
    Sharpen {
        /// Unsharp-mask blur radius (sigma). Fixed to `DEFAULT_SHARPEN_RADIUS`:
        /// the `unsharp` filter carries no radius (a fixed 5×5 mask), so the GPU
        /// node uses a constant that approximates it.
        radius: f32,
        /// Sharpening amount (the `unsharp` luma amount).
        strength: f32,
    },
    /// `ff_render::VignetteNode` (radial darkening).
    Vignette {
        /// Normalised distance where darkening begins. Fixed to
        /// `DEFAULT_VIGNETTE_RADIUS` (the `vignette` filter has no radius option).
        radius: f32,
        /// Maximum darkening at the corners (the mapped `vignette` amount).
        strength: f32,
        /// Falloff width. Fixed to `DEFAULT_VIGNETTE_FEATHER`.
        feather: f32,
    },
    /// `ff_render::FilmGrainNode` (temporal grain).
    FilmGrain {
        /// Luma grain amplitude (the `noise` strength mapped to the node's scale).
        luma_strength: f32,
        /// Chroma grain amplitude (the `noise` strength mapped to the node's scale).
        chroma_strength: f32,
        /// Per-frame seed. Derived from the frame time so the grain varies each frame
        /// (the CPU path uses the `noise` filter's own `allf=t` temporal seed).
        frame_index: u32,
    },
    /// `ff_render::GlowNode` (three-pass bloom).
    Glow {
        /// Luminance threshold for highlight extraction.
        threshold: f32,
        /// Gaussian blur radius (sigma) for the glow spread; the node clamps it to its
        /// blur range (`[0.5, 20.0]`), so a larger `noise`/CPU radius spreads further.
        radius: f32,
        /// Additive blend weight of the glow layer.
        intensity: f32,
    },
    /// `ff_render::ColorWheelsNode` (three-way lift/gamma/gain corrector).
    ColorWheels {
        /// Shadows lift (additive, neutral 0.0) per channel `[R, G, B]`.
        shadows_lift: [f32; 3],
        /// Midtones gamma (neutral 1.0) per channel `[R, G, B]`.
        midtones_gamma: [f32; 3],
        /// Highlights gain (neutral 1.0) per channel `[R, G, B]`.
        highlights_gain: [f32; 3],
    },
    /// `ff_render::CurvesNode` (per-channel tone curves).
    Curves {
        /// Master curve control points `[input, output]` (applied to every channel).
        master: Vec<[f32; 2]>,
        /// Red channel curve control points.
        red: Vec<[f32; 2]>,
        /// Green channel curve control points.
        green: Vec<[f32; 2]>,
        /// Blue channel curve control points.
        blue: Vec<[f32; 2]>,
    },
    /// `ff_render::HslNode` (hue / saturation / lightness adjustment).
    Hsl {
        /// Hue rotation in degrees.
        hue_shift: f32,
        /// Saturation multiplier (neutral `1.0`).
        saturation: f32,
        /// Lightness offset (neutral `0.0`).
        lightness: f32,
    },
    /// `ff_render::LutNode` (3D colour LUT loaded from a `.cube` / `.3dl` file).
    Lut {
        /// Path to the LUT file the compositor loads.
        path: String,
    },
    /// `ff_render::ChromaKeyNode` (green-screen keying by chroma distance).
    ChromaKey {
        /// Key colour in RGB, each channel `0.0..=1.0`.
        key_color: [f32; 3],
        /// Chroma-distance threshold (the `chromakey` `similarity`).
        tolerance: f32,
        /// Edge softness (the `chromakey` `blend`).
        softness: f32,
    },
    /// `ff_render::LumaMaskNode` (alpha *= the frame's own BT.709 luma). The
    /// compositor builds the mask from the frame itself when applying this.
    LumaMask {
        /// When `true`, mask by `1 - luma` (dark pixels stay opaque).
        invert: bool,
    },
    /// `ff_render::ShapeMaskNode` (a rectangular alpha mask). The compositor builds
    /// the rectangle mask from these pixel bounds when applying this.
    ShapeMask {
        /// Left edge of the rectangle, in pixels.
        x: u32,
        /// Top edge of the rectangle, in pixels.
        y: u32,
        /// Rectangle width, in pixels.
        width: u32,
        /// Rectangle height, in pixels.
        height: u32,
        /// When `true`, keep the exterior and clear the interior.
        invert: bool,
    },
    /// `ff_render::MotionBlurNode` (exposure-trail accumulation). This node is
    /// **stateful**: the trail accumulates across frames on one node instance, so the
    /// compositor must reuse its cached graph within a clip and reset it at a clip
    /// boundary (RK-025). The shutter is a constant (motion blur cannot animate the
    /// shutter per frame; see [`EffectKind::MotionBlur`](crate::EffectKind::MotionBlur)).
    MotionBlur {
        /// Shutter angle in degrees (`0.0` = no blur, `180.0` = standard film blur).
        shutter_angle: f32,
        /// Trail-length sub-frame count (the node clamps it to `2..=8`).
        sub_frames: u8,
    },
}

/// Blur radius (sigma) the GPU [`ff_render::SharpenNode`] uses. `FFmpeg` `unsharp`
/// has no radius option (a fixed 5×5 luma mask), so the GPU path approximates it
/// with this constant; the parity tolerance absorbs the small kernel difference.
const DEFAULT_SHARPEN_RADIUS: f32 = 1.0;

/// Radius / feather the GPU [`ff_render::VignetteNode`] uses. The `vignette` filter
/// is parameterised only by an `angle` (a `cos^4` falloff), so the GPU node
/// approximates its profile with fixed radius and feather; only the mapped strength
/// varies. The parity tolerance absorbs the profile difference.
const DEFAULT_VIGNETTE_RADIUS: f32 = 0.5;
const DEFAULT_VIGNETTE_FEATHER: f32 = 0.5;

/// Maps the `noise` filter's `[0, 100]` strength to the GPU [`ff_render::FilmGrainNode`]
/// grain amplitude. Calibrated so a given strength produces a comparable grain
/// standard deviation on both paths (the parity test compares that, not pixels).
const NODE_GRAIN_SCALE: f32 = 0.0042;

/// One layer of a [`GpuScenePlan`]: the transform / blend / opacity the
/// `ff_render::Compositor` needs, plus the per-layer effect nodes to run before
/// compositing. The transform is stored as scalars (evaluated at the frame time);
/// Br3/Br4 build the `ff_render::LayerTransform` from them.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuLayerPlan {
    /// Compositing order (bottom to top); equals the layer's index.
    pub z_order: i32,
    // These carry the model's units unchanged: `x`/`y` are canvas **pixels** and
    // `rotation` is **clockwise degrees**. `ff_render::LayerTransform` is UV-space and
    // counter-clockwise radians, so an executor must convert (or fall back) rather
    // than feed these straight in (see docs/specs/gpu-compositing-bridge.md).
    /// Horizontal offset, in canvas pixels (model units).
    pub x: f32,
    /// Vertical offset, in canvas pixels (model units).
    pub y: f32,
    /// Horizontal scale factor (`1.0` = no change).
    pub scale_x: f32,
    /// Vertical scale factor (`1.0` = no change).
    pub scale_y: f32,
    /// Rotation in clockwise degrees (model units).
    pub rotation: f32,
    /// Layer opacity in `[0.0, 1.0]`.
    pub opacity: f32,
    /// Blend mode against the layers below.
    pub blend_mode: RenderBlendMode,
    /// Porter-Duff operator for this layer. The blend mode above is only
    /// meaningful with `Over`; for any other operator the model does not apply
    /// it, so `map_scene` sets `Normal` (#1670).
    pub composite_op: RenderCompositeOp,
    /// Per-layer effects applied to the source frame before compositing.
    pub effects: Vec<GpuEffect>,
}

/// A whole frame's GPU compositing plan: the output canvas and the z-ordered layers.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuScenePlan {
    /// Output canvas `(width, height)`.
    pub canvas: (u32, u32),
    /// Layers in compositing order (bottom to top).
    pub layers: Vec<GpuLayerPlan>,
}

/// Why a frame cannot composite on the GPU and falls back to the CPU compositor.
/// `#[non_exhaustive]`: more reasons appear as coverage changes.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GpuFallback {
    /// A blend mode with no `ff-render` equivalent.
    UnsupportedBlendMode(BlendMode),
    /// A composite operator other than `Over`.
    UnsupportedCompositeOp(CompositeOp),
    /// An effect step with no GPU node.
    UnsupportedEffect,
}

/// The result of mapping a frame: a GPU plan, or a whole-frame CPU fallback.
#[derive(Debug, Clone, PartialEq)]
pub enum GpuMapping {
    /// The frame composites on the GPU with this plan.
    Gpu(GpuScenePlan),
    /// The frame falls back to the existing CPU compositor (with the reason).
    Fallback(GpuFallback),
}

/// Maps a derived layer set (bottom to top) at frame time `t` to a [`GpuMapping`].
///
/// Returns [`GpuMapping::Gpu`] only when every layer fully maps to the `ff-render`
/// node set; the first unsupported blend mode, composite op, or effect step makes
/// the whole frame a [`GpuMapping::Fallback`] (no step is ever silently dropped).
#[must_use]
pub fn map_scene<L: GpuLayerSource>(layers: &[L], canvas: (u32, u32), t: Duration) -> GpuMapping {
    let mut plan_layers = Vec::with_capacity(layers.len());
    for (i, layer) in layers.iter().enumerate() {
        let Some(composite_op) = map_composite_op(layer.composite_op()) else {
            return GpuMapping::Fallback(GpuFallback::UnsupportedCompositeOp(layer.composite_op()));
        };
        // The blend mode is only applied when the composite op is `Over` (see
        // `VideoLayer` and `composition_inner.rs`), so any other operator drops
        // it rather than falling back, matching the CPU path (#1670).
        let blend_mode = if matches!(composite_op, RenderCompositeOp::Over) {
            let Some(mode) = map_blend_mode(layer.blend_mode()) else {
                return GpuMapping::Fallback(GpuFallback::UnsupportedBlendMode(layer.blend_mode()));
            };
            mode
        } else {
            RenderBlendMode::Normal
        };

        let mut effects = Vec::new();
        // Rotation contributed by the effect chain (`RotateAnimated`), which is
        // **added** to the layer scalar rather than replacing it: on the CPU both
        // apply — the compositor rotates the layer from `layer.rotation` and the
        // effect chain rotates again — and where `derive` neutralized the scalar
        // (ADR-0005) the sum degrades to the step's own angle.
        let mut step_rotation = 0.0_f32;
        for step in layer.effects() {
            match classify_step(step, t) {
                StepClass::Skip => {}
                StepClass::Effect(effect) => effects.push(effect),
                StepClass::Rotation(degrees) => step_rotation += degrees,
                StepClass::Unsupported => {
                    return GpuMapping::Fallback(GpuFallback::UnsupportedEffect);
                }
            }
        }

        plan_layers.push(GpuLayerPlan {
            // Layer count fits an i32 in every real timeline; saturate defensively.
            z_order: i32::try_from(i).unwrap_or(i32::MAX),
            x: eval_at(layer.x(), t),
            y: eval_at(layer.y(), t),
            scale_x: eval_at(layer.scale_x(), t),
            scale_y: eval_at(layer.scale_y(), t),
            rotation: eval_at(layer.rotation(), t) + step_rotation,
            opacity: eval_at(layer.opacity(), t).clamp(0.0, 1.0),
            blend_mode,
            composite_op,
            effects,
        });
    }
    GpuMapping::Gpu(GpuScenePlan {
        canvas,
        layers: plan_layers,
    })
}

/// Evaluates an animation track at `t` and narrows to the `f32` the GPU nodes use.
// Transform / colour params are well within `f32` range; the narrowing from the
// model's `f64` is the intended, lossy conversion.
#[allow(clippy::cast_possible_truncation)]
fn eval_at(value: &AnimatedValue<f64>, t: Duration) -> f32 {
    value.value_at(t) as f32
}

/// How an effect step maps: skipped (temporal / handled upstream), a GPU effect, or
/// unsupported (forces whole-frame CPU fallback).
enum StepClass {
    Skip,
    Effect(GpuEffect),
    /// A step that belongs on the layer's transform rather than in its effect
    /// list: clockwise degrees to add to [`GpuLayerPlan::rotation`].
    Rotation(f32),
    Unsupported,
}

// `chroma != 0.0` compares against the neutral sentinel our mapping always emits,
// so an exact float comparison is intended here.
#[allow(clippy::cast_possible_truncation, clippy::float_cmp)]
fn classify_step(step: &FilterStep, t: Duration) -> StepClass {
    match step {
        // Temporal / decode-scheduling steps are applied upstream (the GPU path
        // composites already-decoded frames), so they are not the compositor's
        // concern here.
        FilterStep::Trim { .. }
        | FilterStep::ResetPts
        | FilterStep::OffsetPts { .. }
        | FilterStep::Speed { .. } => StepClass::Skip,
        FilterStep::Eq {
            brightness,
            contrast,
            saturation,
            temperature,
            tint,
        } => StepClass::Effect(GpuEffect::ColorGrade {
            brightness: *brightness,
            contrast: *contrast,
            saturation: *saturation,
            temperature: *temperature,
            tint: *tint,
        }),
        FilterStep::EqAnimated {
            brightness,
            contrast,
            saturation,
            gamma,
            temperature,
            tint,
        } => {
            // ff-render's ColorGradeNode has no gamma, so only an `eq` whose gamma
            // is neutral at `t` maps; otherwise fall back so gamma is not dropped.
            if (gamma.value_at(t) - 1.0).abs() < 1e-6 {
                StepClass::Effect(GpuEffect::ColorGrade {
                    brightness: brightness.value_at(t) as f32,
                    contrast: contrast.value_at(t) as f32,
                    saturation: saturation.value_at(t) as f32,
                    temperature: temperature.value_at(t) as f32,
                    tint: tint.value_at(t) as f32,
                })
            } else {
                StepClass::Unsupported
            }
        }
        FilterStep::Scale {
            width,
            height,
            algorithm,
        } => StepClass::Effect(GpuEffect::Scale {
            width: *width,
            height: *height,
            algorithm: map_scale_algo(*algorithm),
        }),
        // `ScaleAnimated` is the same resize step as `Scale` with a track for each
        // dimension (`derive` emits it under ADR-0005 with the layer scalar
        // neutralized, sized as `canvas * factor`), so it maps to the same node
        // evaluated at the frame time. `map_scene` runs per frame, which is what
        // makes that sound — the same reason the animated blur/sharpen/vignette
        // arms below evaluate at `t`.
        //
        // No zero-size guard: plain `Scale` has none either, and `FFmpeg`'s `scale`
        // reads `0` as "keep the source size". Falling back at zero would invent a
        // divergence the static path does not have.
        FilterStep::ScaleAnimated {
            width,
            height,
            algorithm,
        } => StepClass::Effect(GpuEffect::Scale {
            width: round_u32(width.value_at(t)),
            height: round_u32(height.value_at(t)),
            algorithm: map_scale_algo(*algorithm),
        }),
        // `RotateAnimated` is the rotation `derive` lifted off the layer scalar
        // (ADR-0005), so it goes back onto the transform rather than into the
        // effect list — there is no GPU rotate *node*, rotation is a layer property.
        FilterStep::RotateAnimated { angle, fill_color } => {
            rotate_animated_step(angle.value_at(t), fill_color)
        }
        FilterStep::GBlur { sigma } => StepClass::Effect(GpuEffect::Blur { sigma: *sigma }),
        FilterStep::GBlurAnimated { sigma } => StepClass::Effect(GpuEffect::Blur {
            // The blur node is rebuilt each composite (map_scene runs per frame), so
            // an animated sigma is evaluated at the frame time here.
            sigma: sigma.value_at(t) as f32,
        }),
        // The GPU SharpenNode is luma-only; a non-zero chroma amount cannot be
        // represented, so it falls back rather than silently drop it (RK-020).
        FilterStep::Unsharp {
            luma_strength,
            chroma_strength,
        } => {
            if *chroma_strength != 0.0 {
                StepClass::Unsupported
            } else {
                StepClass::Effect(GpuEffect::Sharpen {
                    radius: DEFAULT_SHARPEN_RADIUS,
                    strength: *luma_strength,
                })
            }
        }
        FilterStep::UnsharpAnimated {
            luma_strength,
            chroma_strength,
        } => {
            if chroma_strength.value_at(t) != 0.0 {
                StepClass::Unsupported
            } else {
                // Rebuilt per frame (map_scene runs per frame), so the animated
                // amount is evaluated at the frame time here.
                StepClass::Effect(GpuEffect::Sharpen {
                    radius: DEFAULT_SHARPEN_RADIUS,
                    strength: luma_strength.value_at(t) as f32,
                })
            }
        }
        // The GPU VignetteNode is centred; an off-centre `vignette` falls back
        // rather than render a differently-placed vignette (RK-020).
        FilterStep::Vignette { angle, x0, y0 } => {
            if *x0 != 0.0 || *y0 != 0.0 {
                StepClass::Unsupported
            } else {
                StepClass::Effect(GpuEffect::Vignette {
                    radius: DEFAULT_VIGNETTE_RADIUS,
                    // The `vignette` angle spans [0, PI/2]; normalise to the node's
                    // [0, 1] strength.
                    strength: (angle / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0),
                    feather: DEFAULT_VIGNETTE_FEATHER,
                })
            }
        }
        FilterStep::VignetteAnimated { amount, x0, y0 } => {
            if *x0 != 0.0 || *y0 != 0.0 {
                StepClass::Unsupported
            } else {
                StepClass::Effect(GpuEffect::Vignette {
                    radius: DEFAULT_VIGNETTE_RADIUS,
                    strength: (amount.value_at(t) as f32).clamp(0.0, 1.0),
                    feather: DEFAULT_VIGNETTE_FEATHER,
                })
            }
        }
        FilterStep::FilmGrain {
            luma_strength,
            chroma_strength,
        } => film_grain_step(*luma_strength, *chroma_strength, t),
        FilterStep::FilmGrainAnimated {
            luma_strength,
            chroma_strength,
        } => film_grain_step(
            luma_strength.value_at(t) as f32,
            chroma_strength.value_at(t) as f32,
            t,
        ),
        FilterStep::Glow {
            threshold,
            radius,
            intensity,
        } => glow_step(*threshold, *radius, *intensity),
        FilterStep::GlowAnimated {
            threshold,
            radius,
            intensity,
        } => glow_step(
            threshold.value_at(t) as f32,
            radius.value_at(t) as f32,
            intensity.value_at(t) as f32,
        ),
        // The `curves`/`ThreeWayCC` lift is neutral at 1.0; the node's additive
        // `shadows_lift` is neutral at 0.0, so subtract 1.0.
        FilterStep::ThreeWayCC { lift, gamma, gain } => color_wheels_step(
            [lift.r - 1.0, lift.g - 1.0, lift.b - 1.0],
            [gamma.r, gamma.g, gamma.b],
            [gain.r, gain.g, gain.b],
        ),
        FilterStep::ThreeWayCCAnimated { lift, gamma, gain } => {
            let at = |a: &[AnimatedValue<f64>; 3]| {
                [
                    a[0].value_at(t) as f32,
                    a[1].value_at(t) as f32,
                    a[2].value_at(t) as f32,
                ]
            };
            let l = at(lift);
            color_wheels_step([l[0] - 1.0, l[1] - 1.0, l[2] - 1.0], at(gamma), at(gain))
        }
        // An all-empty set of curves is the identity, so skip it (no-op).
        FilterStep::Curves { master, r, g, b } => {
            if master.is_empty() && r.is_empty() && g.is_empty() && b.is_empty() {
                StepClass::Skip
            } else {
                let pts = |c: &[(f32, f32)]| c.iter().map(|&(x, y)| [x, y]).collect::<Vec<_>>();
                StepClass::Effect(GpuEffect::Curves {
                    master: pts(master),
                    red: pts(r),
                    green: pts(g),
                    blue: pts(b),
                })
            }
        }
        // `hue` is `hsl` with a neutral saturation and lightness: both compile to
        // the same `hue` filter's `h=` (see `FilterStep::args`), so this is a
        // rename, not an approximation.
        FilterStep::Hue { degrees } => hsl_step(*degrees, 1.0, 0.0),
        FilterStep::Hsl {
            hue,
            saturation,
            lightness,
        } => hsl_step(*hue, *saturation, *lightness),
        FilterStep::HslAnimated {
            hue,
            saturation,
            lightness,
        } => hsl_step(
            hue.value_at(t) as f32,
            saturation.value_at(t) as f32,
            lightness.value_at(t) as f32,
        ),
        // An empty path is the identity (no-op); otherwise the compositor loads the
        // LUT file. A file it cannot load falls back to CPU there (RK-020).
        FilterStep::Lut3d { path } => {
            if path.is_empty() {
                StepClass::Skip
            } else {
                StepClass::Effect(GpuEffect::Lut { path: path.clone() })
            }
        }
        // ChromaKey: parse the canonical `0xRRGGBB` colour back to the node's key
        // colour. A colour string this parser cannot read (a named/other form from
        // a non-typed source) has no GPU node, so it falls back to CPU (RK-020).
        FilterStep::ChromaKey {
            color,
            similarity,
            blend,
        } => match parse_key_color(color) {
            Some(key_color) => chroma_key_step(key_color, *similarity, *blend),
            None => StepClass::Unsupported,
        },
        FilterStep::ChromaKeyAnimated {
            color,
            similarity,
            blend,
        } => match parse_key_color(color) {
            Some(key_color) => chroma_key_step(
                key_color,
                similarity.value_at(t) as f32,
                blend.value_at(t) as f32,
            ),
            None => StepClass::Unsupported,
        },
        // LumaMask (self-luma mask): the compositor builds the mask from the frame,
        // so only the `invert` toggle carries into the GPU effect.
        FilterStep::LumaMask { invert } => {
            StepClass::Effect(GpuEffect::LumaMask { invert: *invert })
        }
        // ShapeMask (rectangular mask): the compositor builds the rectangle mask from
        // these bounds. A zero-size rectangle masks nothing (Skip).
        FilterStep::RectMask {
            x,
            y,
            width,
            height,
            invert,
        } => shape_mask_step(*x, *y, *width, *height, *invert),
        FilterStep::RectMaskAnimated {
            x,
            y,
            width,
            height,
            invert,
        } => shape_mask_step(
            round_u32(x.value_at(t)),
            round_u32(y.value_at(t)),
            round_u32(width.value_at(t)),
            round_u32(height.value_at(t)),
            *invert,
        ),
        // MotionBlur (exposure trail): the compositor builds the stateful
        // `MotionBlurNode`. A zero shutter is no blur (Skip). Accumulation reuses one
        // node across the clip, which is what the trail *is*.
        FilterStep::MotionBlur {
            shutter_angle_degrees,
            sub_frames,
        } => motion_blur_step(*shutter_angle_degrees, *sub_frames),
        // The animated form evaluates at the frame time like the other animated arms
        // here. The node is not rebuilt for the new value: the compositor pushes it
        // into the live node (`NodeParam::MotionBlurShutter`), because rebuilding
        // would discard the trail this effect exists to accumulate (#1705).
        FilterStep::MotionBlurAnimated {
            shutter_angle,
            sub_frames,
        } => {
            #[allow(clippy::cast_possible_truncation)]
            let degrees = shutter_angle.value_at(t) as f32;
            motion_blur_step(degrees, *sub_frames)
        }
        // Everything else (other colour, masks, animated geometry, xfade, ...) has
        // no GPU node yet. `_` is required: `FilterStep` is `#[non_exhaustive]`
        // from ff-filter (RK-003).
        _ => StepClass::Unsupported,
    }
}

/// Maps `ff_filter::BlendMode` to `ff_render::BlendMode`, or `None` when
/// `ff-render` has no equivalent (forcing fallback).
///
/// Every mode `ff-filter` defines today has a GPU node (#1669); the `None` arm
/// covers a mode a future `FFmpeg` release adds to the `#[non_exhaustive]` enum.
fn map_blend_mode(mode: BlendMode) -> Option<RenderBlendMode> {
    Some(match mode {
        BlendMode::Normal => RenderBlendMode::Normal,
        BlendMode::Multiply => RenderBlendMode::Multiply,
        BlendMode::Screen => RenderBlendMode::Screen,
        BlendMode::Overlay => RenderBlendMode::Overlay,
        BlendMode::SoftLight => RenderBlendMode::SoftLight,
        BlendMode::HardLight => RenderBlendMode::HardLight,
        BlendMode::ColorDodge => RenderBlendMode::ColorDodge,
        BlendMode::ColorBurn => RenderBlendMode::ColorBurn,
        BlendMode::Difference => RenderBlendMode::Difference,
        BlendMode::Exclusion => RenderBlendMode::Exclusion,
        BlendMode::Add => RenderBlendMode::Add,
        BlendMode::Subtract => RenderBlendMode::Subtract,
        BlendMode::Darken => RenderBlendMode::Darken,
        BlendMode::Lighten => RenderBlendMode::Lighten,
        BlendMode::And => RenderBlendMode::And,
        BlendMode::Average => RenderBlendMode::Average,
        BlendMode::Bleach => RenderBlendMode::Bleach,
        BlendMode::Divide => RenderBlendMode::Divide,
        BlendMode::Extremity => RenderBlendMode::Extremity,
        BlendMode::Freeze => RenderBlendMode::Freeze,
        BlendMode::Geometric => RenderBlendMode::Geometric,
        BlendMode::Glow => RenderBlendMode::Glow,
        BlendMode::GrainExtract => RenderBlendMode::GrainExtract,
        BlendMode::GrainMerge => RenderBlendMode::GrainMerge,
        BlendMode::HardMix => RenderBlendMode::HardMix,
        BlendMode::HardOverlay => RenderBlendMode::HardOverlay,
        BlendMode::Harmonic => RenderBlendMode::Harmonic,
        BlendMode::Heat => RenderBlendMode::Heat,
        BlendMode::Interpolate => RenderBlendMode::Interpolate,
        BlendMode::LinearLight => RenderBlendMode::LinearLight,
        BlendMode::Multiply128 => RenderBlendMode::Multiply128,
        BlendMode::Negation => RenderBlendMode::Negation,
        BlendMode::Or => RenderBlendMode::Or,
        BlendMode::Phoenix => RenderBlendMode::Phoenix,
        BlendMode::PinLight => RenderBlendMode::PinLight,
        BlendMode::Reflect => RenderBlendMode::Reflect,
        BlendMode::SoftDifference => RenderBlendMode::SoftDifference,
        BlendMode::Stain => RenderBlendMode::Stain,
        BlendMode::VividLight => RenderBlendMode::VividLight,
        BlendMode::Xor => RenderBlendMode::Xor,
        // A mode added to `ff-filter` after #1669. `_` is required: `BlendMode`
        // is `#[non_exhaustive]` from ff-filter (RK-003).
        _ => return None,
    })
}

/// Maps `ff_filter::CompositeOp` to `ff_render::CompositeOp`, or `None` when
/// `ff-render` has no equivalent (forcing fallback).
///
/// Every operator `ff-filter` defines today composites on the GPU (#1670); the
/// `None` arm covers one a future release adds to the `#[non_exhaustive]` enum.
fn map_composite_op(op: CompositeOp) -> Option<RenderCompositeOp> {
    Some(match op {
        CompositeOp::Over => RenderCompositeOp::Over,
        CompositeOp::Under => RenderCompositeOp::Under,
        CompositeOp::In => RenderCompositeOp::In,
        CompositeOp::Out => RenderCompositeOp::Out,
        CompositeOp::Atop => RenderCompositeOp::Atop,
        CompositeOp::Xor => RenderCompositeOp::Xor,
        // An operator added to `ff-filter` after #1670. `_` is required:
        // `CompositeOp` is `#[non_exhaustive]` from ff-filter (RK-003).
        _ => return None,
    })
}

/// Classifies a film-grain step: a zero strength is a no-op (`Skip`); otherwise it
/// maps the `noise` `[0, 100]` strength to the node's amplitude and derives a
/// per-frame seed from the frame time so the grain varies each frame.
// `as_millis` fits a u32 for any realistic clip time; the grain only needs the seed
// to differ between frames, so wrapping is harmless.
#[allow(clippy::cast_possible_truncation)]
fn film_grain_step(luma_strength: f32, chroma_strength: f32, t: Duration) -> StepClass {
    if luma_strength <= 0.0 && chroma_strength <= 0.0 {
        return StepClass::Skip;
    }
    StepClass::Effect(GpuEffect::FilmGrain {
        luma_strength: luma_strength * NODE_GRAIN_SCALE,
        chroma_strength: chroma_strength * NODE_GRAIN_SCALE,
        frame_index: t.as_millis() as u32,
    })
}

/// Classifies a three-way corrector: a fully neutral corrector is a no-op (`Skip`);
/// otherwise it maps straight to the GPU node (parameters already in the node's
/// convention: additive lift neutral 0, gamma/gain neutral 1).
fn color_wheels_step(
    shadows_lift: [f32; 3],
    midtones_gamma: [f32; 3],
    highlights_gain: [f32; 3],
) -> StepClass {
    let neutral = shadows_lift.iter().all(|&v| v.abs() < 1e-6)
        && midtones_gamma.iter().all(|&v| (v - 1.0).abs() < 1e-6)
        && highlights_gain.iter().all(|&v| (v - 1.0).abs() < 1e-6);
    if neutral {
        return StepClass::Skip;
    }
    StepClass::Effect(GpuEffect::ColorWheels {
        shadows_lift,
        midtones_gamma,
        highlights_gain,
    })
}

/// Classifies a glow step: a non-positive intensity is a no-op (`Skip`); otherwise
/// it maps the (matching) threshold / radius / intensity straight to the GPU node.
fn glow_step(threshold: f32, radius: f32, intensity: f32) -> StepClass {
    if intensity <= 0.0 {
        return StepClass::Skip;
    }
    StepClass::Effect(GpuEffect::Glow {
        threshold,
        radius,
        intensity,
    })
}

/// Classifies an HSL step: a fully neutral adjustment is a no-op (`Skip`);
/// otherwise it maps straight to the GPU node (hue degrees / saturation multiplier
/// / lightness offset already in the node's convention).
#[allow(clippy::float_cmp)]
fn hsl_step(hue_shift: f32, saturation: f32, lightness: f32) -> StepClass {
    if hue_shift == 0.0 && saturation == 1.0 && lightness == 0.0 {
        return StepClass::Skip;
    }
    StepClass::Effect(GpuEffect::Hsl {
        hue_shift,
        saturation,
        lightness,
    })
}

/// Classifies a chroma-key step: a zero `tolerance` removes nothing, so it is a
/// no-op (`Skip`); otherwise it maps straight to the GPU node (the `chromakey`
/// `similarity`/`blend` are the node's `tolerance`/`softness`).
#[allow(clippy::float_cmp)]
fn chroma_key_step(key_color: [f32; 3], tolerance: f32, softness: f32) -> StepClass {
    if tolerance == 0.0 {
        return StepClass::Skip;
    }
    StepClass::Effect(GpuEffect::ChromaKey {
        key_color,
        tolerance,
        softness,
    })
}

/// Classifies a rectangular shape-mask step: a zero `width` or `height` masks
/// nothing, so it is a no-op (`Skip`); otherwise it maps to the GPU node with the
/// rectangle bounds the compositor bakes into the mask.
fn shape_mask_step(x: u32, y: u32, width: u32, height: u32, invert: bool) -> StepClass {
    if width == 0 || height == 0 {
        return StepClass::Skip;
    }
    StepClass::Effect(GpuEffect::ShapeMask {
        x,
        y,
        width,
        height,
        invert,
    })
}

/// Classifies a motion-blur step: a non-positive shutter is no blur, so it is a no-op
/// (`Skip`); otherwise it maps straight to the stateful GPU node (shutter degrees /
/// sub-frame count in the node's convention).
fn motion_blur_step(shutter_angle: f32, sub_frames: u8) -> StepClass {
    if shutter_angle <= 0.0 {
        return StepClass::Skip;
    }
    StepClass::Effect(GpuEffect::MotionBlur {
        shutter_angle,
        sub_frames,
    })
}

/// Classifies a `RotateAnimated` step: the angle folds onto the layer transform
/// rather than becoming an effect node, but only when the fill the CPU `rotate`
/// would use is one the GPU transform can reproduce.
///
/// The GPU transform leaves the corners a rotation exposes **transparent**, while
/// `rotate` fills them with `fillcolor`. Two fills are safe:
///
/// * `none` — transparent, exactly what the transform does.
/// * `black` — what `derive` emits, and what the CPU compositor's own *static*
///   layer rotation uses (`composition_inner` builds `rotate` with no `fillcolor`,
///   whose default is black). Folding therefore makes an animated rotation behave
///   like a static one on both paths: the black-versus-transparent difference is
///   the pre-existing one the static mapping already carries, not a new one.
///
/// Any other fill has nothing to map to, so it falls back rather than render
/// different corners (RK-020). The comparison is case-insensitive because
/// `FFmpeg` colour names are.
// The model's `f64` degrees narrow to the `f32` the plan carries; that is the
// intended, lossy conversion, as in `eval_at`.
#[allow(clippy::cast_possible_truncation)]
fn rotate_animated_step(degrees: f64, fill_color: &str) -> StepClass {
    if !(fill_color.eq_ignore_ascii_case("black") || fill_color.eq_ignore_ascii_case("none")) {
        return StepClass::Unsupported;
    }
    StepClass::Rotation(degrees as f32)
}

/// Rounds an animated pixel bound (`f64`) to a non-negative `u32` for the mask.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_u32(v: f64) -> u32 {
    v.max(0.0).round() as u32
}

/// Parses an `FFmpeg` colour string into an RGB triple (each channel
/// `0.0..=1.0`), or `None` when it is not a colour this build can name.
///
/// Delegates to [`ff_format::Color::parse_ffmpeg`], so a name (`"green"`) works as
/// well as the canonical hex the typed effects emit — which is what lets a
/// hand-written or imported `chromakey=green` reach the GPU node instead of
/// falling back (#1630). Any alpha the string carries is dropped: the GPU node's
/// key colour is RGB, and the alpha it produces comes from the keying.
fn parse_key_color(color: &str) -> Option<[f32; 3]> {
    let c = ff_format::Color::parse_ffmpeg(color)?;
    Some([
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
    ])
}

/// Maps `ff_filter::ScaleAlgorithm` to `ff_render::ScaleAlgorithm` (ff-render has no
/// `Fast`, so it maps to the closest `Bilinear`).
fn map_scale_algo(algo: ScaleAlgorithm) -> RenderScaleAlgorithm {
    match algo {
        ScaleAlgorithm::Fast | ScaleAlgorithm::Bilinear => RenderScaleAlgorithm::Bilinear,
        ScaleAlgorithm::Bicubic => RenderScaleAlgorithm::Bicubic,
        ScaleAlgorithm::Lanczos => RenderScaleAlgorithm::Lanczos,
        // `ScaleAlgorithm` is `#[non_exhaustive]` from ff-filter (RK-003).
        _ => RenderScaleAlgorithm::Bilinear,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ff_filter::{AnimationTrack, Easing, Keyframe, XfadeTransition};

    use super::*;
    use crate::Clip;
    use crate::track::TrackAutomation;

    /// Names the GPU/CPU parity test (in `crates/avio/tests/gpu_parity_tests.rs`) that
    /// exercises each mapped [`GpuEffect`] node. The exhaustive `match` is the point:
    /// adding a `GpuEffect` variant without a parity test fails to compile here, which
    /// enforces #1663's "a per-effect parity test exists for each supported effect" for
    /// every future node. **Never add a `_` arm** — that would defeat the guard (this is
    /// in-crate, so `#[non_exhaustive]` does not force one; RK-003).
    fn parity_coverage(effect: &GpuEffect) -> &'static str {
        match effect {
            GpuEffect::ColorGrade { .. } => "color_grade_gpu_should_match_cpu_within_tolerance",
            GpuEffect::Scale { .. } => "scale_gpu_should_match_cpu_within_tolerance",
            GpuEffect::Blur { .. } => "blur_gpu_should_match_cpu_within_tolerance",
            GpuEffect::Sharpen { .. } => "sharpen_gpu_should_match_cpu_within_tolerance",
            GpuEffect::Vignette { .. } => "vignette_gpu_should_match_cpu_within_tolerance",
            GpuEffect::FilmGrain { .. } => "film_grain_gpu_should_match_cpu_grain_strength",
            GpuEffect::Glow { .. } => "glow_gpu_should_match_cpu_within_tolerance",
            GpuEffect::ColorWheels { .. } => "color_wheels_gpu_should_match_cpu_within_tolerance",
            GpuEffect::Curves { .. } => "curves_gpu_should_match_cpu_within_tolerance",
            GpuEffect::Hsl { .. } => "hsl_gpu_should_match_cpu_within_tolerance",
            GpuEffect::Lut { .. } => "lut_gpu_should_match_cpu_within_tolerance",
            GpuEffect::ChromaKey { .. } => "chroma_key_gpu_should_match_cpu_within_tolerance",
            GpuEffect::LumaMask { .. } => "luma_mask_gpu_should_match_cpu_within_tolerance",
            GpuEffect::ShapeMask { .. } => "shape_mask_gpu_should_match_cpu_within_tolerance",
            GpuEffect::MotionBlur { .. } => "motion_blur_gpu_should_match_cpu_within_tolerance",
        }
    }

    #[test]
    fn every_gpu_effect_variant_is_registered_for_parity() {
        // The compile-time exhaustiveness of `parity_coverage` is the real guard; this
        // keeps it referenced and spot-checks one mapping.
        let scale = GpuEffect::Scale {
            width: 2,
            height: 2,
            algorithm: RenderScaleAlgorithm::Bilinear,
        };
        assert_eq!(
            parity_coverage(&scale),
            "scale_gpu_should_match_cpu_within_tolerance"
        );
    }

    /// A fully controllable [`GpuLayerSource`] for exercising the mapping in
    /// isolation (identity transform, Normal blend, Over composite, no effects).
    struct TestLayer {
        opacity: AnimatedValue<f64>,
        x: AnimatedValue<f64>,
        y: AnimatedValue<f64>,
        scale_x: AnimatedValue<f64>,
        scale_y: AnimatedValue<f64>,
        rotation: AnimatedValue<f64>,
        blend_mode: BlendMode,
        composite_op: CompositeOp,
        effects: Vec<FilterStep>,
    }

    impl TestLayer {
        fn identity() -> Self {
            Self {
                opacity: AnimatedValue::Static(1.0),
                x: AnimatedValue::Static(0.0),
                y: AnimatedValue::Static(0.0),
                scale_x: AnimatedValue::Static(1.0),
                scale_y: AnimatedValue::Static(1.0),
                rotation: AnimatedValue::Static(0.0),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                effects: Vec::new(),
            }
        }
    }

    impl GpuLayerSource for TestLayer {
        fn opacity(&self) -> &AnimatedValue<f64> {
            &self.opacity
        }
        fn x(&self) -> &AnimatedValue<f64> {
            &self.x
        }
        fn y(&self) -> &AnimatedValue<f64> {
            &self.y
        }
        fn scale_x(&self) -> &AnimatedValue<f64> {
            &self.scale_x
        }
        fn scale_y(&self) -> &AnimatedValue<f64> {
            &self.scale_y
        }
        fn rotation(&self) -> &AnimatedValue<f64> {
            &self.rotation
        }
        fn blend_mode(&self) -> BlendMode {
            self.blend_mode
        }
        fn composite_op(&self) -> CompositeOp {
            self.composite_op
        }
        fn effects(&self) -> &[FilterStep] {
            &self.effects
        }
    }

    fn gpu(mapping: GpuMapping) -> GpuScenePlan {
        match mapping {
            GpuMapping::Gpu(plan) => plan,
            GpuMapping::Fallback(reason) => panic!("expected Gpu, got Fallback({reason:?})"),
        }
    }

    #[test]
    fn map_scene_should_map_identity_layer_to_a_single_gpu_plan_layer() {
        let plan = gpu(map_scene(
            &[TestLayer::identity()],
            (1920, 1080),
            Duration::ZERO,
        ));
        assert_eq!(plan.canvas, (1920, 1080));
        let [layer] = plan.layers.as_slice() else {
            panic!("expected one layer");
        };
        assert_eq!(layer.z_order, 0);
        assert_eq!(layer.x, 0.0);
        assert_eq!(layer.scale_x, 1.0);
        assert_eq!(layer.rotation, 0.0);
        assert_eq!(layer.opacity, 1.0);
        assert_eq!(layer.blend_mode, RenderBlendMode::Normal);
        assert!(layer.effects.is_empty());
    }

    #[test]
    fn map_scene_should_evaluate_animated_transform_at_t() {
        // x ramps 0 -> 100 over 0..2s; at t=1s it should read ~50.
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.0, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(2), 100.0, Easing::Linear));
        let mut layer = TestLayer::identity();
        layer.x = AnimatedValue::Track(track);
        let plan = gpu(map_scene(&[layer], (100, 100), Duration::from_secs(1)));
        assert!((plan.layers[0].x - 50.0).abs() < 1e-3);
    }

    #[test]
    fn map_scene_should_map_eq_to_color_grade() {
        let mut layer = TestLayer::identity();
        // Non-neutral temperature/tint must flow through to the node (not the old
        // hardcoded 0.0), so the assertion below is non-vacuous.
        layer.effects = vec![FilterStep::Eq {
            brightness: 0.5,
            contrast: 1.2,
            saturation: 0.8,
            temperature: 0.4,
            tint: -0.3,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::ColorGrade {
                brightness: 0.5,
                contrast: 1.2,
                saturation: 0.8,
                temperature: 0.4,
                tint: -0.3,
            }]
        );
    }

    #[test]
    fn map_scene_should_map_eq_animated_to_color_grade_when_gamma_is_neutral() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::EqAnimated {
            brightness: AnimatedValue::Static(0.3),
            contrast: AnimatedValue::Static(1.1),
            saturation: AnimatedValue::Static(0.9),
            gamma: AnimatedValue::Static(1.0),
            temperature: AnimatedValue::Static(0.5),
            tint: AnimatedValue::Static(-0.2),
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::ColorGrade {
                brightness: 0.3,
                contrast: 1.1,
                saturation: 0.9,
                temperature: 0.5,
                tint: -0.2,
            }]
        );
    }

    #[test]
    fn map_scene_should_fall_back_when_eq_animated_gamma_is_not_neutral() {
        // ff-render's ColorGrade has no gamma, so a non-neutral gamma must fall back
        // rather than silently drop the gamma.
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::EqAnimated {
            brightness: AnimatedValue::Static(0.0),
            contrast: AnimatedValue::Static(1.0),
            saturation: AnimatedValue::Static(1.0),
            gamma: AnimatedValue::Static(1.5),
            temperature: AnimatedValue::Static(0.0),
            tint: AnimatedValue::Static(0.0),
        }];
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedEffect)
        );
    }

    #[test]
    fn map_scene_should_clamp_out_of_range_opacity() {
        let mut layer = TestLayer::identity();
        layer.opacity = AnimatedValue::Static(1.5);
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(plan.layers[0].opacity, 1.0);
    }

    #[test]
    fn map_scene_should_map_scale_step() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Scale {
            width: 320,
            height: 240,
            algorithm: ScaleAlgorithm::Bicubic,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::Scale {
                width: 320,
                height: 240,
                algorithm: RenderScaleAlgorithm::Bicubic,
            }]
        );
    }

    #[test]
    fn map_scene_should_map_scale_animated_to_the_scale_node_at_the_frame_time() {
        // #1630. Width ramps 320 -> 640 and height 240 -> 480 over 0..2s, so t=1s
        // must read the midpoint. Asserting at a single `t` would pass for a
        // mapping that ignored the tracks and used their t=0 values (RK-015).
        let track = |a: f64, b: f64| {
            AnimatedValue::Track(
                AnimationTrack::new()
                    .push(Keyframe::new(Duration::ZERO, a, Easing::Linear))
                    .push(Keyframe::new(Duration::from_secs(2), b, Easing::Linear)),
            )
        };
        let step = FilterStep::ScaleAnimated {
            width: track(320.0, 640.0),
            height: track(240.0, 480.0),
            algorithm: ScaleAlgorithm::Bicubic,
        };
        let at = |t: Duration| {
            let mut layer = TestLayer::identity();
            layer.effects = vec![step.clone()];
            gpu(map_scene(&[layer], (16, 16), t)).layers[0]
                .effects
                .clone()
        };
        assert_eq!(
            at(Duration::ZERO).as_slice(),
            [GpuEffect::Scale {
                width: 320,
                height: 240,
                // The algorithm must survive: it is the field a fold into the
                // layer transform would have had to drop.
                algorithm: RenderScaleAlgorithm::Bicubic,
            }]
        );
        assert_eq!(
            at(Duration::from_secs(1)).as_slice(),
            [GpuEffect::Scale {
                width: 480,
                height: 360,
                algorithm: RenderScaleAlgorithm::Bicubic,
            }]
        );
    }

    #[test]
    fn map_scene_should_fold_rotate_animated_into_the_layer_rotation() {
        // #1630. `derive` lifts an animated rotation off the layer scalar
        // (ADR-0005); the mapping puts it back, per frame. Two times again, so a
        // mapping that ignored the track cannot pass.
        let step = FilterStep::RotateAnimated {
            angle: AnimatedValue::Track(
                AnimationTrack::new()
                    .push(Keyframe::new(Duration::ZERO, 0.0, Easing::Linear))
                    .push(Keyframe::new(Duration::from_secs(2), 90.0, Easing::Linear)),
            ),
            fill_color: "black".to_string(),
        };
        let at = |t: Duration| {
            let mut layer = TestLayer::identity();
            layer.effects = vec![step.clone()];
            let plan = gpu(map_scene(&[layer], (16, 16), t));
            // It belongs on the transform, not in the effect list.
            assert!(
                plan.layers[0].effects.is_empty(),
                "a rotation must not become an effect node"
            );
            plan.layers[0].rotation
        };
        assert!((at(Duration::ZERO) - 0.0).abs() < 1e-3);
        assert!((at(Duration::from_secs(1)) - 45.0).abs() < 1e-3);
    }

    #[test]
    fn map_scene_should_add_a_rotate_animated_step_to_the_layer_rotation() {
        // The CPU applies both the layer scalar (the compositor's own `rotate`)
        // and the effect chain's, so the mapping sums them. A test with only one
        // source of rotation passes just as well if the mapping *replaces*, which
        // is why this one supplies both.
        let mut layer = TestLayer::identity();
        layer.rotation = AnimatedValue::Static(30.0);
        layer.effects = vec![FilterStep::RotateAnimated {
            angle: AnimatedValue::Static(15.0),
            fill_color: "black".to_string(),
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert!(
            (plan.layers[0].rotation - 45.0).abs() < 1e-3,
            "layer 30deg + step 15deg must be 45deg, got {}",
            plan.layers[0].rotation
        );
    }

    #[test]
    fn map_scene_should_fall_back_for_a_rotate_animated_with_an_unmappable_fill() {
        // The GPU transform leaves the exposed corners transparent, so only a fill
        // that matches (`none`) or the one the static path already uses (`black`)
        // can fold. Anything else has no mapping and must not be rendered as if it
        // did (RK-020).
        for fill in ["black", "none", "BLACK", "None"] {
            let mut layer = TestLayer::identity();
            layer.effects = vec![FilterStep::RotateAnimated {
                angle: AnimatedValue::Static(10.0),
                fill_color: fill.to_string(),
            }];
            assert!(
                matches!(
                    map_scene(&[layer], (16, 16), Duration::ZERO),
                    GpuMapping::Gpu(_)
                ),
                "{fill} must fold onto the transform"
            );
        }
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::RotateAnimated {
            angle: AnimatedValue::Static(10.0),
            fill_color: "red".to_string(),
        }];
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedEffect)
        );
    }

    #[test]
    fn map_scene_should_map_hue_to_the_hsl_node() {
        // #1630. `hue` and `hsl` compile to the same FFmpeg `hue` filter's `h=`,
        // so `Hue` is `Hsl` with a neutral saturation and lightness.
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Hue { degrees: 42.0 }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::Hsl {
                hue_shift: 42.0,
                saturation: 1.0,
                lightness: 0.0,
            }]
        );

        // A zero rotation is the identity, so it is skipped rather than mapped.
        let mut neutral = TestLayer::identity();
        neutral.effects = vec![FilterStep::Hue { degrees: 0.0 }];
        let plan = gpu(map_scene(&[neutral], (16, 16), Duration::ZERO));
        assert!(plan.layers[0].effects.is_empty());
    }

    #[test]
    fn map_scene_should_map_a_chroma_key_with_a_named_colour() {
        // #1630: the colour string used to have to be hex. A name now resolves
        // through `ff_format::Color::parse_ffmpeg`, and FFmpeg's `green` is
        // 0x008000 — so this also pins that the value came from that table.
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::ChromaKey {
            color: "green".to_string(),
            similarity: 0.3,
            blend: 0.1,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        let [GpuEffect::ChromaKey { key_color, .. }] = plan.layers[0].effects.as_slice() else {
            panic!("a named chroma-key colour must map to the GPU node");
        };
        assert!((key_color[0] - 0.0).abs() < 1e-6);
        assert!((key_color[1] - 128.0 / 255.0).abs() < 1e-6);
        assert!((key_color[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn map_scene_should_still_fall_back_for_a_color_key() {
        // Deliberately NOT mapped. `ChromaKeyNode` measures *chroma* distance (it
        // subtracts BT.709 luma from pixel and key first), which is `chromakey`'s
        // semantic; `colorkey` is a full-RGB distance. Routing it to that node
        // would be a silent approximation (RK-020), so it stays a fallback and
        // this pins that the named-colour parser did not quietly enable it.
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::ColorKey {
            color: "green".to_string(),
            similarity: 0.3,
            blend: 0.1,
        }];
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedEffect)
        );
    }

    #[test]
    fn map_scene_should_map_gblur_to_blur() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::GBlur { sigma: 3.5 }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::Blur { sigma: 3.5 }]
        );
    }

    #[test]
    fn map_scene_should_evaluate_animated_gblur_sigma_at_t() {
        // sigma ramps 2 -> 6 over 0..2s; at t=1s it should read ~4.
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 2.0, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(2), 6.0, Easing::Linear));
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::GBlurAnimated {
            sigma: AnimatedValue::Track(track),
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::from_secs(1)));
        let [GpuEffect::Blur { sigma }] = plan.layers[0].effects.as_slice() else {
            panic!("animated gblur must map to a single Blur effect");
        };
        assert!(
            (sigma - 4.0).abs() < 1e-3,
            "animated sigma at t=1s should be ~4; got {sigma}"
        );
    }

    #[test]
    fn map_scene_should_map_unsharp_to_sharpen() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Unsharp {
            luma_strength: 0.8,
            chroma_strength: 0.0,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::Sharpen {
                radius: DEFAULT_SHARPEN_RADIUS,
                strength: 0.8,
            }]
        );
    }

    #[test]
    fn map_scene_should_evaluate_animated_unsharp_amount_at_t() {
        // amount ramps 0.2 -> 0.6 over 0..2s; at t=1s it should read ~0.4.
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.2, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(2), 0.6, Easing::Linear));
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::UnsharpAnimated {
            luma_strength: AnimatedValue::Track(track),
            chroma_strength: AnimatedValue::Static(0.0),
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::from_secs(1)));
        let [GpuEffect::Sharpen { strength, .. }] = plan.layers[0].effects.as_slice() else {
            panic!("animated unsharp must map to a single Sharpen effect");
        };
        assert!(
            (strength - 0.4).abs() < 1e-3,
            "animated amount at t=1s should be ~0.4; got {strength}"
        );
    }

    #[test]
    fn map_scene_should_fall_back_on_unsharp_with_chroma() {
        // A non-zero chroma amount cannot be represented by the luma-only GPU node,
        // so the whole frame must fall back rather than drop the chroma (RK-020).
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Unsharp {
            luma_strength: 0.5,
            chroma_strength: 0.5,
        }];
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedEffect)
        );
    }

    #[test]
    fn map_scene_should_map_vignette_animated_to_vignette() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::VignetteAnimated {
            amount: AnimatedValue::Static(0.6),
            x0: 0.0,
            y0: 0.0,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        let [
            GpuEffect::Vignette {
                radius,
                strength,
                feather,
            },
        ] = plan.layers[0].effects.as_slice()
        else {
            panic!("vignette must map to a single Vignette effect");
        };
        assert!(
            (strength - 0.6).abs() < 1e-6,
            "amount maps to strength; got {strength}"
        );
        assert!(
            *radius > 0.0 && *feather > 0.0,
            "radius/feather use the defaults"
        );
    }

    #[test]
    fn map_scene_should_map_static_vignette_normalising_angle() {
        // A hand-built static Vignette at the max angle (PI/2) maps to full strength.
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Vignette {
            angle: std::f32::consts::FRAC_PI_2,
            x0: 0.0,
            y0: 0.0,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        let [GpuEffect::Vignette { strength, .. }] = plan.layers[0].effects.as_slice() else {
            panic!("vignette must map to a single Vignette effect");
        };
        assert!(
            (strength - 1.0).abs() < 1e-6,
            "angle PI/2 -> strength 1.0; got {strength}"
        );
    }

    #[test]
    fn map_scene_should_fall_back_on_off_centre_vignette() {
        // The GPU node is centred, so an off-centre vignette falls back (RK-020).
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::VignetteAnimated {
            amount: AnimatedValue::Static(0.5),
            x0: 320.0,
            y0: 0.0,
        }];
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedEffect)
        );
    }

    #[test]
    fn map_scene_should_map_film_grain_with_per_frame_seed() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::FilmGrain {
            luma_strength: 20.0,
            chroma_strength: 5.0,
        }];
        // Two distinct frame times must yield distinct grain seeds (no sticking).
        let plan_a = gpu(map_scene(
            &[TestLayer {
                effects: layer.effects.clone(),
                ..TestLayer::identity()
            }],
            (16, 16),
            Duration::from_millis(0),
        ));
        let plan_b = gpu(map_scene(&[layer], (16, 16), Duration::from_millis(100)));
        let [
            GpuEffect::FilmGrain {
                luma_strength,
                frame_index: fa,
                ..
            },
        ] = plan_a.layers[0].effects.as_slice()
        else {
            panic!("film grain must map to a single FilmGrain effect");
        };
        let [
            GpuEffect::FilmGrain {
                frame_index: fb, ..
            },
        ] = plan_b.layers[0].effects.as_slice()
        else {
            panic!("film grain must map to a single FilmGrain effect");
        };
        assert!(
            *luma_strength > 0.0,
            "the noise strength maps to a node amplitude"
        );
        assert_ne!(
            fa, fb,
            "distinct frame times must give distinct grain seeds"
        );
    }

    #[test]
    fn map_scene_should_skip_zero_strength_film_grain() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::FilmGrain {
            luma_strength: 0.0,
            chroma_strength: 0.0,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert!(
            plan.layers[0].effects.is_empty(),
            "zero-strength grain is a no-op and is skipped"
        );
    }

    #[test]
    fn map_scene_should_evaluate_animated_film_grain_at_t() {
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 10.0, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(2), 30.0, Easing::Linear));
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::FilmGrainAnimated {
            luma_strength: AnimatedValue::Track(track),
            chroma_strength: AnimatedValue::Static(0.0),
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::from_secs(1)));
        let [GpuEffect::FilmGrain { luma_strength, .. }] = plan.layers[0].effects.as_slice() else {
            panic!("animated film grain must map to a single FilmGrain effect");
        };
        // amount at t=1s is ~20 (noise scale); scaled to the node amplitude.
        assert!(
            (luma_strength - 20.0 * NODE_GRAIN_SCALE).abs() < 1e-4,
            "animated strength at t=1s should be ~20*scale; got {luma_strength}"
        );
    }

    #[test]
    fn map_scene_should_map_glow_directly() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Glow {
            threshold: 0.8,
            radius: 10.0,
            intensity: 0.8,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::Glow {
                threshold: 0.8,
                radius: 10.0,
                intensity: 0.8,
            }]
        );
    }

    #[test]
    fn map_scene_should_evaluate_animated_glow_at_t() {
        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.4, Easing::Linear))
            .push(Keyframe::new(Duration::from_secs(2), 1.2, Easing::Linear));
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::GlowAnimated {
            threshold: AnimatedValue::Static(0.8),
            radius: AnimatedValue::Static(10.0),
            intensity: AnimatedValue::Track(track),
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::from_secs(1)));
        let [GpuEffect::Glow { intensity, .. }] = plan.layers[0].effects.as_slice() else {
            panic!("animated glow must map to a single Glow effect");
        };
        assert!(
            (intensity - 0.8).abs() < 1e-3,
            "animated intensity at t=1s should be ~0.8; got {intensity}"
        );
    }

    #[test]
    fn map_scene_should_skip_zero_intensity_glow() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Glow {
            threshold: 0.8,
            radius: 10.0,
            intensity: 0.0,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert!(
            plan.layers[0].effects.is_empty(),
            "zero-intensity glow is a no-op and is skipped"
        );
    }

    #[test]
    fn map_scene_should_map_three_way_cc_to_color_wheels() {
        // ThreeWayCC lift 1.1 -> node shadows_lift 0.1 (subtract the neutral 1.0).
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::ThreeWayCC {
            lift: ff_filter::Rgb {
                r: 1.1,
                g: 1.1,
                b: 1.1,
            },
            gamma: ff_filter::Rgb {
                r: 1.2,
                g: 1.2,
                b: 1.2,
            },
            gain: ff_filter::Rgb::NEUTRAL,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        let [
            GpuEffect::ColorWheels {
                shadows_lift,
                midtones_gamma,
                ..
            },
        ] = plan.layers[0].effects.as_slice()
        else {
            panic!("three-way cc must map to a single ColorWheels effect");
        };
        assert!(
            (shadows_lift[0] - 0.1).abs() < 1e-5,
            "lift 1.1 -> shadows_lift 0.1"
        );
        assert!(
            (midtones_gamma[0] - 1.2).abs() < 1e-6,
            "gamma maps directly"
        );
    }

    #[test]
    fn map_scene_should_skip_neutral_three_way_cc() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::ThreeWayCC {
            lift: ff_filter::Rgb::NEUTRAL,
            gamma: ff_filter::Rgb::NEUTRAL,
            gain: ff_filter::Rgb::NEUTRAL,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert!(
            plan.layers[0].effects.is_empty(),
            "a neutral three-way corrector is a no-op and is skipped"
        );
    }

    #[test]
    fn map_scene_should_map_curves_with_array_points() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Curves {
            master: vec![(0.0, 0.0), (0.5, 0.7), (1.0, 1.0)],
            r: vec![],
            g: vec![],
            b: vec![],
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        let [GpuEffect::Curves { master, red, .. }] = plan.layers[0].effects.as_slice() else {
            panic!("curves must map to a single Curves effect");
        };
        assert_eq!(master.as_slice(), [[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
        assert!(red.is_empty(), "an empty per-channel curve stays empty");
    }

    #[test]
    fn map_scene_should_skip_empty_curves() {
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Curves {
            master: vec![],
            r: vec![],
            g: vec![],
            b: vec![],
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert!(
            plan.layers[0].effects.is_empty(),
            "an all-empty Curves is a no-op and is skipped"
        );
    }

    #[test]
    fn map_scene_should_skip_temporal_steps() {
        let mut layer = TestLayer::identity();
        // Temporal steps present on the export layer must be skipped, not fallback.
        layer.effects = vec![
            FilterStep::Trim {
                start: Some(1.0),
                end: Some(4.0),
            },
            FilterStep::ResetPts,
            FilterStep::OffsetPts { seconds: 2.0 },
            FilterStep::Speed { factor: 2.0 },
            FilterStep::Eq {
                brightness: 0.1,
                contrast: 1.0,
                saturation: 1.0,
                temperature: 0.0,
                tint: 0.0,
            },
        ];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.len(),
            1,
            "temporal steps skipped; only the colour grade remains"
        );
        assert!(matches!(
            plan.layers[0].effects[0],
            GpuEffect::ColorGrade { .. }
        ));
    }

    #[test]
    fn map_scene_should_fall_back_on_unsupported_effect() {
        // `Hue` stood here until #1630 mapped it; `XFade` needs the 2-input
        // `CrossfadeNode` the plan has no place for, so it is the current example
        // of a step with no GPU mapping at all.
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::XFade {
            transition: XfadeTransition::Fade,
            duration: 1.0,
            offset: 0.0,
        }];
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedEffect)
        );
    }

    /// #1669's acceptance criterion as an executable check: every blend mode the
    /// model can express maps to a GPU node, so none of them forces a fallback.
    /// A mode `ff-filter` adds later has no row here and would still fall back,
    /// which is what the `_` arm in `map_blend_mode` is for.
    #[test]
    fn map_scene_should_map_every_blend_mode() {
        let modes = [
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
            BlendMode::SoftDifference,
            BlendMode::Stain,
            BlendMode::VividLight,
            BlendMode::Xor,
        ];
        assert_eq!(modes.len(), 40, "ff-filter's BlendMode set changed");
        for mode in modes {
            let mut layer = TestLayer::identity();
            layer.blend_mode = mode;
            let mapping = map_scene(&[layer], (16, 16), Duration::ZERO);
            assert!(
                !matches!(
                    mapping,
                    GpuMapping::Fallback(GpuFallback::UnsupportedBlendMode(_))
                ),
                "{mode:?} must map to a GPU node; got {mapping:?}"
            );
        }
    }

    /// #1670's acceptance criterion as an executable check: every composite
    /// operator the model can express maps to the GPU, so none forces a
    /// fallback. An operator `ff-filter` adds later has no row here and would
    /// still fall back, which is what the `_` arm in `map_composite_op` is for.
    #[test]
    fn map_scene_should_map_every_composite_op() {
        let ops = [
            CompositeOp::Over,
            CompositeOp::Under,
            CompositeOp::In,
            CompositeOp::Out,
            CompositeOp::Atop,
            CompositeOp::Xor,
        ];
        assert_eq!(ops.len(), 6, "ff-filter's CompositeOp set changed");
        for op in ops {
            let mut layer = TestLayer::identity();
            layer.composite_op = op;
            let mapping = map_scene(&[layer], (16, 16), Duration::ZERO);
            assert!(
                !matches!(
                    mapping,
                    GpuMapping::Fallback(GpuFallback::UnsupportedCompositeOp(_))
                ),
                "{op:?} must map to the GPU; got {mapping:?}"
            );
        }
    }

    /// The model does not combine a blend mode with a non-`Over` composite, so
    /// the plan carries `Normal` rather than falling back on the blend mode.
    #[test]
    fn map_scene_should_drop_the_blend_mode_for_a_non_over_composite() {
        let mut layer = TestLayer::identity();
        layer.composite_op = CompositeOp::In;
        layer.blend_mode = BlendMode::Multiply;
        let GpuMapping::Gpu(plan) = map_scene(&[layer], (16, 16), Duration::ZERO) else {
            panic!("a non-Over composite must map to the GPU since #1670");
        };
        assert_eq!(plan.layers[0].composite_op, RenderCompositeOp::In);
        assert_eq!(
            plan.layers[0].blend_mode,
            RenderBlendMode::Normal,
            "the blend mode must be dropped, matching the CPU path"
        );
    }

    #[test]
    fn map_scene_should_accept_realtime_layer_and_references() {
        // The preview runner maps borrowed `RealtimeLayer`s (`&[&RealtimeLayer]`);
        // the owned and borrowed forms must produce the same plan.
        let desc = RealtimeLayerDescriptor {
            effects: vec![FilterStep::Eq {
                brightness: 0.5,
                contrast: 1.0,
                saturation: 1.0,
                temperature: 0.0,
                tint: 0.0,
            }],
            opacity: AnimatedValue::Static(1.0),
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            blend_mode: BlendMode::Normal,
            composite_op: CompositeOp::Over,
        };
        let layer = RealtimeLayer::with_dimensions(desc, 8, 8, ff_format::PixelFormat::Rgba);
        let owned = map_scene(std::slice::from_ref(&layer), (8, 8), Duration::ZERO);
        let by_ref = map_scene(&[&layer], (8, 8), Duration::ZERO);
        assert_eq!(owned, by_ref, "a &RealtimeLayer maps like a RealtimeLayer");
        assert!(matches!(owned, GpuMapping::Gpu(_)));
    }

    #[test]
    fn map_scene_should_map_the_blend_mode_intersection() {
        let mut layer = TestLayer::identity();
        layer.blend_mode = BlendMode::Multiply;
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(plan.layers[0].blend_mode, RenderBlendMode::Multiply);
    }

    #[test]
    fn map_scene_should_be_shared_by_video_layer_and_realtime_descriptor() {
        // The export VideoLayer carries temporal steps; the preview descriptor does
        // not. After skipping temporal steps both must yield the same plan (single
        // source of truth). A trimmed, sped-up, colour-corrected clip exercises the
        // temporal skip.
        let clip = Clip::new("v.mp4")
            .trim(Duration::from_secs(1), Duration::from_secs(4))
            .with_speed(2.0)
            .with_color_correction(0.5, 1.2, 0.8);
        let auto = TrackAutomation::default();
        let video_layer = crate::derive::video_layer(
            &clip,
            0,
            &auto,
            1920,
            1080,
            &crate::derive::Placement::default(),
            None,
        );
        let descriptor = crate::derive::realtime_descriptor(&clip, &auto, 1920, 1080);

        let from_export = map_scene(
            std::slice::from_ref(&video_layer),
            (1920, 1080),
            Duration::ZERO,
        );
        let from_preview = map_scene(
            std::slice::from_ref(&descriptor),
            (1920, 1080),
            Duration::ZERO,
        );
        assert_eq!(
            from_export, from_preview,
            "the mapping must agree for the export and preview derived layers"
        );
        // And it is a real GPU plan with the single colour grade.
        let plan = gpu(from_export);
        assert!(matches!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::ColorGrade { .. }]
        ));
    }

    #[test]
    fn parse_key_color_should_read_hex_and_named_forms_and_reject_others() {
        assert_eq!(parse_key_color("0x00FF00"), Some([0.0, 1.0, 0.0]));
        assert_eq!(parse_key_color("#0000FF"), Some([0.0, 0.0, 1.0]));
        // #1630: a name used to be `None` (CPU fallback); it now resolves through
        // `ff_format::Color::parse_ffmpeg`. FFmpeg's `green` is 0x008000, so this
        // also pins that the value comes from that table and not from a guess.
        assert_eq!(parse_key_color("green"), Some([0.0, 128.0 / 255.0, 0.0]));
        assert_eq!(parse_key_color("lime"), Some([0.0, 1.0, 0.0]));
        // Alpha is dropped: the node's key colour is RGB.
        assert_eq!(parse_key_color("0x00FF0080"), Some([0.0, 1.0, 0.0]));
        assert_eq!(parse_key_color("no_such_colour"), None);
        assert_eq!(parse_key_color("0x00FF"), None);
    }

    #[test]
    fn classify_chroma_key_should_map_to_node_with_parsed_colour() {
        let step = FilterStep::ChromaKey {
            color: "0x00FF00".to_string(),
            similarity: 0.3,
            blend: 0.1,
        };
        match classify_step(&step, Duration::ZERO) {
            StepClass::Effect(GpuEffect::ChromaKey {
                key_color,
                tolerance,
                softness,
            }) => {
                assert_eq!(key_color, [0.0, 1.0, 0.0]);
                assert!((tolerance - 0.3).abs() < 1e-5);
                assert!((softness - 0.1).abs() < 1e-5);
            }
            _ => panic!("expected a ChromaKey GPU effect"),
        }
    }

    #[test]
    fn classify_chroma_key_zero_tolerance_should_skip() {
        let step = FilterStep::ChromaKey {
            color: "0x00FF00".to_string(),
            similarity: 0.0,
            blend: 0.1,
        };
        assert!(matches!(
            classify_step(&step, Duration::ZERO),
            StepClass::Skip
        ));
    }

    #[test]
    fn classify_chroma_key_unparseable_colour_should_fall_back_to_cpu() {
        // `"green"` stood here until #1630 taught the parser colour names. The
        // fallback is still reachable, just for a string no colour table has.
        let step = FilterStep::ChromaKey {
            color: "not_a_colour".to_string(),
            similarity: 0.3,
            blend: 0.1,
        };
        assert!(matches!(
            classify_step(&step, Duration::ZERO),
            StepClass::Unsupported
        ));
    }

    #[test]
    fn classify_luma_mask_should_map_to_node_with_invert_flag() {
        for invert in [false, true] {
            let step = FilterStep::LumaMask { invert };
            match classify_step(&step, Duration::ZERO) {
                StepClass::Effect(GpuEffect::LumaMask { invert: got }) => assert_eq!(got, invert),
                _ => panic!("expected a LumaMask GPU effect for invert={invert}"),
            }
        }
    }

    #[test]
    fn classify_rect_mask_should_map_to_shape_mask_with_bounds() {
        let step = FilterStep::RectMask {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
            invert: true,
        };
        match classify_step(&step, Duration::ZERO) {
            StepClass::Effect(GpuEffect::ShapeMask {
                x,
                y,
                width,
                height,
                invert,
            }) => {
                assert_eq!((x, y, width, height), (10, 20, 30, 40));
                assert!(invert);
            }
            _ => panic!("expected a ShapeMask GPU effect"),
        }
    }

    #[test]
    fn classify_rect_mask_animated_should_evaluate_bounds_at_t() {
        let step = FilterStep::RectMaskAnimated {
            x: AnimatedValue::Static(5.0),
            y: AnimatedValue::Static(6.0),
            width: AnimatedValue::Static(7.0),
            height: AnimatedValue::Static(8.0),
            invert: false,
        };
        match classify_step(&step, Duration::from_secs(1)) {
            StepClass::Effect(GpuEffect::ShapeMask {
                x,
                y,
                width,
                height,
                ..
            }) => assert_eq!((x, y, width, height), (5, 6, 7, 8)),
            _ => panic!("expected a ShapeMask GPU effect"),
        }
    }

    #[test]
    fn classify_rect_mask_zero_size_should_skip() {
        let step = FilterStep::RectMask {
            x: 0,
            y: 0,
            width: 0,
            height: 10,
            invert: false,
        };
        assert!(matches!(
            classify_step(&step, Duration::ZERO),
            StepClass::Skip
        ));
    }

    #[test]
    fn classify_motion_blur_should_map_to_gpu_effect() {
        let step = FilterStep::MotionBlur {
            shutter_angle_degrees: 180.0,
            sub_frames: 6,
        };
        match classify_step(&step, Duration::ZERO) {
            StepClass::Effect(GpuEffect::MotionBlur {
                shutter_angle,
                sub_frames,
            }) => {
                assert!((shutter_angle - 180.0).abs() < 1e-4);
                assert_eq!(sub_frames, 6);
            }
            _ => panic!("expected a MotionBlur GPU effect"),
        }
    }

    #[test]
    fn classify_animated_motion_blur_should_evaluate_the_shutter_at_the_frame_time() {
        // The point of the animated form: two different `t` give two different
        // shutters. A classifier that collapsed to `t = 0` would return the same
        // value twice, which is what shipped before #1705.
        let track = ff_filter::AnimationTrack::new()
            .push(ff_filter::Keyframe::new(
                Duration::ZERO,
                0.0,
                ff_filter::Easing::Linear,
            ))
            .push(ff_filter::Keyframe::new(
                Duration::from_secs(1),
                360.0,
                ff_filter::Easing::Linear,
            ));
        let step = FilterStep::MotionBlurAnimated {
            shutter_angle: AnimatedValue::Track(track),
            sub_frames: 6,
        };
        let at = |t: Duration| match classify_step(&step, t) {
            StepClass::Effect(GpuEffect::MotionBlur { shutter_angle, .. }) => shutter_angle,
            _ => panic!("expected a MotionBlur GPU effect"),
        };
        let quarter = at(Duration::from_millis(250));
        let three_quarters = at(Duration::from_millis(750));
        assert!(
            (quarter - 90.0).abs() < 1.0,
            "a linear 0->360 ramp is 90 at a quarter in, got {quarter}"
        );
        assert!(
            (three_quarters - 270.0).abs() < 1.0,
            "and 270 at three quarters, got {three_quarters}"
        );
    }

    #[test]
    fn classify_animated_motion_blur_zero_shutter_should_skip() {
        // The degenerate case follows the constant form: a shutter that is zero at
        // the frame time is no blur, not a node that renders nothing.
        let step = FilterStep::MotionBlurAnimated {
            shutter_angle: AnimatedValue::Static(0.0),
            sub_frames: 4,
        };
        assert!(matches!(
            classify_step(&step, Duration::ZERO),
            StepClass::Skip
        ));
    }

    #[test]
    fn classify_motion_blur_zero_shutter_should_skip() {
        let step = FilterStep::MotionBlur {
            shutter_angle_degrees: 0.0,
            sub_frames: 4,
        };
        assert!(matches!(
            classify_step(&step, Duration::ZERO),
            StepClass::Skip
        ));
    }
}
