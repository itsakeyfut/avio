---
status: proposed
date: 2026-08-21
decision-makers: itsakeyfut
---

# Give ff-sys a curated RAII safe layer over the raw bindings

## Context and Problem Statement

`ff-sys` is the FFI base for the `ff-*` family. Its `lib.rs` includes the bindgen
output under `#![allow(warnings)]` and adds hand-written "safe-wrapper" modules
(`avcodec`, `avformat`, `swscale`, `swresample`). In practice those wrappers only
convert FFmpeg return codes to `Result<_, c_int>` and null returns to
`Option<*const T>`; they are still `pub unsafe fn`, still take and return raw
pointers (`alloc_context3 -> *mut AVCodecContext` paired with a manual
`free_context(*mut *mut AVCodecContext)`), and use `NonNull` nowhere. So every
consumer (`ff-decode` / `ff-encode` / …) still juggles raw pointers, still owns
the manual free on every path, and the wrapper code itself is unlinted because
`#![allow(warnings)]` blankets the whole crate.

This record decides the design direction of that wrapper layer before the next
round of `ff-*` hardening grows it further. It is `proposed`: nothing depends on
it yet, and it will be implemented in a separate PR.

## Decision Drivers

* ff-sys controls both the binding and its only consumer (the family), so it can
  be opinionated where a general-purpose `-sys` crate cannot. The strength to
  pursue is correctness-by-construction over the narrow surface the family uses,
  not API breadth.
* Manual free (`free_context`, the `SwrContext` free) across `?` / early-return /
  panic paths is the classic leak / double-free site; the safety win is to make it
  structurally impossible.
* Raw `*mut` / `*const` in public signatures is the main readability cost at call
  sites; the family should speak owned values and references, not C pointers.
* `unsafe` should sit where the actual unsafety is (dereference, lifetime), not
  blanket every wrapper: calling `avcodec_find_decoder` is safe; using its result
  is not.
* Additions stay demand-driven (the primitive-scope litmus in
  `docs/rules/design.md`: do not gold-plate the primitives).

## Considered Options

* **Curated two-layer design**: a `raw` boundary (bindgen, `allow`-all, `unsafe`)
  and a `safe` module of RAII owned newtypes over `NonNull`, returning a typed
  error, with `unsafe` localized and the module fully linted. Scoped to what the
  family uses.
* **Status quo**: keep the thin `unsafe fn` free-function wrappers that return raw
  pointers and require manual free.
* **Adopt an existing safe wrapper** (`rsmpeg`) and delete the hand-written layer.
* **General-purpose full RAII wrapper**: grow the `safe` layer to broad FFmpeg
  coverage like `rsmpeg`.

## Decision Outcome

Chosen option: the **curated two-layer design**. The bindgen output moves behind a
`raw` boundary that keeps `#![allow(warnings)]`; the hand-written layer becomes a
`safe` module of owned newtypes (`CodecContext`, `SwrContext`, and the frame /
packet owners) each wrapping `NonNull<T>` and freeing its resource in `Drop`, so
manual `free_*` disappears from the API and a leak or double-free cannot survive an
early return. Public signatures carry owned values and references, never raw
pointers; fallible calls return a typed error (a newtype over `c_int` whose
`Display` uses the existing `av_error_string`, with the `EAGAIN` / `EOF` drain
states named). `unsafe` is placed at the real unsafety, and the `safe` module
compiles under `#![deny(unsafe_op_in_unsafe_fn)]` with each block carrying a
`// SAFETY:` note and full clippy, unlike the raw layer.

The scope stays what the family needs (decode / encode / format / resample), not
general coverage. This is the foundation on which call-order typestate
(`Decoder<Unopened> -> Decoder<Ready>`) can later be added; that step is out of
scope here and would be its own record.

### Confirmation

`proposed`, so nothing enforces it yet. Once implemented, these guard it:

* Each owned newtype has a `Drop` test (a drop counter, or a Miri run over
  alloc → drop) proving the resource is freed exactly once; a manual-free API no
  longer exists to misuse.
* The `safe` module compiles under `#![deny(unsafe_op_in_unsafe_fn)]` and the
  workspace clippy gate; a raw `*mut` / `*const` or an un-annotated `unsafe` in its
  public surface fails review.
* A grep / CI guard that `pub` signatures in the `safe` module name no raw pointer
  type.

Until the PR lands, this record is the only statement of the direction; the status
is honest, not enforced.

### Consequences

* Good, because manual free leaves the API and resource safety stops depending on
  caller discipline on every path.
* Good, because the family speaks owned values, so consumer `unsafe` and
  raw-pointer juggling shrink sharply and call sites read plainly.
* Good, because the hand-written layer finally gets lint coverage, separated from
  the necessarily-unlinted bindgen output.
* Bad, because it is a real refactor of the wrapper surface and its consumers, done
  in one hardening PR rather than incrementally.
* Bad, because two layers add indirection for calls that a raw function already
  served.
* What would reverse this: if the curated layer drifts toward general coverage it
  should instead adopt `rsmpeg` (a new record superseding this one); if the family
  ever needed near-complete FFmpeg breadth, the build-vs-adopt trade flips.

## Pros and Cons of the Options

### Curated two-layer RAII (chosen)

* Good, because RAII + `NonNull` + typed errors remove the manual-free and
  raw-pointer footguns over exactly the surface the family uses.
* Good, because it isolates unlinted bindgen from fully-linted hand-written code,
  and it is the base for later typestate.
* Bad, because it is a breaking refactor of the wrapper API and adds a layer.

### Status quo (thin unsafe pointer wrappers)

* Good, because it is the least code and already works.
* Bad, because manual free and raw pointers leak into every consumer, the wrappers
  are unlinted, and nothing prevents order or ownership errors. This is the defect
  this record addresses.

### Adopt rsmpeg

* Good, because it is a mature, maintained safe wrapper, with no wrapper code to
  own.
* Bad, because it is general-purpose and unopinionated, pulls in its own `-sys` and
  build assumptions, and gives up the version / ABI and build-detection control
  that is ff-sys's reason to exist. It removes the surface where a curated crate can
  be *safer*, not merely as safe.

### General-purpose full RAII wrapper

* Good, because it would cover more of FFmpeg for future needs.
* Bad, because it is scope creep against the litmus (gold-plating the primitive), a
  large maintenance surface, and duplicates what `rsmpeg` already does without the
  curation advantage.

## More Information

* Current code: `crates/ff-sys/src/lib.rs` (`#![allow(warnings)]`, the wrapper
  `pub mod`s), `avcodec.rs` (`alloc_context3` / `free_context`, `send_packet` /
  `receive_frame` as `unsafe fn` over raw pointers), `swresample/context.rs`
  (`alloc` / `free`). `NonNull` is used nowhere in `src/`.
* Rules this rests on: `docs/rules/unsafe.md` (`*_inner.rs` isolation, `// SAFETY:`),
  `docs/rules/design.md` (primitive scope / litmus).
* Call order this would let types enforce later: the per-crate
  `docs/crates/*/design.md` FFmpeg call-order sections, where deviating from the
  specified order is a bug.
* Related: the `ff-*` hardening track (v0.17.0). Follow-on, out of scope here:
  call-order typestate.
