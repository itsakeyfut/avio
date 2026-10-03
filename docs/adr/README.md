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
| [0014](./0014-reject-porter-duff-on-the-filter-path.md) | The filter path refuses `In`/`Out`/`Atop`/`Xor` at `build()` until it can carry alpha, and the engine refuses such a timeline up front when no GPU compositor is attached, rather than computing per-channel arithmetic under the operator's name | accepted | `composite_expression_operators_should_be_rejected_at_build` and the nested-chain case (`crates/ff-filter/tests/push_pull_tests.rs`), `composite_op_in_layer_should_be_rejected_at_build` (`composition_tests.rs`), `overlay_composite_expr_ops_should_be_rejected_at_build` (`realtime_composer.rs`), and the export/preview gate tests in `crates/avio/tests/composite_op_gate.rs`; each goes red when its gate is removed |
| [0013](./0013-zero-copy-gpu-to-encoder.md) | Defer the zero-copy GPU-to-encoder handoff; export keeps composite -> readback -> encoder | accepted | nothing fails by construction (the record forbids no code); the shape is held by `GpuCompositor::composite` returning a CPU buffer, which any zero-copy path must change visibly |
| [0015](./0015-unpaced-runner-for-e2e-tests.md) | The preview runner offers `Pacing::Unpaced`, a clock the loop itself moves one frame period per presented frame, and every e2e test that drives `SceneRunner` uses it unless real-time pacing is its subject | accepted | `unpaced_runner_should_deliver_every_frame_through_a_stall` and `real_time_runner_should_drop_the_frames_a_stall_makes_late` (`crates/ff-preview/tests/pacing_test.rs`), the seam test's deterministic window (`gpu_transition_seam_test.rs`), and the frame-count parity in `crates/avio/tests/preview_export_parity.rs` |
| [0016](./0016-base-layer-placed-in-canvas-space.md) | Every layer, the base included, is placed in canvas space on every route (top-left at `(x, y)`, native size at `scale == 1`, `canvas * scale` otherwise, no implicit fit), the construction the CPU export composer always used; the realtime composer and the GPU compositor adopt it | accepted | the four-route lit-box checks in `crates/avio/tests/base_transform_tests.rs`, `a_positioned_single_layer_gpu_should_match_cpu_within_tolerance` and the native-size / clipped / overhang parity tests (`gpu_parity_tests.rs`), `a_positioned_base_clip_should_export_the_same_on_both_routes` (`gpu_export_tests.rs`), and the base placement tests in `realtime_composer.rs` |
| [0017](./0017-clamp-unrepresentable-timestamps.md) | A timestamp `Duration` cannot represent (negative, non-finite or overflowing) clamps to zero and is logged, rather than failing the decode; the guard is one `Duration::try_from_secs_f64` in `Timestamp::as_duration`, which both decoders call instead of computing the position themselves | accepted | the four `as_duration_should_clamp_*` unit tests (`crates/ff-format/src/time/timestamp.rs`) and `decoding_a_negative_first_pts_should_report_a_position_instead_of_aborting` (`crates/ff-decode/tests/negative_pts_tests.rs`) |
| [0018](./0018-exact-seek-holds-its-frame.md) | An `Exact` seek holds back the frame it landed on, instead of consuming it; `Keyframe`/`Backward` stay approximate | accepted | the four exact-seek tests in `crates/ff-decode/tests/video_seeking_tests.rs` (land-on-target, stale-after-second-seek, flush-discards, and the tightened `test_seek_exact_mode`), their three audio counterparts in `audio_decoder_tests.rs`, and `trims_should_start_on_the_same_source_frame_on_both_routes` with its round-trip control (`crates/avio/tests/trim_frame_parity.rs`) |
| [0019](./0019-grouped-edits-propagate-as-a-timeline-delta.md) | A grouped edit propagates as a timeline-time delta, clamped per member; a razor cuts every member that spans the cut and the right halves form one new group | accepted | the eleven group tests in `crates/avio/src/edit.rs` (trim propagation, delta-not-value, per-member speed, clamp, unset out-point, ripple per track, razor/skip/regroup, the move-after-split tear, and the ungrouped razor) and `every_propagating_command_should_reach_the_linked_member` (`crates/avio/tests/group_propagation.rs`) |
| [0020](./0020-text-clips-require-the-drawtext-filter.md) | Text clips stay on `drawtext`; the documented install names the freetype requirement, and the capability is queryable and mirrored in `Timeline::validate` | accepted | `text_rendering_available_should_agree_with_building_a_text_source` and `text_filter_should_name_the_filter_the_text_layer_builds` (`crates/ff-filter/src/capability.rs`), the two `validate` tests in `crates/avio/src/validate.rs`, and `rendering_a_text_clip_should_be_refused_by_name_when_the_build_cannot_draw_it` (`crates/avio/tests/text_renderer_gate.rs`) |
| [0021](./0021-a-locked-track-refuses-edits.md) | A locked track refuses the edits that would change it, the derivation still ignores the flag, and `Command::SetTrackLock` is the exempt way out | accepted | the ten lock tests in `crates/avio/src/edit.rs` (per-command refusals, the grouped cases, the batch, the release and the timeline-level commands), `is_active_should_ignore_the_lock_flag` (`crates/avio/src/track.rs`) and `a_locked_track_should_render_the_same_as_an_unlocked_one` (`crates/avio/tests/locked_track_render.rs`) |
| [0022](./0022-the-loudness-range-target-is-verified-not-achieved.md) | `LoudnessNormalize`'s `lra` is the range a programme is required to fit within, which the engine verifies and reports on rather than achieves; `loudnorm` was not adopted because `ebur128` publishes no gating threshold, so its linear mode is unreachable | accepted | `wide_dynamic_range_should_be_reported_against_a_narrow_target` and `a_range_within_the_target_should_not_be_reported` (`crates/ff-filter/tests/loudness_range_tests.rs`), `lra_outcome_should_treat_an_exactly_equal_range_as_within` (`crates/ff-filter/src/filter_inner/normalize.rs`) and `different_true_peak_ceilings_should_produce_different_and_respected_peaks` (`crates/ff-filter/tests/loudness_ceiling_tests.rs`) |
| [0023](./0023-where-an-authoring-rule-is-enforced.md) | An authoring rule is enforced where it can afford to be: `apply` is pure, `TimelineBuilder::build` may read files, `Timeline::validate` only reports; a rule with an I/O-free part and an I/O-dependent part is split, not deferred | accepted | `a_locked_track_should_be_refused_before_the_kind_rule`, the three `*_should_refuse_a_generated_source_*` / `*_cannot_serve_*` tests and `move_clip_to_track_should_accept_a_file_source_on_an_audio_track` (`crates/avio/src/edit.rs`), `build_should_refuse_a_generated_clip_on_an_audio_track` and `build_should_accept_a_file_clip_on_an_audio_track` (`crates/avio/src/timeline.rs`), `validate_should_report_a_clip_that_cannot_serve_its_track` (`crates/avio/src/validate.rs`) |
| [0024](./0024-the-project-format-is-the-derive-output-in-a-versioned-envelope.md) | The project format is the serde derive output, carried in a `{ format_version, timeline }` envelope and migrated by a chain of `Value` transforms; `Project` owns saving so the envelope cannot be bypassed | accepted | `every_committed_fixture_should_load_to_its_expected_model` and `a_document_from_a_newer_version_should_be_refused` (`crates/avio/tests/project_format.rs`), plus `migrate_should_apply_a_step_that_changes_the_document`, `migrate_should_apply_steps_in_ascending_order`, `migrate_should_refuse_a_gap_in_the_chain` and `the_version_zero_step_should_wrap_a_bare_model_in_the_envelope` (`crates/avio/src/project.rs`) |

| [0025](./0025-a-stored-decimal-frame-rate-is-read-as-its-standard-ratio.md) | A stored decimal frame rate is read as the standard ratio it names (29.97 to `30000/1001`), an integer as `n/1`, and anything else literally and reduced; a decimal in this domain names a rate rather than being one | accepted | `a_version_one_decimal_broadcast_rate_should_load_as_its_standard_ratio` (`crates/avio/tests/project_format.rs`), plus `rate_to_rational_should_map_a_decimal_broadcast_rate_to_its_standard_rational`, `rate_to_rational_should_keep_an_integer_rate_exact` and `rate_to_rational_should_reduce_an_unrecognised_rate` (`crates/avio/src/project.rs`) |
**By status** - accepted: 0001, 0002, 0003, 0004, 0005, 0006, 0007, 0008, 0009, 0010, 0011, 0012, 0013, 0014, 0015, 0016, 0017, 0018, 0019, 0020, 0021, 0022, 0023, 0024, 0025 · proposed: none · superseded: none

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
