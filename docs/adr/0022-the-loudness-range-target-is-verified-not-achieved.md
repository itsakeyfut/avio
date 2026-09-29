---
status: "accepted"
date: 2026-09-29
decision-makers: itsakeyfut
---

# The loudness range target is verified, not achieved

## Context and Problem Statement

`FilterStep::LoudnessNormalize` takes three parameters, and `lra` was validated, stored and never
read: `run_loudness_normalization` used `target_lufs` and `true_peak_db` and dropped the rest with
`..` (#1854). A delivery was prepared believing a loudness-range constraint had been applied. The
parameter cannot simply stay as it was, and making it real means deciding what "real" is: a single
gain moves a programme's level, never its range.

## Decision Drivers

* A parameter that is accepted and silently dropped is the one outcome that must not survive.
* #1821 (the target is reached on the master bus) and #1822 (the ceiling bounds the gain) were just
  established by measurement on the existing `ebur128` + `volume` construction. Replacing that
  construction would invalidate both rather than extend them.
* What FFmpeg does with the same parameter, read from the pinned source rather than the manual.

## Considered Options

* Compare the requested range against the measurement pass 1 already takes, and report
* Replace the step with FFmpeg's `loudnorm` filter, whose dynamic mode does shape the range
* Remove `lra` from the public API

## Decision Outcome

Chosen option: "compare and report", because `LRA` in `loudnorm` is primarily a *gate* rather than a
target, and this construction is the mode that gate guards.

`af_loudnorm.c:806-812` of the pinned n8.0.1 source picks linear mode only when all four measured
values are supplied **and** `offset_tp <= target_tp` **and** `measured_lra <= target_lra`; otherwise
the frame type is dynamic, where `target_lra` shapes the gain envelope (`:559`). The `ebur128` +
`volume` construction here *is* that linear mode, and it applied the gain without ever asking the
second question. `f_ebur128.c:930` already emits `lavfi.r128.LRA` under `metadata=1`, which pass 1
already passes, so the comparison costs no new filter, option or pass.

So `lra` is now the range the programme is required to fit within, which the engine checks and
reports on: within the target, the single gain is legitimate and the agreement is logged; wider, a
warning names both values and states that the range was not changed. The gain itself is untouched.
Callers who need to know before exporting can measure with `LoudnessMeter`, whose `lra` field is
public.

### Confirmation

In `crates/ff-filter/tests/loudness_range_tests.rs`,
`wide_dynamic_range_should_be_reported_against_a_narrow_target` fails if a range wider than the
target goes unreported, and `a_range_within_the_target_should_not_be_reported` fails if agreement is
reported as a breach. `lra_outcome_should_treat_an_exactly_equal_range_as_within` in
`crates/ff-filter/src/filter_inner/normalize.rs` pins the boundary as `af_loudnorm.c:812`'s `<=`.
`crates/ff-filter/tests/loudness_ceiling_tests.rs` fails if this decision is violated in the other
direction, by letting the outcome move the gain that #1822 established.

### Consequences

* Good, because the parameter is honest for the first time, in the construction whose behaviour two
  preceding issues measured.
* Good, because the accepted range is now `loudnorm`'s own `[1.0, 50.0]` (`af_loudnorm.c:106`), so a
  target that could not be expressed downstream cannot be built.
* Bad, because a caller who wants the range *achieved* still has nothing that achieves it; they get a
  warning instead of dynamics processing.
* Bad, because the report is a log line, so a host that does not capture logs learns nothing. Making
  it a returned value would widen the filter API for one diagnostic.
* What would reverse this: a decision to do real dynamic-range processing, which means adopting
  `loudnorm`'s dynamic mode or `dynaudnorm` and announcing that the output of every existing caller
  changes.

## Pros and Cons of the Options

### Compare and report

* Good, because the measurement already exists and the construction stays as measured.
* Good, because it reproduces FFmpeg's own decision rule rather than inventing one.
* Bad, because the constraint is checked, not met.

### Replace the step with `loudnorm`

* Good, because it would make all three parameters real in one filter.
* Bad, because its honest two-pass is `loudnorm` to `loudnorm`. Linear mode needs `measured_thresh`,
  and `f_ebur128.c`'s metadata block publishes `M`, `S`, `I`, `LRA`, `LRA.low`, `LRA.high` and the
  peaks but no gating threshold; `loudnorm` prints it from its own internal `ebur128` instance.
  Feeding it what this repository can measure would leave `measured_thresh` at its default, `:811`
  would fail, and **every render would silently become dynamic mode**: different-sounding output for
  every existing caller, with #1821's and #1822's measured behaviour invalidated rather than extended.

### Remove `lra` from the public API

* Good, because nothing would be accepted and dropped.
* Bad, because the parameter is meaningful and, as above, checkable; removing it breaks
  `FilterGraphBuilder::loudness_normalize` and `FilterStep::LoudnessNormalize` to discard information
  the engine can report.

## More Information

* #1854, and its design comment, which records the measurements this rests on: a flat 10 s tone
  reports `LRA = 0.0 LU` and 5 s quiet plus 5 s loud reports `LRA = 7.8 LU` through `LoudnessMeter`.
* #1821 (the target on the master bus) and #1822 (the ceiling), whose construction this preserves.
* Pinned FFmpeg n8.0.1: `libavfilter/af_loudnorm.c` (`:106`, `:559`, `:806-812`) and
  `libavfilter/f_ebur128.c` (`:930`).
