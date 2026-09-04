//! Multi-layer GPU compositing: [`FrameLayer`] stack in, one texture out.
//!
//! # Alpha convention
//!
//! The canvas starts transparent (its texture is zero-initialised) and each
//! layer is composited onto it in `z_order`. Two rules, which
//! `shaders/blend.wgsl` and [`nodes::BlendModeNode`](crate::BlendModeNode)
//! implement identically:
//!
//! - **Colour** is composited against an **opaque black backdrop**, so the blend
//!   result is not reweighted by the backdrop alpha the way W3C's
//!   `Cs' = (1 - ab) * Cs + ab * B(Cb, Cs)` would. That matches the CPU
//!   compositor's `color=c=#000000` canvas, which ADR-0007 keeps as the
//!   correctness reference.
//! - **Alpha** accumulates as src-over **coverage**, `ao = as + ab * (1 - as)`:
//!   zero where no layer has drawn (letterbox bands, the transparent regions a
//!   transform leaves behind), rising toward one as opaque layers cover it
//!   (#1750).
//! - **RGB is premultiplied** by that alpha. A white layer composited at opacity
//!   `0.5` over the empty canvas reads back `(128, 128, 128, 128)`, not
//!   `(255, 255, 255, 128)`. A consumer that wants straight alpha divides by
//!   `ao`, guarding `ao == 0`.
//!
//! Consumers that flatten to an opaque format (the export path converts rgba to
//! yuv420p) ignore the alpha and read the premultiplied RGB directly, which is
//! the same thing as compositing over black; consumers that composite further,
//! or that need to know what a layer covered, read the alpha.

#[cfg(feature = "wgpu")]
mod compositor_inner;

use ff_format::VideoFrame;

use crate::nodes::{BlendMode, CompositeOp};

// LayerTransform

/// 2D affine transform parameters for a compositor layer.
///
/// All values use UV-space coordinates where 0.0 is no change. The default
/// (identity) transform leaves the layer centred and unscaled.
#[derive(Debug, Clone)]
pub struct LayerTransform {
    /// Horizontal UV-space offset (positive = shift right). Default: `0.0`.
    pub x: f32,
    /// Vertical UV-space offset (positive = shift down). Default: `0.0`.
    pub y: f32,
    /// Horizontal scale factor (`1.0` = no change). Default: `1.0`.
    pub scale_x: f32,
    /// Vertical scale factor (`1.0` = no change). Default: `1.0`.
    pub scale_y: f32,
    /// Counter-clockwise rotation in radians. Default: `0.0`.
    pub rotation: f32,
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
        }
    }
}

impl LayerTransform {
    /// Returns `true` when this transform is the identity (no visual change).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.x.abs() < 1e-6
            && self.y.abs() < 1e-6
            && (self.scale_x - 1.0).abs() < 1e-6
            && (self.scale_y - 1.0).abs() < 1e-6
            && self.rotation.abs() < 1e-6
    }
}

// FrameLayer

/// A single layer in the composition stack.
pub struct FrameLayer {
    /// Source video frame (uploaded to GPU by [`Compositor`]).
    pub frame: VideoFrame,
    /// 2D affine transform applied before compositing.
    pub transform: LayerTransform,
    /// Blend mode used when compositing this layer over layers below.
    ///
    /// Only meaningful with [`CompositeOp::Over`]: the editing model does not
    /// combine the two, so `avio::gpu::map_scene` emits `Normal` for a layer
    /// whose composite operator is anything else.
    pub blend_mode: BlendMode,
    /// Porter-Duff operator deciding how much of this layer and the canvas
    /// below survive. Defaults to [`CompositeOp::Over`].
    pub composite_op: CompositeOp,
    /// Layer opacity (`0.0` = transparent, `1.0` = fully opaque).
    pub opacity: f32,
    /// Z-order — lower values are further back. Layers are sorted ascending
    /// by this field before compositing.
    pub z_order: i32,
}

// Compositor

/// Stateful high-level multi-layer GPU compositor.
///
/// Accepts a list of [`FrameLayer`]s, sorts them by [`FrameLayer::z_order`],
/// uploads each frame to the GPU, applies per-layer transforms and blend modes,
/// and returns the composited [`wgpu::Texture`].
///
/// The wgpu render pipeline is built on the first call to
/// [`composite`](Self::composite) and reused across frames. It is rebuilt only
/// when the number of layers changes.
///
/// # Thread safety
///
/// `Compositor` is [`Send`] and can be moved to a background thread. When
/// multiple threads need to share a compositor, wrap it in
/// `Arc<Mutex<Compositor>>`.
///
/// Requires the `wgpu` feature.
#[cfg(feature = "wgpu")]
pub struct Compositor {
    ctx: std::sync::Arc<crate::context::RenderContext>,
    width: u32,
    height: u32,
    graph: Option<compositor_inner::CompositorGraph>,
    last_layer_count: usize,
}

#[cfg(feature = "wgpu")]
impl Compositor {
    /// Create a compositor targeting the given output resolution.
    #[must_use]
    pub fn new(
        ctx: std::sync::Arc<crate::context::RenderContext>,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            ctx,
            width,
            height,
            graph: None,
            last_layer_count: 0,
        }
    }

    /// Composite `layers` into a single [`wgpu::Texture`].
    ///
    /// Layers are sorted by [`FrameLayer::z_order`] before compositing
    /// (ascending — lowest `z_order` is the bottom layer).
    ///
    /// The wgpu pipeline is built on the first call and cached; it is rebuilt
    /// only when `layers.len()` changes between calls.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`](crate::error::RenderError) on GPU texture
    /// creation failure, unsupported pixel format, or render failure.
    pub fn composite(
        &mut self,
        layers: &mut [FrameLayer],
    ) -> Result<wgpu::Texture, crate::error::RenderError> {
        layers.sort_unstable_by_key(|l| l.z_order);

        let need_rebuild = self.graph.is_none() || self.last_layer_count != layers.len();
        if need_rebuild {
            self.graph = Some(compositor_inner::CompositorGraph::build(
                &self.ctx,
                layers.len(),
                self.width,
                self.height,
            ));
            self.last_layer_count = layers.len();
        }

        let Some(graph) = self.graph.as_mut() else {
            return Err(crate::error::RenderError::Composite {
                message: "compositor graph not initialized".to_string(),
            });
        };
        graph.composite(&self.ctx, layers, self.width, self.height)
    }

    /// Composite `layers` and read the result back to a dense `rgba` buffer
    /// (`width * height * 4` bytes, `Rgba8Unorm`), returning `(rgba, width, height)`.
    ///
    /// A convenience over [`composite`](Self::composite) for callers that need the
    /// pixels on the CPU (e.g. handing a frame to an encoder or a CPU frame sink).
    /// Prefer [`composite`](Self::composite) plus the zero-copy display path when a
    /// GPU-resident texture is enough.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`](crate::error::RenderError) on composite failure or if
    /// the GPU-to-CPU readback fails.
    pub fn composite_to_rgba(
        &mut self,
        layers: &mut [FrameLayer],
    ) -> Result<(Vec<u8>, u32, u32), crate::error::RenderError> {
        let texture = self.composite(layers)?;
        let rgba =
            compositor_inner::read_texture_rgba(&self.ctx, &texture, self.width, self.height)?;
        Ok((rgba, self.width, self.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::{PixelFormat, VideoFrame};

    fn make_frame() -> VideoFrame {
        VideoFrame::empty(2, 2, PixelFormat::Rgba).expect("test frame")
    }

    #[test]
    fn layer_transform_default_should_be_identity() {
        let t = LayerTransform::default();
        assert!(
            t.is_identity(),
            "default LayerTransform must be the identity"
        );
    }

    #[test]
    fn layer_transform_nonzero_x_should_not_be_identity() {
        let t = LayerTransform {
            x: 0.1,
            ..Default::default()
        };
        assert!(
            !t.is_identity(),
            "LayerTransform with non-zero x must not be identity"
        );
    }

    #[test]
    fn frame_layer_should_construct_with_defaults() {
        let layer = FrameLayer {
            frame: make_frame(),
            transform: LayerTransform::default(),
            blend_mode: BlendMode::Normal,
            composite_op: CompositeOp::Over,
            opacity: 1.0,
            z_order: 0,
        };
        assert_eq!(layer.z_order, 0);
        assert!((layer.opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn compositor_layers_should_sort_by_z_order() {
        let mut layers = vec![
            FrameLayer {
                frame: make_frame(),
                transform: LayerTransform::default(),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                opacity: 1.0,
                z_order: 3,
            },
            FrameLayer {
                frame: make_frame(),
                transform: LayerTransform::default(),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                opacity: 1.0,
                z_order: 1,
            },
            FrameLayer {
                frame: make_frame(),
                transform: LayerTransform::default(),
                blend_mode: BlendMode::Normal,
                composite_op: CompositeOp::Over,
                opacity: 1.0,
                z_order: 2,
            },
        ];
        layers.sort_unstable_by_key(|l| l.z_order);
        let z_orders: Vec<i32> = layers.iter().map(|l| l.z_order).collect();
        assert_eq!(
            z_orders,
            vec![1, 2, 3],
            "layers must sort ascending by z_order"
        );
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn compositor_should_be_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Compositor>();
    }
}
