//! Maps a clip's [`XfadeTransition`] onto the `ff-render` transition node that renders
//! it on the GPU (#1657).
//!
//! The mapping is a pure function of the transition kind: it says *which* node and with
//! *what* geometry, not how to drive it. Progress and the incoming clip's pixels come
//! from whoever runs the transition, which today is the CPU preview path
//! (`ff_preview::apply_xfade`) and, once it is wired, the GPU one.
//!
//! A kind with no GPU equivalent maps to `None`, which is the caller's signal to keep
//! that transition on the CPU rather than render something that is not what the model
//! asked for (RK-020).

use ff_filter::XfadeTransition;

/// The `ff-render` transition node an [`XfadeTransition`] renders as.
///
/// Carries the node's *static* geometry only. `progress` and clip B are supplied per
/// frame by the caller, so this stays comparable and cheap to hand around.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GpuTransition {
    /// Linear cross-blend (`ff_render::FadeTransitionNode`).
    Fade,
    /// Per-pixel threshold reveal (`ff_render::DissolveTransitionNode`).
    Dissolve,
    /// Directional wipe (`ff_render::WipeTransitionNode`), `angle` in **radians**.
    ///
    /// The angle points at the edge clip B grows *from*, which is the opposite of the
    /// transition's name: `wiperight` sweeps its edge rightward and so reveals B from
    /// the left (`angle = PI`). The model names a direction and `ff_render` wants an
    /// angle, so the conversion — and its inversion — happen here rather than being
    /// passed through raw (RK-020).
    Wipe {
        /// Wipe direction in radians.
        angle: f32,
    },
    /// Two-phase dip through a solid colour (`ff_render::DipToColorNode`), RGB in
    /// `[0, 1]`.
    Dip {
        /// Dip colour.
        color: [f32; 3],
    },
}

/// The GPU node for `kind`, or `None` when it has no equivalent and must stay on the
/// CPU path.
///
/// [`XfadeTransition`] is `#[non_exhaustive]` and defined in `ff-filter`, so this match
/// needs a `_` arm (RK-003) and the compiler cannot report a newly added kind here. The
/// substitute guard is the `MAPPED_KINDS` / `CPU_ONLY_KINDS` tables in this module's
/// tests, which fail if a listed kind stops behaving as recorded.
#[must_use]
pub fn map_transition(kind: XfadeTransition) -> Option<GpuTransition> {
    match kind {
        XfadeTransition::Fade => Some(GpuTransition::Fade),
        XfadeTransition::Dissolve => Some(GpuTransition::Dissolve),
        // The angle names the side clip B grows *from*, which is the opposite of the
        // FFmpeg kind's name: `wiperight` sweeps its edge rightward, so B is revealed
        // from the left. `WipeTransitionNode` fills where `proj > center`, i.e. from
        // the high end of the projected axis, so `wiperight` needs the axis flipped.
        XfadeTransition::WipeRight => Some(GpuTransition::Wipe {
            angle: std::f32::consts::PI,
        }),
        XfadeTransition::WipeLeft => Some(GpuTransition::Wipe { angle: 0.0 }),
        XfadeTransition::WipeDown => Some(GpuTransition::Wipe {
            angle: -std::f32::consts::FRAC_PI_2,
        }),
        XfadeTransition::WipeUp => Some(GpuTransition::Wipe {
            angle: std::f32::consts::FRAC_PI_2,
        }),
        XfadeTransition::FadeBlack => Some(GpuTransition::Dip {
            color: [0.0, 0.0, 0.0],
        }),
        XfadeTransition::FadeWhite => Some(GpuTransition::Dip {
            color: [1.0, 1.0, 1.0],
        }),
        // Slides need a translating sampler, and the geometric / mosaic kinds need
        // nodes that do not exist yet; all stay on the CPU path.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind that maps to a GPU node, with the node it maps to.
    ///
    /// Stands in for the exhaustive match the `#[non_exhaustive]` enum denies us: the test
    /// below walks this list, so a mapping that silently changes is caught even though a
    /// *newly added* variant cannot be.
    const MAPPED_KINDS: &[(XfadeTransition, GpuTransition)] = &[
        (XfadeTransition::Fade, GpuTransition::Fade),
        (XfadeTransition::Dissolve, GpuTransition::Dissolve),
        (
            XfadeTransition::WipeRight,
            GpuTransition::Wipe {
                angle: std::f32::consts::PI,
            },
        ),
        (
            XfadeTransition::WipeLeft,
            GpuTransition::Wipe { angle: 0.0 },
        ),
        (
            XfadeTransition::WipeDown,
            GpuTransition::Wipe {
                angle: -std::f32::consts::FRAC_PI_2,
            },
        ),
        (
            XfadeTransition::WipeUp,
            GpuTransition::Wipe {
                angle: std::f32::consts::FRAC_PI_2,
            },
        ),
        (
            XfadeTransition::FadeBlack,
            GpuTransition::Dip {
                color: [0.0, 0.0, 0.0],
            },
        ),
        (
            XfadeTransition::FadeWhite,
            GpuTransition::Dip {
                color: [1.0, 1.0, 1.0],
            },
        ),
    ];

    /// Every kind deliberately kept on the CPU path.
    const CPU_ONLY_KINDS: &[XfadeTransition] = &[
        XfadeTransition::SlideLeft,
        XfadeTransition::SlideRight,
        XfadeTransition::SlideUp,
        XfadeTransition::SlideDown,
        XfadeTransition::CircleOpen,
        XfadeTransition::CircleClose,
        XfadeTransition::FadeGrays,
        XfadeTransition::Pixelize,
    ];

    #[test]
    fn map_transition_should_return_the_recorded_node_for_every_mapped_kind() {
        for (kind, want) in MAPPED_KINDS {
            assert_eq!(
                map_transition(*kind),
                Some(*want),
                "{kind:?} should map to {want:?}"
            );
        }
    }

    #[test]
    fn map_transition_should_return_none_for_every_cpu_only_kind() {
        // AC2's first half: an unsupported kind must fall back rather than render an
        // approximation of what the model asked for.
        for kind in CPU_ONLY_KINDS {
            assert_eq!(
                map_transition(*kind),
                None,
                "{kind:?} has no GPU node and must fall back to CPU"
            );
        }
    }

    #[test]
    fn mapped_and_cpu_only_kinds_should_not_overlap() {
        // The two lists together are the coverage claim, so an entry appearing in both
        // would make that claim contradictory rather than merely incomplete.
        for (kind, _) in MAPPED_KINDS {
            assert!(
                !CPU_ONLY_KINDS.contains(kind),
                "{kind:?} is listed as both mapped and CPU-only"
            );
        }
    }

    #[test]
    fn wipe_angles_should_oppose_their_named_direction() {
        // The angle is the one unit conversion in this module and it is *opposite* the
        // kind's name: `WipeTransitionNode` fills from the high end of the projected
        // axis, while `wiperight` reveals clip B from the left. Writing the intuitive
        // pairing here produced a mirrored wipe that the transition parity suite caught
        // at mean=127.5, so pin the counter-intuitive direction explicitly.
        let angle_of = |kind| match map_transition(kind) {
            Some(GpuTransition::Wipe { angle }) => angle,
            other => panic!("{kind:?} must map to a wipe, got {other:?}"),
        };
        assert!(
            angle_of(XfadeTransition::WipeRight).cos() < -0.99,
            "wiperight reveals B from the left, so its axis points -x"
        );
        assert!(
            angle_of(XfadeTransition::WipeLeft).cos() > 0.99,
            "wipeleft reveals B from the right, so its axis points +x"
        );
        assert!(
            angle_of(XfadeTransition::WipeDown).sin() < -0.99,
            "wipedown reveals B from the top, so its axis points -y"
        );
        assert!(
            angle_of(XfadeTransition::WipeUp).sin() > 0.99,
            "wipeup reveals B from the bottom, so its axis points +y"
        );
    }
}
