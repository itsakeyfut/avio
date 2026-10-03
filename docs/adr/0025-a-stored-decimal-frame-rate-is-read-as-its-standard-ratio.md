---
status: "accepted"
date: 2026-10-03
decision-makers: itsakeyfut
---

# A stored decimal frame rate is read as the standard ratio it names

## Context and Problem Statement

`Timeline` stored its frame rate as an `f64` until #1947. That cannot represent the rates this engine
is for: 29.97 fps is `30000/1001`, and no `f64` spelled `29.97` is that value, so the model could not
say which frame a position fell on and nothing built on it could be frame-exact. #1947 makes the rate a
`Rational`.

Projects written before that change carry a decimal, and `frame_rate` is part of the serialised model
(`crates/avio/tests/fixtures/project/v1.json` holds `"frame_rate": 30.0`). The migration step from
format version 1 therefore has to turn a decimal into a ratio, and **that conversion is not
determined**: `29.97` could mean `30000/1001`, the ratio the number is an abbreviation of, or
`2997/100`, the number itself. An integer rate converts the same way under either reading, so the
question only has teeth for the broadcast rates, which are exactly the rates the change exists for.

Nothing in a version 1 document says which was meant, and both readings produce a valid project. The
decision is which one a reader assumes.

## Decision Drivers

* A migration must not silently change what a project means. The user is not asked and does not see it
  happen.
* Frame-exact addressing is what #1947 enables, so whichever grid a project lands on becomes visible
  to its owner shortly afterwards, in a way it was not before.
* The reading has to be stated somewhere a future reader will find it, because the alternative is
  defensible and would otherwise look like a bug.

## Considered Options

* **Read a decimal as the standard ratio it names**, within a tolerance, and read anything else
  literally as a reduced ratio.
* **Read every decimal literally**, so `29.97` becomes `2997/100`.
* **Refuse to migrate** a version 1 document and require the owner to restate the rate.

## Decision Outcome

Chosen: **read a decimal as the standard ratio it names.** `29.97` becomes `30000/1001`, `23.976`
becomes `24000/1001`, `59.94` becomes `60000/1001`, `47.952` becomes `48000/1001` and `119.88` becomes
`120000/1001`; an integer becomes `n/1`; anything else is read literally and reduced with a bounded
denominator.

The reason is that in this domain `29.97` is not a number, it is the name of a rate. A project that
stored it was cutting NTSC material, because that is the only thing the value occurs on. Reading it
literally would pin that project to a grid 0.1% away from its footage, and over a ten-hour timeline
that is more than a frame of divergence between the two rates: measured in
`the_decimal_and_the_ratio_should_disagree_on_a_long_timeline`. The project would still render, and the
error would surface later as clips that no longer sit where they were cut, which is the worst shape for
a data migration to fail in.

Refusing was rejected because the format exists so that an update does not cost the user their
projects (ADR-0024). A chain whose first real step declines to read what shipped would defeat its own
purpose.

### Consequences

* Good, because a project authored against NTSC keeps the grid it was authored against, and the
  conversion is the one its owner would have chosen.
* Good, because a rate that names no standard is still carried, reduced rather than refused, so the
  step has no shape it cannot read.
* Bad, because a project that genuinely meant `2997/100` is changed. Nothing can distinguish it from
  one that meant NTSC, and it would have been an unusual thing to author deliberately.
* Bad, because the tolerance is a threshold, and a rate that sits between two standards within it
  would be mapped to the nearer one. The standards are far apart relative to the tolerance, so this is
  a theoretical rather than a reachable concern.
* Neutral, because the ratios are hardcoded. A new standard rate means a new table entry, which is a
  one-line change in the step that owns the reading.

### Confirmation

`a_version_one_decimal_broadcast_rate_should_load_as_its_standard_ratio`
(`crates/avio/tests/project_format.rs`) loads the committed `v1-ntsc.json` fixture, whose `frame_rate`
is `29.97`, and asserts the model comes back holding `30000/1001`. Reading the decimal literally fails
it.

`rate_to_rational_should_map_a_decimal_broadcast_rate_to_its_standard_rational`,
`rate_to_rational_should_keep_an_integer_rate_exact` and
`rate_to_rational_should_reduce_an_unrecognised_rate` (`crates/avio/src/project.rs`) hold the three
branches of the reading separately, so narrowing any one of them fails a test rather than quietly
changing the others.

## More Information

The measurement that made the ratio necessary at all is in
`crates/avio/tests/frame_rate_round_trip.rs`: a frame survives the trip to a `Duration` and back at
every rate, provided the reverse conversion rounds. Flooring it loses frames at every rate, 25 and 50
included, because converting a position into a `Duration` is itself inexact. That is why the rate being
a ratio buys knowing *which* rate it is rather than arithmetic the decimal gets wrong, which
`Timeline::frame_rate`'s documentation also records.

The split that produced #1947 is in #1827's design comment; #1914 depends on the same exactness in the
tempo domain.
