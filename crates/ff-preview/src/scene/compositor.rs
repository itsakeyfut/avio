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

use ff_filter::{RealtimeLayer, XfadeTransition};
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

    /// Blend the outgoing frame `a` into the incoming frame `b` at `progress`
    /// (`0` = all `a`, `1` = all `b`) for the `xfade` `kind`, both packed RGBA of
    /// `w * h * 4` bytes.
    ///
    /// Returns `Some(rgba)` on success, or `None` to leave the frame to the runner's
    /// CPU `apply_xfade` — which is the answer for a kind the implementor does not
    /// render, a missing adapter, a GPU error, and a kind it renders correctly but
    /// slower. Declining must never leave the runner in a bad state.
    ///
    /// Defaults to `None`, so an implementor that only composites is unaffected.
    ///
    /// This sits beside `composite` rather than in a trait of its own because both
    /// exist for the same reason — reaching `ff-render`, which depends on this crate —
    /// and one injected object means one GPU context rather than two.
    fn blend(
        &mut self,
        kind: XfadeTransition,
        a: &[u8],
        b: &[u8],
        progress: f32,
        w: u32,
        h: u32,
    ) -> Option<Vec<u8>> {
        let _ = (kind, a, b, progress, w, h);
        None
    }

    /// Drops whatever the implementor carries from one clip into the next.
    ///
    /// The runner calls this when playback crosses a clip boundary. It exists for a
    /// **stateful** effect: motion blur accumulates an exposure trail across the
    /// frames of one clip, and without a reset at the cut the outgoing clip's trail
    /// bleeds into the incoming clip's first frame. The export path has always done
    /// this; playback did not, which is what #1705 fixes.
    ///
    /// Defaults to a no-op, so an implementor that carries nothing across frames is
    /// unaffected.
    fn reset_effects(&mut self) {}
}
