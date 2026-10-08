---
status: "accepted"
date: 2026-10-08
decision-makers: itsakeyfut
---

# A musical position is a rational beat count, and its bound is refused rather than wrapped

## Context and Problem Statement

The editing model expressed a position only as a `Duration`. Music-driven editing is authored against a
tempo grid, and for an edit where one source is chopped into hundreds of short samples laid on that
grid, computing the seconds outside the engine is the whole job (#1914).

A musical position has to be stored somehow, and the two shapes in common use disagree about what they
are exact for:

* a **tick index** at a fixed resolution, as MIDI files use, and
* a **rational count of beats**.

The choice is not a matter of taste. The material this engine is for mixes triplets and dotted values
freely, and a tick grid is exact only for subdivisions that divide its resolution. The decision also
shapes the model's public surface, because the type appears in a command and in two accessors.

## Decision Drivers

* Triplets and dotted values have to survive exactly. The issue states it as a requirement, and a
  subdivision that is approximated accumulates error along the timeline rather than cancelling.
* The representation's limits have to be reachable only loudly. A musical position that silently
  becomes wrong places a clip somewhere nobody asked for, and the person authoring the edit finds out
  by watching it.
* The model already holds a frame rate as a `Rational` (ADR-0025, #1947), and adding a second numeric
  convention for time would be a second thing to keep consistent.

## Considered Options

* **A rational beat count in `ff_format::Rational`, with checked arithmetic that refuses its bound.**
* **A tick index at a fixed resolution** (960 or 480 per beat, as sequencers use).
* **A rational count backed by `i64`**, as a new type.

## Decision Outcome

Chosen: **a rational beat count, with arithmetic that refuses what it cannot represent.**

A fraction is exact for every subdivision, which is the requirement. The bound, and the way the bound
is reached, are what the rest of this record is about, because they are the reason the arithmetic is
checked rather than operator-shaped.

**The bound is generous for real material and not unbounded.** Accumulating 5948 subdivisions drawn
from halves, thirds, quarters, fifths, sixths, sevenths, eighths, twelfths and sixteenths across 1740
beats (ten minutes at 174 BPM) reached a reduced numerator of 2,920,397 and a denominator of 1680,
which is 0.14% of `i32`. For the subdivisions a piece actually mixes, whose denominators have lowest
common multiple 48, the representable range is about 44.7 million beats, roughly 4285 hours at 174 BPM.
But a position mixing *every* divisor from 1 to 16 has denominator 720720, and `i32` runs out at 2979
beats, about 17 minutes, which is inside the length of this material.

**And the failure at the bound would be silent and signed.** `Rational`'s `Add` finishes with
`(num / g) as i32`, and a Rust `i64 as i32` wraps: 3,000,000,000 becomes -1,294,967,296. A beat
position that overflowed through the operator would be negative, placing a clip before the start of
the timeline with nothing reported. That is why `Beats` implements `checked_add` and `checked_sub` and
deliberately does **not** implement `Add` and `Sub`: an operator has nowhere to put the refusal, and
offering one would make the dangerous path the convenient one.

A tick index was rejected because it cannot hold a septuplet at any resolution that holds the binary
subdivisions: 960 ticks to the beat gives 137.14 for a seventh. The requirement is explicit about
triplets and dotted values surviving, and the material that wants septuplets is the same material.

A separate `i64`-backed rational was rejected because it would be the same concept twice, which this
workspace's layering rules forbid, and because `Rational`'s own sharp edges are being addressed on
their own terms (#1949) rather than worked around with a parallel type.

### Consequences

* Good, because every subdivision the material uses is exact, and a sum of mixed subdivisions is too.
* Good, because the bound is documented in beats and in minutes at a named tempo, so a caller can tell
  whether it is anywhere near one.
* Good, because the one numeric convention for time in the model stays `Rational`.
* Bad, because arithmetic is `checked_*` rather than `+`, which is less pleasant to write. That is the
  price of the refusal being unavoidable, and it is the right way round.
* Bad, because a pathological subdivision set is refused at a length a real piece could reach. The
  alternative was for it to be wrong instead.
* Neutral, because the reverse conversion takes the subdivision as a parameter rather than being
  parameterless. That follows from `Duration`'s nanosecond quantisation rather than from this decision:
  beat 1 at 174 BPM is 344827586 ns, and converting that back exactly gives `4999999997/5000000000`,
  not 1. Only the caller knows the grid it is authoring against, exactly as only the caller knows a
  source's frame rate in `Clip::in_point_frame`.

### Confirmation

`beats_checked_add_should_refuse_a_sum_i32_cannot_hold`
(`crates/ff-format/src/time/beats.rs`) adds two positions whose common denominator is about 4.6e18 and
asserts `None`. Replacing the checked arithmetic with `Rational`'s operators makes it return a wrapped
negative instead, and the test fails.

`beats_should_add_every_subdivision_exactly` in the same module holds the other half: triplets summing
to exactly one beat, a dotted value as `3/4`, and a mixed sum reduced to `41/42`, asserted on the
components rather than on the value because `Rational`'s equality cross-multiplies and would not notice
a denominator growing without bound.

`beat_at_should_round_trip_a_position_on_its_own_grid` (`crates/ff-format/src/time/tempo.rs`) covers
subdivisions 1, 2, 3, 4, 7, 16 and 960 across four tempi including `375/4` and `30000/1001`. A tick
grid would fail its septuplet rows, which is the comparison this decision rests on.

## More Information

The drift argument #1914 inherited from #1827 does not hold, and the case for musical time is
expression rather than round-trip error: measuring found the lossy step is `Duration`'s nanosecond
quantisation rather than the rate, so flooring a reverse conversion is wrong at 25 and 50 fps as much
as at 29.97, and rounding is exact at every rate. `crates/avio/tests/frame_rate_round_trip.rs` holds
that.

ADR-0025 records the frame rate's move to a `Rational` and the reading of a stored decimal, which this
decision follows rather than repeats.
