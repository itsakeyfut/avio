# avio — Concepts

> avio is a safe, high-level Rust toolkit over FFmpeg for building media applications, structured as
> a **video-editing engine** on top of a family of **model-agnostic media primitives**.
> Design specs live in [`specs/`](./specs/); coding rules in [`rules/`](./rules/README.md).

---

## 1. What avio is

avio is two things in one workspace:

- **`avio` — the engine.** It owns an opinionated, modern editing model (`Timeline`, `Clip`, tracks,
  per-clip effect stacks, keyframes) and derives every rendered frame from that model. If you want a
  ready-made editing model, you build on `avio`.
- **`ff-*` — the primitives.** `ff-sys` / `-common` / `-format` / `-probe` / `-decode` / `-encode` /
  `-filter` / `-pipeline` / `-stream` / `-preview` / `-render` are safe, **model-agnostic** building
  blocks (decode, encode, filter graphs, a compositor, streaming, GPU). They know nothing about
  timelines or edits. If you want to design your **own** editing model, you build on `ff-*` and avio
  imposes nothing on you.

The safety guarantee runs through both: every unsafe FFmpeg call is encapsulated, so application
code never needs `unsafe`.

---

## 2. Background and purpose

- avio began as a safe FFmpeg wrapper. Gaps were found by building
  [avio-editor-demo](https://github.com/itsakeyfut/avio-editor-demo) (an egui editor) on top of it,
  but isolating "is this a demo, egui, or avio bug?" was hard. That motivated verifying editing
  features against avio alone.
- The realization: the reusable, hard, valuable part of a video editor is the **engine** (the edit
  model, the render graph, preview/export consistency, frame accuracy, undo), not the UI. avio
  commits to being that engine.
- But editing apps differ in how they model an edit (AviUtl's object timeline, Premiere's tracks,
  Final Cut's magnetic timeline). Forcing one model on everyone would be a general-purpose trap. So
  avio splits the concern: the **engine** (`avio`) has one modern model, and the **primitives**
  (`ff-*`) let anyone build their own.

---

## 3. Vision

- **avio is the engine; `ff-*` is the substrate.** Product identity and design investment go into
  the engine. The primitives stay clean and reusable.
- **Two audiences, no conflict.** "I want a ready editing model" -> use `avio`. "I want to build my
  own model" -> use `ff-*`. The clean separation lets avio be opinionated without locking anyone
  out: the primitives are the escape hatch.
- **Toward a stable 1.0**: a production-grade engine, validated by real applications built on it.

---

## 4. Concepts and design philosophy

- **Engine vs primitive boundary.** The model (`avio`) answers "**WHAT** to edit"; the primitives
  (`ff-*`) answer "**HOW** to execute this frame". A primitive never needs to know about time,
  tracks, clips, edits, or history. (See [rules/design.md](./rules/design.md) and
  [specs/engine-and-primitives.md](./specs/engine-and-primitives.md).)
- **Immutable, declarative model -> pure derivation.** The edit state is an immutable value; the
  render scene at time `t` is a pure function of it. This makes **preview == export structural**
  (both paths consume the same derivation) and undo trivial (a history of versions).
- **Safe by construction.** `unsafe` is isolated in `*_inner.rs`; the public API is entirely safe.
- **Batteries included, but not everything.** avio covers the editing and delivery range (decode,
  encode, filter, compose, preview, stream, GPU, analysis), not every FFmpeg feature.
- **Borrow what is solid, build what differentiates.** FFmpeg for codecs and filters, wgpu for GPU.
  avio builds the editing model, the derivation, preview/export consistency, and the engine's
  correctness.

---

## 5. Positioning

- avio sits **between low-level bindings (`ffmpeg-next`) and full frameworks (GStreamer)**: a safe,
  batteries-included editing engine with reusable primitives underneath.
- The shape to aim for is "the MLT / GStreamer Editing Services of the Rust world, but with a modern
  immutable-model core and preview == export by construction."
- The primitives also serve plain media plumbing (safe Rust decode / encode / transcode / stream), a
  real secondary use beyond editing.

---

## 6. What avio does not do

- The **engine** does not try to model every editing paradigm. It commits to a track + per-clip
  effect stack + keyframe + compositing model. Node-based (Nuke-style) and magnetic-timeline
  (Final Cut-style) models are out of scope for the engine; apps wanting those build on `ff-*`
  directly.
- avio does not aim to wrap every FFmpeg feature.
- The primitives (`ff-*`) carry no editing opinions. Keeping them model-free is enforced by the
  dependency direction (the editing model sits at the top, in `avio`), not by discipline alone.
