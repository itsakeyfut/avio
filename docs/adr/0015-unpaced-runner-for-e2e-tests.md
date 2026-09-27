---
status: "accepted"
date: 2026-09-07
decision-makers: itsakeyfut
---

# The preview runner offers an unpaced mode, and e2e tests drive it that way

## Context and Problem Statement

`ff_preview::SceneRunner::run` paces frame delivery against a wall clock: it sleeps until a frame
is due and drops any frame more than one period late. That is right for playback. It is wrong for a
test, because a loaded or virtualised runner is late by a frame period as a matter of course, and a
test that puts a lower bound on delivered frames then fails by luck of scheduling.

Four issues were this one hazard: #1723 (the parity test's span bound), #1737 (the transition never
armed), #1757 (the transition seam test dropping its whole 400ms window on `macos-latest`), #1780
(the parity test again, after a stall longer than the timeline). Each fix widened a margin: a
half-span bound, a wider fixture, an elapsed-time arm. The flake kept coming back because the cause,
the wall clock, was still in the loop.

## Decision Drivers

* An e2e test must be deterministic on the slowest CI runner, not merely likely to pass.
* The assertions must keep their meaning: `blends > 0` still has to fail when the runner stops
  routing transitions through the seam, and a frame count still has to fail when a frame is lost.
* Playback must not change: the wall-clock behaviour is the product.
* The runner is the only place that knows when a frame is presented, so it is the only place that
  can define "not late".

## Considered Options

* **Widen the fixtures and bounds** until the flake is rare.
* **Retry or serialise** the affected tests in CI.
* **Give the runner a clock the loop itself moves**, and switch the e2e tests to it.

## Decision Outcome

Chosen option: **a runner-driven clock**, because it removes the cause rather than the symptom, and
it makes the previously weakened assertions strict again.

Concretely:

* `MasterClock::Stepped { pts }` reads back exactly the position the runner last set. The runner
  sets it to `presented_pts + frame_period` after every delivered frame and advances it one period
  per synthesised gap frame, so a frame is never early or late.
* `ff_preview::Pacing { RealTime, Unpaced }` and `SceneRunner::set_pacing` select the clock before
  `run`. Under `Unpaced` the pacing sleep and the late-frame drop are both skipped; gap detection
  and gap filling are unchanged. Reverse playback and the pause poll keep waiting on the wall
  clock; neither is a test subject.
* Every e2e test that drives `SceneRunner` sets `Pacing::Unpaced` unless real-time pacing is what
  it tests. Three keep the wall clock: `av_sync_test` (drift between wall time and PTS is its
  subject) and the two ignored control-timing tests in `timeline_preview_tests`
  (pause-drift regression, seek-completed latency).
* `docs/rules/test.md` states the rule.

### Confirmation

* `crates/ff-preview/tests/pacing_test.rs`: with a sink that stalls for five frame periods on its
  first frame, `unpaced_runner_should_deliver_every_frame_through_a_stall` asserts a complete,
  evenly spaced PTS sequence, and `real_time_runner_should_not_deliver_every_frame_through_a_stall`
  asserts the same stall costs frames under the default pacing. Measured by knocking the mechanism
  out: making `Pacing::Unpaced` install the wall clock turns the first red, and so does removing
  the post-present clock step. Removing the `stepped` guard on the drop arm does **not**: under the
  stepped clock every consecutive frame reads `diff == 0`, so that arm is unreachable and its guard
  is defensive, which is stated here rather than claimed as coverage.
  `unpaced_runner_should_fill_a_pre_roll_gap_without_duplicates` covers the one place the clock
  is moved by something other than a presented frame: removing the gap loop's `advance` makes it
  repeat the first slot and turns the test red.
* `preview_runner_should_use_the_injected_blend_for_a_transition`
  (`crates/ff-preview/tests/gpu_transition_seam_test.rs`) now reaches its window on exactly four
  frames; short-circuiting `try_gpu_blend` to `None` still turns it red, which is #1737's coverage.
* `preview_export_parity` (`crates/avio/tests/preview_export_parity.rs`) asserts the preview
  delivered the same number of frames the export encoded, which the elapsed-time arm of #1780 could
  not.

### Consequences

* Good, because the e2e suite no longer depends on the runner's scheduling, and the assertions
  weakened by #1723 and #1780 are strict again.
* Good, because the mode is a product feature too: a thumbnail strip or an exhaustive check wants
  every frame, not real-time delivery.
* Bad, because tests driven unpaced no longer exercise the drop path; `pacing_test` and
  `av_sync_test` are what keep it covered.
* Neutral: this removes the wall clock as a cause of macOS flakes, not every cause. Measuring the
  repeated runs exposed a second one, a hardware decoder failing mid-stream and ending the run as
  if at EOF (#1789); the e2e tests skip on the resulting `PlayerEvent::Error` until that is fixed.
* What would reverse this: a runner that no longer owns pacing (for instance one driven by a
  caller-supplied clock), at which point `Pacing` becomes the caller's concern.

## Pros and Cons of the Options

### Widen the fixtures and bounds

* Good, because each change is one line.
* Bad, because it has already been done three times and the failure recurred each time; the drop
  is unbounded, so no margin is a guarantee.

### Retry or serialise in CI

* Good, because it touches no code.
* Bad, because a retried flake still hides a real regression behind the same message, and
  serialising does not make a virtualised runner keep real time.

### Runner-driven clock (chosen)

* Good, because determinism comes from the loop's own bookkeeping, not from the machine.
* Bad, because it adds a clock variant and one setting to the public runner API.

## More Information

* Issues #1757 (this decision), #1723, #1737, #1780 (the earlier symptoms).
* Code: `crates/ff-preview/src/playback/master_clock.rs` (`Stepped`, `advance`, `is_stepped`),
  `crates/ff-preview/src/scene/runner.rs` (`Pacing`, `set_pacing`, the `stepped` guards in `run`).
* Rule: `docs/rules/test.md`, "Integration test policy", the `SceneRunner` bullet.
