# Architecture Decision Records

A design decision's rationale lives here and nowhere else. `docs/specs/**` and the
per-crate `docs/specs/crates/*/design.md` state the *outcome* and link to the record;
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
| [0005](./0005-per-frame-compositor-scale-rotation.md) | Animate scale/rotation per frame via self-animating compositor steps emitted by `derive`, neutralizing the static layer transform | accepted | derive unit tests in `crates/avio/src/derive.rs` (animated scale/rotation emit `ScaleAnimated`/`RotateAnimated` + neutralize; static stays on the layer) and the probe-gated `compositor_should_evaluate_per_frame_scale_and_rotation` |
| [0006](./0006-typed-re-editable-clip-effects.md) | Represent per-clip effects as a typed, id-addressed, keyframable model (`ClipEffect`/`EffectKind`/`Param`/`EffectId`) compiled to `FilterStep`, folding the flat color fields into `EffectKind::ColorCorrect` | accepted | unit tests in `crates/avio/src/effect.rs` (derive/neutral-skip), `crates/avio/src/edit.rs` (five commands + effect-id stamping), `crates/avio/src/editor.rs` (no id reuse across undo), and `crates/avio/tests/serde_persistence.rs` (round-trip) |
| [0007](./0007-gpu-compositing-bridge.md) | Drive `ff-render`'s GPU compositor from `avio` for preview and export, GPU by default with automatic CPU fallback; mapping in a `gpu`-gated `avio` module, whole-frame fallback, export = composite -> readback -> existing encoder | accepted | (Br1 is docs-only) compile-time no-cycle guard (`avio -> ff-render`, no reverse dep), plus the Br2 mapping tests (#1625) and the Br5 fallback/parity tests (#1628) that its later slices land |
| [0008](./0008-ffx-primitives-facade.md) | Introduce `ffx`, a feature-gated facade over the whole `ff-*` primitive family (Bevy-style), and make `avio` depend only on it; the two-repository split is deferred | accepted | a dependency guard that `avio`'s `Cargo.toml` names no `ff-*` crate directly (only `ffx`), `ffx` re-export tests, and a `default`-features build that excludes `ff-render`/wgpu (to land with the Phase-1 migration) |
| [0009](./0009-transition-placement-semantics.md) | A transition preserves the timeline length and is fed by the outgoing clip's handle; one `transition::effective_duration` rule serves every derivation | accepted | `crates/avio/tests/transition_placement.rs` (hard-cut length preserved, no black gap on a middle clip, chained transitions at their own boundaries, A/V alignment), `crates/avio/tests/preview_transition_reach.rs` (a derived scene reaches the runner's blend), and the `clamp_to_handle` unit tests in `crates/avio/src/transition.rs` |
| [0010](./0010-gpu-blend-modes-follow-ffmpeg.md) | GPU blend modes reproduce `FFmpeg`'s `vf_blend` (the `DEPTH == 32` branch, `A`=base/`B`=overlay per ff-filter's pad wiring), correcting `ColorDodge`/`ColorBurn`/`SoftLight` away from Photoshop; `And`/`Or`/`Xor` take the 8-bit definition | accepted | the formula-pinning table and singularity tests in `crates/ff-render/src/nodes/composite/blend_math.rs`, the adapter-gated `blend_gpu_should_match_the_cpu_path_for_every_mode` (`crates/ff-render/tests/gpu_nodes.rs`), and `map_scene_should_map_every_blend_mode` (`crates/avio/src/gpu.rs`) |
| [0011](./0011-explicit-bitstream-filters-only.md) | Bitstream filters are exposed for explicit use only (`ff-sys::BsfContext`, `ff-remux`'s `video_bsf`/`audio_bsf`); libavformat keeps selecting the container-required ones, because its muxer `check_bitstream` callbacks already do so on every write path | accepted | `mp4_to_mpegts_stream_copy_should_produce_annex_b_h264` and `trim_with_an_explicit_video_bsf_should_change_the_output` (`crates/ff-remux/tests/bsf_tests.rs`), and the lifecycle tests in `crates/ff-sys/src/bsf.rs` |
| [0012](./0012-chain-description-escape-hatch.md) | The filter-string escape hatch takes a whole libavfilter *description* (`avfilter_graph_parse2`), because `raw_filter` already covers the single-filter case; it must be one-in/one-out, and unlike every other step it is validated at `build()` | accepted | `parse_desc_should_reject_an_unknown_filter_at_build_time`, `parse_desc_should_accept_a_branching_description`, `parse_desc_should_reject_a_description_that_is_not_one_in_one_out` and `parse_desc_should_build_a_working_graph_from_a_chain_description` (`crates/ff-filter/tests/parse_desc_tests.rs`) |
| [0013](./0013-zero-copy-gpu-to-encoder.md) | Defer the zero-copy GPU-to-encoder handoff; export keeps composite -> readback -> encoder | accepted | nothing fails by construction (the record forbids no code); the shape is held by `GpuCompositor::composite` returning a CPU buffer, which any zero-copy path must change visibly |

**By status** - accepted: 0001, 0002, 0003, 0004, 0005, 0006, 0007, 0008, 0009, 0010, 0011, 0012, 0013 · proposed: none · superseded: none

Records are numbered consecutively from `0001`.

## Where each kind of writing belongs

| Location | Holds | Does not hold |
|---|---|---|
| `docs/specs/**` | what the design is (architecture of record) | why it was chosen; links here instead |
| `docs/specs/crates/*/design.md` | per-crate design and the FFmpeg call order | why a cross-cutting decision was made |
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
