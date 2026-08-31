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
//! compositor. The mapping never silently drops a step. v1 covers colour grade
//! (`Eq`) and plain scale as per-layer effects; broader node coverage is tracked as
//! a follow-up.

use std::time::Duration;

use ff_filter::{
    AnimatedValue, BlendMode, CompositeOp, FilterStep, RealtimeLayer, RealtimeLayerDescriptor,
    ScaleAlgorithm, VideoLayer,
};
use ff_render::{BlendMode as RenderBlendMode, ScaleAlgorithm as RenderScaleAlgorithm};

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
    /// `ff_render::ColorGradeNode` (brightness / contrast / saturation; temperature
    /// and tint are neutral until a source carries them).
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
}

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
        // Check composite first: the blend mode is only applied when the composite
        // op is `Over` (see `VideoLayer`), so for any other op the composite is the
        // semantically active reason to fall back. `Over` is the only plain
        // top-over-bottom composite; the others need node wiring the compositor
        // does not provide yet.
        if !matches!(layer.composite_op(), CompositeOp::Over) {
            return GpuMapping::Fallback(GpuFallback::UnsupportedCompositeOp(layer.composite_op()));
        }
        let Some(blend_mode) = map_blend_mode(layer.blend_mode()) else {
            return GpuMapping::Fallback(GpuFallback::UnsupportedBlendMode(layer.blend_mode()));
        };

        let mut effects = Vec::new();
        for step in layer.effects() {
            match classify_step(step, t) {
                StepClass::Skip => {}
                StepClass::Effect(effect) => effects.push(effect),
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
            rotation: eval_at(layer.rotation(), t),
            opacity: eval_at(layer.opacity(), t).clamp(0.0, 1.0),
            blend_mode,
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
    Unsupported,
}

#[allow(clippy::cast_possible_truncation)]
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
        } => StepClass::Effect(GpuEffect::ColorGrade {
            brightness: *brightness,
            contrast: *contrast,
            saturation: *saturation,
            temperature: 0.0,
            tint: 0.0,
        }),
        FilterStep::EqAnimated {
            brightness,
            contrast,
            saturation,
            gamma,
        } => {
            // ff-render's ColorGradeNode has no gamma, so only an `eq` whose gamma
            // is neutral at `t` maps; otherwise fall back so gamma is not dropped.
            if (gamma.value_at(t) - 1.0).abs() < 1e-6 {
                StepClass::Effect(GpuEffect::ColorGrade {
                    brightness: brightness.value_at(t) as f32,
                    contrast: contrast.value_at(t) as f32,
                    saturation: saturation.value_at(t) as f32,
                    temperature: 0.0,
                    tint: 0.0,
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
        FilterStep::GBlur { sigma } => StepClass::Effect(GpuEffect::Blur { sigma: *sigma }),
        FilterStep::GBlurAnimated { sigma } => StepClass::Effect(GpuEffect::Blur {
            // The blur node is rebuilt each composite (map_scene runs per frame), so
            // an animated sigma is evaluated at the frame time here.
            sigma: sigma.value_at(t) as f32,
        }),
        // Everything else (other colour, keying, masks, animated geometry,
        // xfade, ...) has no GPU node yet. `_` is required: `FilterStep` is
        // `#[non_exhaustive]` from ff-filter (RK-003).
        _ => StepClass::Unsupported,
    }
}

/// Maps `ff_filter::BlendMode` to the `ff_render::BlendMode` intersection, or `None`
/// when `ff-render` has no equivalent (forcing fallback).
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
        // No ff-render equivalent. `_` is required: `BlendMode` is
        // `#[non_exhaustive]` from ff-filter (RK-003).
        _ => return None,
    })
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
    use ff_filter::{AnimationTrack, Easing, Keyframe};

    use super::*;
    use crate::Clip;
    use crate::track::TrackAutomation;

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
        layer.effects = vec![FilterStep::Eq {
            brightness: 0.5,
            contrast: 1.2,
            saturation: 0.8,
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::ColorGrade {
                brightness: 0.5,
                contrast: 1.2,
                saturation: 0.8,
                temperature: 0.0,
                tint: 0.0,
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
        }];
        let plan = gpu(map_scene(&[layer], (16, 16), Duration::ZERO));
        assert_eq!(
            plan.layers[0].effects.as_slice(),
            [GpuEffect::ColorGrade {
                brightness: 0.3,
                contrast: 1.1,
                saturation: 0.9,
                temperature: 0.0,
                tint: 0.0,
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
        let mut layer = TestLayer::identity();
        layer.effects = vec![FilterStep::Hue { degrees: 30.0 }];
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedEffect)
        );
    }

    #[test]
    fn map_scene_should_fall_back_on_unsupported_blend_mode() {
        let mut layer = TestLayer::identity();
        layer.blend_mode = BlendMode::Glow;
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedBlendMode(BlendMode::Glow))
        );
    }

    #[test]
    fn map_scene_should_fall_back_on_non_over_composite() {
        let mut layer = TestLayer::identity();
        layer.composite_op = CompositeOp::Under;
        assert_eq!(
            map_scene(&[layer], (16, 16), Duration::ZERO),
            GpuMapping::Fallback(GpuFallback::UnsupportedCompositeOp(CompositeOp::Under))
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
        let video_layer = crate::derive::video_layer(&clip, 0, &auto, 1920, 1080, None, None);
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
}
