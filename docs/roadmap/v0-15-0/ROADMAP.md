# v0.15.0 — FFmpeg Token Canonicalization (Color Types & Compositing)

**Goal**: Make avio's public enums map exactly to the canonical filter-argument tokens FFmpeg accepts, closing the gap between human-readable `name()` labels and the strings emitted into filter graphs. Two enum families are redesigned: the colour metadata types (`ColorSpace` / `ColorPrimaries` / `ColorTransfer`) become FFmpeg-canonical, and the conflated `BlendMode` enum is split along its two orthogonal axes — colour blend modes (`BlendMode` = `blend all_mode`) and Porter-Duff alpha-compositing operators (a new `CompositeOp` type).

**Prerequisite**: v0.14.0 complete.

**Crates in scope**: `ff-format`, `ff-filter`, `ff-pipeline`, `avio`

---

## Requirements

### FfmpegToken — canonical token vs human-readable name

- Public enums emitted into FFmpeg filter arguments expose a canonical token via an `FfmpegToken` trait, kept distinct from the human-readable `name()`.
- The `format` / `setparams` / `blend` / `overlay` filter-argument builders use `ffmpeg_token()`, never `name()`, so every generated graph string is one FFmpeg actually accepts.
- A token audit records, per enum, the FFmpeg source of truth (pinned C such as `vf_blend.c`, the `AVColorSpace` / `AVColorPrimaries` / `AVColorTransferCharacteristic` tables) so future variants can be verified against it.

### Colour metadata types — FFmpeg-canonical

- `ColorSpace`, `ColorPrimaries`, and `ColorTransfer` cover the curated set of FFmpeg-canonical variants used in real footage, each with a 1:1 `FfmpegToken`.
- Variant names follow FFmpeg's canonical naming: a conflated `Bt601` is split into `Bt470bg` / `Smpte170m`; `Bt2020` into `Bt2020Ncl` / `Bt2020Cl`; transfer uses `arib-std-b67` (HLG) and `smpte2084` (PQ) tokens.
- A `setparams` filter step consumes `ColorPrimaries` / `ColorTransfer` (and colorspace / range) so colour metadata can be tagged on a stream; the `format` filter no longer emits options it does not support.

### BlendMode — colour blend only, exact `blend all_mode`

- `BlendMode` maps 1:1 onto FFmpeg's `blend` `all_mode` token set (full canonical coverage) with an all-`Some` `FfmpegToken` (no unmapped variants).
- Photographic-only: the Porter-Duff operators and the unimplemented HSL modes are removed from `BlendMode`.

### CompositeOp — Porter-Duff alpha compositing

- A new `CompositeOp` enum (`Over`, `Under`, `In`, `Out`, `Atop`, `Xor`) expresses alpha-coverage compositing, which has no `all_mode` token and is built via `overlay` / expression-based `blend`.
- `Over` is the default and preserves existing behaviour (the previous `Normal` / `PorterDuffOver` redundancy is resolved).
- A `Composite` filter step plus `Clip::composite_op` / `VideoLayer::composite_op` route the operator through the `Timeline` → `MultiTrackComposer` compositing path, orthogonal to the colour `blend_mode`.
- `CompositeOp` and `BlendMode` are re-exported from `avio`.

---

## Design Decisions

| Topic | Decision |
|---|---|
| name vs token | `name()` stays human-readable; a separate `FfmpegToken::ffmpeg_token()` provides the canonical FFmpeg string used in filter args |
| Colour enum scope | Curated FFmpeg-canonical variants (not the full pixfmt table); names follow FFmpeg's canonical spelling |
| Colour tagging | `setparams` carries primaries / transfer / colorspace / range; `format` is restricted to options it actually supports |
| Blend vs composite | Two orthogonal axes: `BlendMode` = colour (`blend all_mode`), `CompositeOp` = alpha coverage (`overlay` / `all_expr`) |
| Default compositing | `CompositeOp::Over` is the default and is byte-identical to the prior overlay path |
| Breaking change | Accepted pre-1.0; existing graphs for retained modes are unchanged |

---

## Definition of Done

- Every `format` / `setparams` / `blend` / `overlay` argument string is built from `ffmpeg_token()` and accepted by a probe-gated real-FFmpeg test
- `ColorSpace` / `ColorPrimaries` / `ColorTransfer` expose canonical `FfmpegToken` values verified against pinned FFmpeg C source
- `BlendMode` covers the full `blend all_mode` set with an all-`Some` `FfmpegToken`; Porter-Duff and HSL removed
- `CompositeOp` (`Over` default) routes through `Clip` / `Timeline` and is re-exported from `avio`; the default path stays byte-identical
- `cargo clippy --workspace -- -D warnings` clean
- `cargo test --workspace` passes
