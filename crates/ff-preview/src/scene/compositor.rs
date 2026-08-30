//! Pluggable layer compositor for the preview runner.
//!
//! The runner composites each frame with the built-in CPU compositor
//! ([`RealtimeComposer`](ff_filter::RealtimeComposer)). A caller that has a GPU
//! compositor (which `ff-preview` cannot reach directly, since `ff-render` depends
//! on `ff-preview`, not the reverse) can inject one through this seam: the runner
//! tries the injected compositor first and falls back to the CPU path whenever it
//! returns `None` (no GPU, an unsupported layer, or a GPU error). This is how the
//! `avio` engine wires `ff-render` into the preview without a dependency cycle.

use std::time::Duration;

use ff_filter::RealtimeLayer;
use ff_format::VideoFrame;

/// An external compositor the preview runner can use in place of its built-in CPU
/// compositor. Implemented by `avio` over `ff-render`; see the module docs.
pub trait PreviewCompositor: Send {
    /// Composite `layers` (bottom to top, paired with each layer's decoded `rgba`
    /// frame) into a single `rgba` frame at timeline time `t`, targeting the
    /// `canvas` output size.
    ///
    /// Returns `Some((rgba, width, height))` on success, or `None` to fall back to
    /// the runner's CPU compositor (an unsupported layer, no adapter, or a GPU
    /// error). Returning `None` must never leave the runner in a bad state.
    fn composite(
        &mut self,
        layers: &[(&RealtimeLayer, &VideoFrame)],
        canvas: (u32, u32),
        t: Duration,
    ) -> Option<(Vec<u8>, u32, u32)>;
}
