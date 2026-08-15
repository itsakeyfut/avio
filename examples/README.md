# avio-examples

Self-verifying, end-to-end scripts that exercise avio's **video-editing** surface
through the public `avio` facade only.

## Why this exists

Gaps in avio were found by building
[avio-editor-demo](https://github.com/itsakeyfut/avio-editor-demo) (egui), but when a
problem occurred it was hard to tell whether the bug was in the demo, in egui, or in avio.
avio-editor-demo is a thin wrapper over avio, so the same editing features can be verified
against **avio alone**.

This crate depends on `avio` the way a downstream consumer would (a dependency line plus
explicit features), so each script uses only the public API. When a script fails, the
problem is isolated to avio: no egui, no demo. `publish = false`.

This is distinct from `crates/avio/examples/`, which are the shipped, docs.rs teaching
examples. The scripts here are a dev harness: add a feature, add a script, run it, read
`PASS`/`FAIL`.

## Running

Each script generates a small synthetic clip by default, so it runs with no setup:

```bash
cargo run -p avio-examples --bin single_clip_import
cargo run -p avio-examples --bin single_clip_export
```

Point a script at a real media file, and keep the temp files for inspection:

```bash
cargo run -p avio-examples --bin single_clip_export -- --input path/to/clip.mp4 --keep
```

Flags (shared by all scripts):

| Flag | Meaning |
|---|---|
| `--input <file>` / `-i <file>` | Run against a real media file instead of a synthetic clip |
| `--keep` | Do not delete generated temp files on exit |

A script prints one line per check and exits non-zero if any check fails.

## Capability index

| Editing feature | Script | Status |
|---|---|---|
| Import a clip (probe + decode) | `single_clip_import` | done |
| Export a clip (Timeline render) | `single_clip_export` | done |
| Trim | `single_clip_trim` | TODO |
| Multi-track composition / PiP + blend | `multitrack_compose` | TODO |
| Transitions (`XfadeTransition`) | `transitions` | TODO |
| Keyframe animation (`AnimationTrack`) | `keyframe_animation` | TODO |
| Per-clip effects (`FilterStep`) | `effect_<name>` | TODO |
| Preview matches export | `preview_matches_export` | TODO |
| Audio mix + per-clip volume/fades | `audio_mix` | TODO |
| Ken Burns pan & zoom | `ken_burns` | TODO (after #1295) |

## Adding a script

1. Add `src/bin/<name>.rs` with `fn main() -> avio_examples::BoxResult<()>`.
2. Use `avio_examples::{parse_args, resolve_input}` to obtain an input clip and
   `avio_examples::Report` to record `check(label, ok)` assertions.
3. Assemble the feature via the `avio::` public API and assert the expected result.
4. Add a row to the capability index above.
