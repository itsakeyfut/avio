# Design and API Conventions

> How avio's architecture is enforced in code. The rationale and design of record live in
> [`../specs/engine-and-primitives.md`](../specs/engine-and-primitives.md). This document is the
> "how to keep the code aligned" rulebook.
> Related: [rust.md](./rust.md), [error-handling.md](./error-handling.md),
> [unsafe.md](./unsafe.md).

---

## 1. Engine vs primitive (the core boundary)

avio is an **editing engine**; the `ff-*` family are **model-agnostic primitives**.

- **`avio` (engine)** owns the editing model: `Timeline` / `Clip`, the model-to-scene derivation,
  and edit history. It answers "**WHAT** to edit."
- **`ff-*` (primitives)** are stateless executors of the current frame's work: decode, filter,
  composite one frame, interpolate, encode. They answer "**HOW** to execute" and know nothing
  about time, tracks, clips, edits, or history.
- **Litmus**: does this type/function need to know TIME / TRACK / CLIP / EDIT / HISTORY to do its
  job? Yes -> `avio` (model); No -> `ff-*` (primitive).
- Full detail and rationale: [engine-and-primitives.md](../specs/engine-and-primitives.md).

---

## 2. Type placement

- Define a type in the **lowest applicable crate** and re-export it upward. Never define the same
  type in two crates. When adding a public type, also add it to `avio/src/lib.rs`.
- **One deliberate exception**: the editing model (`Timeline` / `Clip` / edit history) is defined
  in `avio` (the top of the graph), so that `ff-*` remain model-free by dependency direction, not
  by discipline.

---

## 3. Encapsulation and `#[non_exhaustive]`

- Public enums/structs in `ff-format` that may gain variants or fields over time are
  `#[non_exhaustive]` (e.g. `PixelFormat`, `ColorSpace`, `ColorRange`, `SampleFormat`). This keeps
  additions non-breaking and steers callers toward constructors and a wildcard match arm.
- When matching a `#[non_exhaustive]` enum **from another crate**, a `_` arm is **required** by the
  compiler — it is not redundant. Do not flag `Self::Unknown | _ => …` as dead code.

---

## 4. Builder pattern

- Use a consuming builder when a type takes **3 or more optional parameters**. Required parameters
  go on `new()` / `create()`.
- Validation is collected in `.build()`, which returns `Result`. Setters do not validate.
- Codecs, encoders, decoders, filter graphs, and pipelines all follow this.

---

## 5. FFmpeg `name()` vs `ffmpeg_token()`

- A type's `name()` is a human-readable label; `FfmpegToken::ffmpeg_token()` is the canonical
  FFmpeg token. They can differ (e.g. `ColorSpace::Bt2020` has `name()` = `"bt2020"` but the token
  is `"bt2020nc"`), and the token may be `None` when FFmpeg has no equivalent.
- Build filter-argument strings from `ffmpeg_token()` and **skip `None`**. Never build them from
  `name()` — that produces arguments FFmpeg rejects.
- Verify token sets/values against the pinned FFmpeg C source, not the HTML docs
  (see [`../specs/ffmpeg-tokens.md`](../specs/ffmpeg-tokens.md)).

---

## 6. FFmpeg call order is part of the design

The crate design docs (`docs/crates/{name}/design.md`) specify the exact FFmpeg call order.
Deviating without a deliberate, documented reason is a bug, not a style choice.

---

## 7. Crate boundaries

| Crate | Holds | Depends on |
|---|---|---|
| `ff-sys` | Raw bindgen FFI + safe thin wrappers | (lowest) |
| `ff-common` | Shared memory abstractions (no FFmpeg dep) | ff-sys |
| `ff-format` | Shared pure-Rust type system (no FFmpeg dep) | ff-common |
| `ff-probe` | Read-only metadata extraction | ff-format |
| `ff-decode` | Decode pipelines (video / audio / image) | ff-format |
| `ff-analysis` | Media analysis (scene / silence / BPM / histogram / keyframe / black-frame detection, video scopes) | ff-decode |
| `ff-encode` | Encode pipelines | ff-format |
| `ff-remux` | Stream-copy remux (trim / audio replace / extract / add), no re-encoding | ff-format |
| `ff-filter` | libavfilter graph construction + primitive compositor | ff-format |
| `ff-pipeline` | Decode -> filter -> encode execution pipelines | ff-filter |
| `ff-stream` | HLS / DASH adaptive streaming output | ff-pipeline |
| `ff-preview` | Real-time preview and proxy primitives | ff-pipeline |
| `ff-render` | GPU compositing (wgpu) | ff-format |
| `avio` | **Engine**: editing model + derivation + history; re-exports the primitives | all `ff-*` |

Dependency direction (no cycles):

```
ff-sys -> ff-common -> ff-format -> ff-probe / ff-decode / ff-encode / ff-remux -> ff-filter
       -> ff-pipeline -> ff-stream / ff-preview / ff-render -> avio (engine, top)

ff-decode -> ff-analysis   (analysis reads decoded frames; sits above ff-decode)
```

- Dependencies point **downward** only. No cycles.
- `ff-format` and `ff-common` carry **no FFmpeg dependency** and **no `unsafe`** (pure Rust).
- Because the editing model sits at the top (`avio`), nothing in `ff-*` can depend on it: the
  primitives are model-free by construction.
