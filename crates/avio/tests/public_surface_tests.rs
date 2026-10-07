//! The public surface, asserted from outside the crate.
//!
//! `lib.rs` has its own `#[cfg(test)] mod tests` naming many of the same items. That is
//! not a duplicate of this file, and neither should be deleted in favour of the other:
//! they verify different properties. A unit test resolves names through `use super::*`,
//! which sees crate-internal items, so a `pub use` demoted to `pub(crate) use` still
//! compiles there. Measured: that demotion on `GpuCompositor` leaves the unit test
//! green and fails this file with `E0603: struct GpuCompositor is private`. Only an
//! external crate path enforces `pub`.
//!
//! That matters here because CLAUDE.md makes re-exporting every new public type into
//! `avio/src/lib.rs` a rule, so the re-export list is edited often, and because CI
//! cannot see a regression in it any other way: the feature powerset job runs
//! `cargo hack check --each-feature --no-dev-deps`, which proves each combination
//! builds but never compiles a test.
//!
//! Compile-only: every assertion is name resolution, so this file needs no `FFmpeg`, no
//! fixture and no GPU adapter, and runs on a minimal CI build.

// Always-present value types
//
// These are `pub use ff_format::{...}` in the engine. Four of them (`AudioCodec`,
// `SampleFormat`, `ChannelLayout`, `MediaInfo`) are named by no other test outside the
// crate, so this is the only thing standing between them and a silent demotion.

#[test]
fn format_value_types_should_be_publicly_reachable() {
    let _: avio::VideoCodec = avio::VideoCodec::default();
    let _: avio::AudioCodec = avio::AudioCodec::default();
    let _: avio::PixelFormat = avio::PixelFormat::default();
    let _: avio::SampleFormat = avio::SampleFormat::default();
    let _: avio::ChannelLayout = avio::ChannelLayout::default();
    let _: avio::Rational = avio::Rational::default();
    let _: avio::Timestamp = avio::Timestamp::default();
    // `Timeline::timecode_at` returns both of these, so a caller depending only on `avio`
    // has to be able to name them: every public type the engine hands out is re-exported
    // from this crate.
    let tc: avio::Timecode = avio::Timecode::at(0, avio::Rational::new(30, 1)).unwrap();
    let _: u64 = tc.frame();
    let _: Result<avio::Timecode, avio::TimecodeError> =
        avio::Timecode::parse("00:00:00:00", avio::Rational::new(30, 1));
    let _: avio::MediaInfo = avio::MediaInfo::default();
}

// preview feature

#[cfg(feature = "preview")]
#[test]
fn preview_surface_should_be_publicly_reachable() {
    let _ = std::mem::size_of::<avio::TimelinePlayer>();
    let _ = std::mem::size_of::<avio::SceneRunner>();
    let _ = std::mem::size_of::<avio::Scene>();
    let _ = std::mem::size_of::<avio::PlayerHandle>();
    // A runner's only output channel is the sink `SceneRunner::set_sink` takes, so a
    // consumer that cannot name these cannot observe the preview at all.
    let _ = std::mem::size_of::<avio::RgbaSink>();
    let _: Option<Box<dyn avio::FrameSink>> = None;
}

// gpu feature

#[cfg(feature = "gpu")]
#[test]
fn gpu_bridge_surface_should_be_publicly_reachable() {
    // The scene-to-GPU mapping a host inspects to see what the compositor will do.
    let _ = std::mem::size_of::<avio::GpuCompositor>();
    let _ = std::mem::size_of::<avio::GpuScenePlan>();
    let _ = std::mem::size_of::<avio::GpuLayerPlan>();
    let _ = std::mem::size_of::<avio::GpuMapping>();
    let _ = std::mem::size_of::<avio::GpuEffect>();
    let _ = std::mem::size_of::<avio::GpuFallback>();
    let _ = std::mem::size_of::<avio::GpuTransition>();
    // Both mappers are free functions, so naming the types would not catch their loss.
    // Bound as typed function pointers, which pins each signature too. `RealtimeLayer`
    // is the `GpuLayerSource` the preview path feeds `map_scene`.
    let _: fn(avio::XfadeTransition) -> Option<avio::GpuTransition> = avio::map_transition;
    let _: fn(&[avio::RealtimeLayer], (u32, u32), std::time::Duration) -> avio::GpuMapping =
        avio::map_scene;
}

#[cfg(all(feature = "gpu", feature = "preview"))]
#[test]
fn gpu_preview_compositor_should_be_publicly_reachable() {
    // The one symbol that needs both features, so neither section alone covers it.
    let _ = std::mem::size_of::<avio::GpuPreviewCompositor>();
}
