//! The one rule for where a transition sits and how long it lasts (ADR-0009).
//!
//! A transition into clip `B` occupies `[B.offset, B.offset + D)` and is fed by the
//! outgoing clip's *handle* -- its frames past its out-point. The timeline length never
//! changes, so no clip moves and audio stays where it was authored.
//!
//! Every derivation goes through [`effective_duration`]: the CPU export's `xfade`, the
//! GPU export's drain, and the preview projection. They cannot compute different answers
//! and drift apart again, which is what #1731 and #1737 were.

use std::time::Duration;

use crate::clip::{Clip, ClipSource};

/// The transition duration actually realisable at the boundary from `outgoing` into
/// `incoming`, or [`Duration::ZERO`] when `incoming` carries no transition.
///
/// The authored duration is clamped to the handle `outgoing` has left past its
/// out-point, because that is the material the blend reads. Probing the source is how
/// the handle becomes known, so this is I/O and belongs here rather than in the pure
/// `derive` pass.
pub(crate) fn effective_duration(outgoing: &Clip, incoming: &Clip) -> Duration {
    if incoming.transition.is_none() {
        return Duration::ZERO;
    }
    clamp_to_handle(incoming.transition_duration, video_handle(outgoing))
}

/// Every boundary of one track, resolved in a single pass: index `i` is the transition
/// **into** `clips[i]`, so index `0` is always [`Duration::ZERO`].
///
/// Each clip sits at two boundaries -- its own transition, and the handle its successor's
/// transition needs -- and both are the same number. Resolving them per clip asks the
/// same source for the same fact twice, and this is the one part of the rule that opens
/// a file, so callers that walk a whole track resolve the track instead.
pub(crate) fn effective_durations(clips: &[Clip]) -> Vec<Duration> {
    clips
        .iter()
        .enumerate()
        .map(|(i, clip)| match i.checked_sub(1) {
            Some(prev) => effective_duration(&clips[prev], clip),
            // The track's first clip has no predecessor to cross-fade from; `derive`
            // drops a transition there with a warning.
            None => Duration::ZERO,
        })
        .collect()
}

/// The authored duration, cut down to the handle when there is not enough of it.
///
/// `handle` of `None` means unbounded (a generated source synthesises frames forever).
/// Split out from [`effective_duration`] so the rule itself is testable without a file
/// on disk.
fn clamp_to_handle(authored: Duration, handle: Option<Duration>) -> Duration {
    let Some(handle) = handle else {
        return authored;
    };
    if handle >= authored {
        return authored;
    }
    log::warn!(
        "transition shortened to the available handle: authored={:.3}s handle={:.3}s",
        authored.as_secs_f64(),
        handle.as_secs_f64()
    );
    handle
}

/// How much timeline time `clip` can keep producing video for past its out-point.
///
/// `None` is unbounded. `Some(Duration::ZERO)` means no handle at all, which degrades
/// the boundary to a hard cut:
///
/// - **no out-point** -- the clip already runs to end-of-file, so there is nothing past
///   it;
/// - **an unreadable source** -- conservative, and keeps every route on the same answer
///   rather than letting one of them discover the shortfall at EOF.
///
/// The handle is measured in *timeline* time, like the transition duration it is clamped
/// against, so a speed-changed clip's source handle is divided by its speed the same way
/// its body is. Converting it back for a trim or a decode bound is [`to_source`].
fn video_handle(clip: &Clip) -> Option<Duration> {
    // A generated source has no end: `color`/`drawtext` synthesise frames indefinitely,
    // so the authored duration is always available.
    let path = match &clip.source {
        ClipSource::File(path) => path.as_path(),
        ClipSource::Text(_) | ClipSource::Solid(_) => return None,
    };
    let Some(out_point) = clip.out_point else {
        // Already running to end-of-file: nothing sits behind it. Not `None`, which
        // this function reads as *unbounded*.
        return Some(Duration::ZERO);
    };
    let source_duration = match ff_probe::open(path) {
        Ok(info) => info.duration(),
        Err(e) => {
            log::warn!(
                "transition handle unknown, treating the boundary as a hard cut \
                 path={} error={e}",
                path.display()
            );
            return Some(Duration::ZERO);
        }
    };
    let handle = source_duration.saturating_sub(out_point);
    Some(if (clip.speed - 1.0).abs() < 1e-9 {
        handle
    } else {
        handle.div_f64(clip.speed.max(0.01))
    })
}

/// A timeline-time span converted to the source time a trim or a decode bound is written
/// in: at speed 2.0 half a second on screen is a second of source.
///
/// The transition duration and the handle are timeline quantities, but `FilterStep::Trim`
/// runs *before* `FilterStep::Speed` and so reads source seconds. Getting the two mixed
/// up shortens (or overruns) the handle by the speed factor, which is invisible at the
/// speed 1.0 every other test uses.
pub(crate) fn to_source(span: Duration, speed: f64) -> Duration {
    if (speed - 1.0).abs() < 1e-9 {
        return span;
    }
    // Not `Duration::mul_f64`: that panics on a non-finite or overflowing product, and
    // `speed` arrives here straight from the model, which never validates it. An absurd
    // speed should degrade to the unscaled span, not abort the export.
    Duration::try_from_secs_f64(span.as_secs_f64() * speed.max(0.01)).unwrap_or(span)
}

/// The seconds a clip contributes to its track's **composited** stream: its trimmed span
/// after `Speed` has scaled it.
///
/// This is the basis the `xfade` offset is measured in, because the filter sits at the
/// end of the layer chain, downstream of `Speed`. Counting the raw source span instead
/// overstates every later clip's start by the speed factor, and because the accumulator
/// carries forward, the error compounds along the track.
///
/// The consequence is not observable end to end today: a transition on a track holding a
/// speed-changed clip fails to configure its graph at all (#1739), so this is pinned by
/// the unit tests below rather than by an export.
pub(crate) fn composited_secs(source_secs: f64, speed: f64) -> f64 {
    source_secs / speed.max(0.01)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_filter::XfadeTransition;

    #[test]
    fn clamp_to_handle_should_keep_the_authored_duration_when_the_handle_is_unbounded() {
        let authored = Duration::from_millis(500);
        assert_eq!(clamp_to_handle(authored, None), authored);
    }

    #[test]
    fn clamp_to_handle_should_keep_the_authored_duration_when_the_handle_covers_it() {
        let authored = Duration::from_millis(500);
        assert_eq!(
            clamp_to_handle(authored, Some(Duration::from_millis(500))),
            authored,
            "a handle exactly as long as the transition is enough"
        );
        assert_eq!(
            clamp_to_handle(authored, Some(Duration::from_secs(2))),
            authored
        );
    }

    #[test]
    fn clamp_to_handle_should_shorten_to_a_partial_handle() {
        assert_eq!(
            clamp_to_handle(Duration::from_millis(500), Some(Duration::from_millis(200))),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn clamp_to_handle_should_degrade_to_a_hard_cut_with_no_handle() {
        assert_eq!(
            clamp_to_handle(Duration::from_millis(500), Some(Duration::ZERO)),
            Duration::ZERO
        );
    }

    #[test]
    fn to_source_should_scale_a_span_by_the_speed() {
        assert_eq!(
            to_source(Duration::from_millis(500), 2.0),
            Duration::from_secs(1),
            "half a second on screen is a second of source at speed 2.0"
        );
        assert_eq!(
            to_source(Duration::from_millis(500), 0.5),
            Duration::from_millis(250)
        );
        assert_eq!(
            to_source(Duration::from_millis(500), 1.0),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn to_source_should_degrade_rather_than_panic_on_an_unvalidated_speed() {
        // `Clip::with_speed` takes any `f64` and the builder never checks it, so these
        // reach the conversion. `Duration::mul_f64` would panic on the first two.
        let span = Duration::from_millis(500);
        assert_eq!(to_source(span, f64::INFINITY), span);
        assert_eq!(to_source(span, f64::NAN), span.mul_f64(0.01));
        assert_eq!(
            to_source(span, -3.0),
            span.mul_f64(0.01),
            "a negative speed is floored, not applied"
        );
    }

    #[test]
    fn composited_secs_should_scale_a_clip_span_by_its_speed() {
        // The rule the track's stream accumulator uses for the `xfade` offset. At speed
        // 2.0 a one-second trim occupies half a second of the composited stream, so
        // counting the source span would put the next transition twice as far along.
        assert!((composited_secs(1.0, 2.0) - 0.5).abs() < 1e-9);
        assert!((composited_secs(1.0, 0.5) - 2.0).abs() < 1e-9);
        assert!((composited_secs(1.0, 1.0) - 1.0).abs() < 1e-9);
        assert!(
            composited_secs(1.0, 0.0).is_finite(),
            "a zero speed is floored rather than dividing by zero"
        );
    }

    #[test]
    fn effective_duration_should_be_zero_without_a_transition() {
        // Generated sources so nothing is probed: this pins the early return, not the
        // handle rule.
        let outgoing = Clip::solid(ff_format::Color::rgb(0, 0, 0));
        let incoming = Clip::solid(ff_format::Color::rgb(255, 255, 255));
        assert_eq!(effective_duration(&outgoing, &incoming), Duration::ZERO);
    }

    #[test]
    fn effective_duration_should_be_unclamped_for_a_generated_outgoing_clip() {
        let outgoing = Clip::solid(ff_format::Color::rgb(0, 0, 0))
            .trim(Duration::ZERO, Duration::from_secs(1));
        let incoming = Clip::solid(ff_format::Color::rgb(255, 255, 255))
            .with_transition(XfadeTransition::Fade, Duration::from_millis(500));
        assert_eq!(
            effective_duration(&outgoing, &incoming),
            Duration::from_millis(500),
            "a color source generates frames past its out-point, so the handle is unbounded"
        );
    }

    #[test]
    fn effective_duration_should_be_zero_when_the_outgoing_clip_has_no_out_point() {
        // No out-point means the clip already plays to end-of-file: there is no handle
        // left behind it, whatever the source is.
        let outgoing = Clip::new("a.mp4");
        let incoming =
            Clip::new("b.mp4").with_transition(XfadeTransition::Fade, Duration::from_millis(500));
        assert_eq!(effective_duration(&outgoing, &incoming), Duration::ZERO);
    }
}
