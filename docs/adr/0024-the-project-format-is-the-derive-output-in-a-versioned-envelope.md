---
status: "accepted"
date: 2026-10-03
decision-makers: itsakeyfut
---

# The project format is the serde derive output, carried in a versioned envelope and migrated per version

## Context and Problem Statement

`Timeline` has derived `Serialize`/`Deserialize` under the `serde` feature since #1452, so a caller could
always persist the model by reaching for `serde_json`. Nothing in that output says which version of the
model wrote it, and there was no save or load API, so no project format had ever been promised (#1905).

v0.19.0 changes the model in eight issues that each alter serialised state (#1878, #1895, #1874, #1828,
#1898, #1886, #1904, #1829). Without a version and a migration path, the first user who updates loses
their projects, which is the worst failure an editing application has: the data is gone rather than
wrong. The format also decides how the rest of the milestone can be built, because with migration in
place a model change is a migration step and without it every model change is an invisible compatibility
break.

## Decision Drivers

* A project that cannot be reopened after an update is worse than any defect this project has fixed.
* The cost of every future model change in this milestone is set by this choice.
* This repository treats a second copy of something that can disagree with the first as a defect class,
  not a matter of taste: #1909 was filed for documents that contradicted the tree, and
  `cargo xtask adr-check` exists to catch that drift in the records themselves.
* The model has not changed since 0.18.4, so whatever is chosen starts from a clean position.

## Considered Options

* The derive output as the format, versioned in an envelope, with a migration chain
* An explicit wire schema the model converts to and from
* A version field inside `Timeline` rather than around it

## Decision Outcome

Chosen option: **the derive output, in an envelope, migrated per version**.

The on-disk shape is `{ "format_version": 1, "timeline": { ...the model... } }`. The envelope's shape is
frozen; the model inside it is not. Loading reads the document as a `serde_json::Value`, takes the
version from it, runs the migration chain over the `Value`, and only then deserialises the model, so a
step can run before the current types have to accept the document.

`Project` owns saving and loading. That matters more than it looks: a version in an envelope is advisory
for as long as `Timeline: Serialize` is public, because `serde_json::to_string(&timeline)` produces a
document without one. The envelope and an API that writes it land together or neither is worth having.

Migration is a chain of steps, each a `fn(Value) -> Result<Value>` keyed by the version it reads. A
`Value` transform means a rename needs no frozen copy of the old types; a step that has to understand
the model can still deserialise the part it needs. The chain refuses a version newer than the reader, and
refuses a gap rather than skipping it.

A document with no `format_version` is version 0. Its step wraps the bare model in the envelope, which is
the whole conversion, because the model's shape did not change between 0.18.4 and this release.

### Confirmation

`every_committed_fixture_should_load_to_its_expected_model` and
`a_document_from_a_newer_version_should_be_refused` (`crates/avio/tests/project_format.rs`) fail if the
version stops being read before the model, or if a newer document is read anyway. In
`crates/avio/src/project.rs`, `migrate_should_apply_a_step_that_changes_the_document` fails if the chain
stops converting, `migrate_should_apply_steps_in_ascending_order` fails if it runs steps out of order,
and `migrate_should_refuse_a_gap_in_the_chain` fails if a missing step is skipped instead of refused.
`the_version_zero_step_should_wrap_a_bare_model_in_the_envelope` fails if an unversioned document stops
being understood.

### Consequences

* Good, because no conversion code exists to fall out of step with the model. The 80 types reachable
  from `Timeline` (53 in `avio`, 9 in `ff-format`, 18 in `ff-filter`) are serialised by the derive that
  already existed.
* Good, because the cost of a format change is paid per breaking change rather than per release: an
  additive field needs `#[serde(default)]` and no step at all.
* Good, because the chain is a pure function over `Value` with its target and its steps as parameters,
  so its behaviour is tested with steps that convert something rather than with the one-step chain that
  ships today.
* Bad, because the Rust shape **is** the format. A field rename is a format change, and a contributor who
  renames one without adding a step breaks every saved project. Nothing in the compiler catches that; the
  fixtures are what catch it.
* Bad, because the fixtures accumulate: one document per released version, forever, and an older one may
  never be edited to match a later shape without destroying the evidence it exists to provide.
* What would reverse this: a model restructure so large that writing its step costs more than the
  explicit schema would have. That is a judgement to make with the step in front of you, and reversing
  means writing the schema once and converting from the last derive-shaped version into it.

## Pros and Cons of the Options

### The derive output in a versioned envelope

* Good, because it adds no parallel description of the model.
* Good, because version 0 is readable for the price of one wrapping step.
* Bad, because the format's stability depends on reviewer attention to renames rather than on a type.

### An explicit wire schema

* Good, because the model could then be restructured freely, with the schema absorbing the change.
* Good, because the format would be documented by a type rather than by what the derive happens to emit.
* Bad, because it is hand-written conversion for 80 types, one of which (`FilterStep`) carries 99
  variants, maintained in step with the model forever. A single predicate in this crate had already been
  copied four times and diverged before it was unified (#1932); an 80-type parallel description is that
  failure mode with a maintenance budget.

### A version field inside `Timeline`

* Good, because there would be no envelope to bypass.
* Bad, because the outer shape has to stay readable forever, and a field inside the model is subject to
  every change the model makes. The version has to be readable before the model is deserialised, which a
  field inside it cannot guarantee.

## More Information

* #1905, and its design comment, which records the measurements this rests on.
* #1452 introduced the derives this builds on.
* ADR-0023 fixed where a rule about a document is enforced; this record fixes how the document is
  written down.
* `crates/avio/tests/fixtures/project/README.md` carries the rule for adding a fixture.
