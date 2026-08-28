# Architecture Decision Records

A design decision's rationale lives here and nowhere else. `docs/specs/**` and the
per-crate `docs/crates/*/design.md` state the *outcome* and link to the record;
they do not repeat the reasoning.

These records are written in **English** (like `docs/rules/`), because they
constrain implementation and are read by both contributors and tooling.

Format: [MADR 4.0](https://adr.github.io/madr/), the de facto Markdown ADR
standard. Copy [`adr-template.md`](./adr-template.md) to start one.

## Index

| # | Decision | Status | Confirmed by |
|---|---|---|---|
| [0001](./0001-clip-and-track-identity.md) | Address clips and tracks by a document-scoped, monotonic `u64` id | accepted | unit tests in `crates/avio/src/edit.rs` (id set/unique, stability, not-found) |
| [0002](./0002-per-clip-animation-in-the-model.md) | Carry all per-clip animation in the model; a primitive may static-evaluate what it cannot yet animate | accepted | derive unit tests in `crates/avio/src/derive.rs` (scale/rotation, pitch tracks flow; export == preview) |
| [0003](./0003-ff-sys-safe-wrapper-layer.md) | Give ff-sys a curated RAII safe layer (owned `NonNull` newtypes, typed errors, localized `unsafe`) over the raw bindings | accepted | per-owned-type drop-once tests, `#![deny(unsafe_op_in_unsafe_fn)]` + CI clippy, and the no-raw-pointer guard `crates/ff-sys/tests/seal.rs` |
| [0004](./0004-avio-engine-not-facade.md) | `avio` exposes only the editing engine and its model-facing types; drop the primitive-facade re-exports | accepted | the primitive-facade re-exports removed from `crates/avio/src/lib.rs` (#1482-#1484), the `lib.rs` accessibility tests assert the kept engine surface, and `avio-examples`/docs build on it (a dedicated regression guard #1485 was declined as premature for a pre-1.0 crate) |

**By status** - accepted: 0001, 0002, 0003, 0004 · proposed: none · superseded: none

Records are numbered consecutively from `0001`.

## Where each kind of writing belongs

| Location | Holds | Does not hold |
|---|---|---|
| `docs/specs/**` | what the design is (architecture of record) | why it was chosen; links here instead |
| `docs/crates/*/design.md` | per-crate design and the FFmpeg call order | why a cross-cutting decision was made |
| `docs/adr/**` | why a decision was chosen, when, and what would reverse it | type or signature detail; links to the specs |
| `docs/rules/**` | what to do while implementing | how a decision was reached |
| `docs/roadmap/**` | what to build next (capabilities) | how a decision was reached |

## When to write one

* Two or more implementations are possible and one is chosen, especially when the
  choice is cross-crate or shapes the editing model.
* An existing decision is reversed: write a new record, mark the old one
  `superseded by ADR-NNNN`, and note what changed.
* You are about to write "undecided" into a spec: open one as `proposed`.

**Not worth an ADR:** naming, formatting, or anything affecting a single call site.

## Conventions

* Filename `NNNN-short-slug.md`, numbers consecutive.
* MADR statuses: `proposed`, `accepted`, `rejected`, `deprecated`,
  `superseded by ADR-NNNN`.
* Every record fills in **Confirmation**: which test or guard fails if the
  decision is violated. If nothing would fail, say so. A decision that looks
  enforced and is not is worse than one that is honestly unenforced.
* A `proposed` status while the codebase already relies on the decision is itself
  a defect; say so in *Context and Problem Statement*.
* Keep the status in sync between an ADR's front matter and its row in this index.

## More Information

* [MADR 4.0](https://adr.github.io/madr/) - the template this follows.
* [`adr-template.md`](./adr-template.md) - copy this to start a new record.
