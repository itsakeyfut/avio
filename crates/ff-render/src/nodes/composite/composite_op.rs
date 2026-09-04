//! `CompositeOp`: the Porter-Duff alpha-compositing operators.
//!
//! # Reference
//!
//! These are the operators from W3C Compositing and Blending Level 1, whose
//! general form is
//!
//! ```text
//! Co = as * Fa * Cs + ab * Fb * Cb
//! ao = as * Fa      + ab * Fb
//! ```
//!
//! with `Fa` and `Fb` chosen per operator. Every doc comment below gives the
//! premultiplied form the shader and [`blend_math`](super::blend_math) actually
//! evaluate, where `s = as * Cs` is the premultiplied source, `d = ab * Cb` the
//! premultiplied backdrop, and `sa` / `da` their alphas. The two are the same
//! thing: substituting `s` and `d` into `co = s * Fa + d * Fb` reproduces the
//! spec's expression. Skia states the same set as one-liners on premultiplied
//! colour (`kSrcIn: r = s * da`), and Natron's compositor cites these sections
//! directly.
//!
//! # Relationship to `BlendMode`
//!
//! [`BlendMode`](super::BlendMode) is a *colour* function of two pixels;
//! `CompositeOp` is *alpha* algebra deciding how much of each side survives.
//! W3C applies them in that order, blend then composite, which is how
//! `shaders/blend.wgsl` is written. avio's editing model does not combine them:
//! a layer with a non-`Over` composite has its blend mode ignored, so
//! `avio::gpu::map_scene` emits `Normal` for those layers.
//!
//! # Why the CPU path differs
//!
//! `ff_filter::CompositeOp` builds In/Out/Atop/Xor from `blend`'s `all_expr`,
//! which is per-channel arithmetic rather than alpha compositing: `FFmpeg` has
//! no Porter-Duff filter, and `all_expr` can only reference the same plane of
//! both inputs, so it cannot express a colour term that depends on the other
//! input's alpha. The GPU implements the real operators; the divergence is
//! recorded on #1670.

/// Porter-Duff alpha-compositing operator.
///
/// The discriminant is the value written into the shader's `composite` uniform,
/// so variants are only ever **appended**, never renumbered. Each doc comment
/// gives the premultiplied output colour `co` and alpha `ao`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum CompositeOp {
    /// Source over destination. `co = s + d(1-sa)`, `ao = sa + da(1-sa)`.
    #[default]
    Over = 0,
    /// Destination over source. `co = d + s(1-da)`, `ao = da + sa(1-da)`.
    Under = 1,
    /// Source shown only where the destination is opaque.
    /// `co = s·da`, `ao = sa·da`.
    In = 2,
    /// Source shown only where the destination is transparent.
    /// `co = s(1-da)`, `ao = sa(1-da)`.
    Out = 3,
    /// Source atop the destination; the destination's shape is kept.
    /// `co = s·da + d(1-sa)`, `ao = da`.
    Atop = 4,
    /// Whichever side the other does not cover.
    /// `co = s(1-da) + d(1-sa)`, `ao = sa(1-da) + da(1-sa)`.
    Xor = 5,
}

#[cfg(test)]
mod tests {
    use super::CompositeOp;

    /// The discriminant is the shader's `composite` uniform value, so
    /// renumbering a variant silently changes what `blend.wgsl` composites. A
    /// new variant needs a row here and a matching `case` in the shader.
    #[test]
    fn composite_op_discriminants_should_match_the_shader_codes() {
        let expected = [
            (CompositeOp::Over, 0),
            (CompositeOp::Under, 1),
            (CompositeOp::In, 2),
            (CompositeOp::Out, 3),
            (CompositeOp::Atop, 4),
            (CompositeOp::Xor, 5),
        ];
        for (op, code) in expected {
            assert_eq!(op as u32, code, "{op:?} moved to a different code");
        }
        assert_eq!(expected.len(), 6);
    }

    #[test]
    fn composite_op_should_default_to_over() {
        assert_eq!(CompositeOp::default(), CompositeOp::Over);
    }
}
