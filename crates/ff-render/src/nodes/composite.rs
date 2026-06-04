//! Compositing nodes, split by node type.
//!
//! This module is the coordinator: it declares the per-node submodules and
//! re-exports the public node types so existing `nodes::composite::*` paths
//! are unchanged. The GPU pipeline helpers are re-exported `pub(crate)` so the
//! multi-layer compositor keeps importing them from `crate::nodes::composite`.

mod blend_math;
mod blend_mode;
mod chroma_key;
mod helpers;
mod masks;
mod transform;

pub use blend_mode::{BlendMode, BlendModeNode};
pub use chroma_key::ChromaKeyNode;
pub use masks::{AlphaMatteNode, LumaMaskNode, ShapeMaskNode};
pub use transform::TransformNode;

#[cfg(feature = "wgpu")]
pub(crate) use helpers::{
    fullscreen_pipeline, linear_sampler, submit_render_pass, two_tex_sampler_uniform_bgl,
    upload_rgba_texture,
};
