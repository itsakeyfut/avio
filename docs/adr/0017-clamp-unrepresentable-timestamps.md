---
status: "accepted"
date: 2026-09-25
decision-makers: itsakeyfut
---

# A timestamp `Duration` cannot represent clamps to zero and is logged, rather than failing the decode

## Context and Problem Statement

`ff-decode` converted a frame's presentation timestamp with an unguarded
`Duration::from_secs_f64`, so a negative value aborted the process. A library a
GUI or a server embeds must not abort its host, and every other failure in that
crate returns `Err`. Deciding between an error and a clamp settles how the whole
workspace treats a timestamp outside `Duration`'s range, because the conversion
is shared: `Timestamp::as_duration` has production callers in `avio`,
`ff-analysis` and `ff-decode`.

## Decision Drivers

* A negative first timestamp is **ordinary media, not corruption**: Opus states
  its pre-skip that way and AAC its priming. Measured on avio's own output, the
  first audio packet is at `-7` for Opus in Matroska and WebM and at `-1024` for
  AAC in MP4.
* `Duration` rejects three distinct inputs, not one: negative, non-finite and
  overflowing. `Rational::as_f64` returns `INFINITY` or `NaN` for a zero
  denominator, which a sign test does not catch.
* `position` is a progress indicator a caller reads between frames, not sample
  data. Losing sub-millisecond accuracy at the start of a stream changes no
  decoded output.
* The workspace had already chosen a clamp at one of the three sites, so the
  options were consistency in one direction or the other.

## Considered Options

* Clamp to zero and log a warning
* Return a typed `DecodeError` variant
* Change `position` to a signed timestamp type

## Decision Outcome

Chosen option: **clamp to zero and log a warning**, because an error would make
ordinary Opus and AAC media undecodable, which is worse for a valid file than the
panic it replaces: the panic at least only fires on the containers that carry the
delay negatively. Clamping also matches what `Timestamp::as_duration` already
did for the negative case, so the decision is now applied uniformly instead of at
one of three sites.

The guard is a single `Duration::try_from_secs_f64`, which rejects all three
unrepresentable inputs, rather than a sign test per site.

### Confirmation

`as_duration_should_clamp_a_negative_codec_delay_to_zero`,
`as_duration_should_clamp_an_infinite_time_base_to_zero`,
`as_duration_should_clamp_a_nan_time_base_to_zero` and
`as_duration_should_clamp_seconds_beyond_duration_to_zero`
(`crates/ff-format/src/time/timestamp.rs`) fail if the conversion stops being
total, and
`decoding_a_negative_first_pts_should_report_a_position_instead_of_aborting`
(`crates/ff-decode/tests/negative_pts_tests.rs`) fails if either decoder computes
the position itself again.

Each boundary was measured rather than assumed. Removing the guard entirely makes
all four unit tests fail; reverting it to the earlier `secs < 0.0` sign test makes
the three covering a non-finite or overflowing value fail, and the negative one
still pass, which is how that shape of guard hid the gap; reverting either decoder
to its own arithmetic makes the integration test fail with the panic from the
issue.

### Consequences

* Good, because a host can decode any input without the library aborting it.
* Good, because the clamp is silent for a negative value and logs only for a
  non-finite or overflowing one, whose line names `pts` and the time base. The
  conversion runs once per frame, and `docs/rules/logging.md` forbids logging
  there; a negative first timestamp is ordinary media by the reasoning above, so
  there is nothing to report, while an unusable time base is worth one line even
  at that rate.
* Good, because the arithmetic exists once. A future call site inherits the guard
  by using the helper rather than by remembering to add a check.
* Bad, because a stream that genuinely starts before zero reports its first
  frames at zero, so a caller cannot reconstruct the pre-roll from `position`.
  A caller that needs the raw value reads `Timestamp::pts` from the frame, which
  is unchanged.
* What would reverse this: a use that needs negative positions preserved end to
  end, such as honouring Opus pre-skip when trimming. That wants a signed
  timestamp on the public surface, which is a breaking change and its own record.

## Pros and Cons of the Options

### Clamp to zero and log a warning

* Good, because ordinary media keeps decoding.
* Good, because it matches the behaviour already shipped in `as_duration`.
* Bad, because the first frames of a delayed stream share a position.

### Return a typed `DecodeError` variant

* Good, because nothing is silently altered and the host is told precisely.
* Bad, because every Opus file in Matroska or WebM would fail to decode, which
  is a regression against a valid file rather than a fix.

### Change `position` to a signed timestamp type

* Good, because no information is lost at any point.
* Bad, because it is a breaking change reaching `avio` and `ff-preview` for a
  quantity no current caller needs signed.

## More Information

* #1819 - the panic, its reproduction and the acceptance criteria.
* `crates/ff-format/src/time/timestamp.rs` - the shared conversion.
* `crates/ff-format/src/time/rational.rs` - `as_f64`, the source of the
  non-finite values.
* The audit recorded in #1819: `ff-probe` and `ff-analysis` already guarded every
  equivalent conversion, so only the two `ff-decode` sites and the non-finite
  gap in `as_duration` were unguarded.
