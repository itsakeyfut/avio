# Engine and Primitives — avio's architecture

> Status: design of record. Drives tracking issues #1326 (relocation + purification) and
> #1327 (immutable model redesign). Internal doc.

## 1. Positioning decision

avio is an **editing engine**, not a general-purpose "build any editor" toolkit. The `ff-*`
family are **model-agnostic primitives**.

- **`avio` (engine)** owns the editing model: what a project/timeline/clip is, how time and
  tracks and edits and history are organised, and how a frame is derived from that model. It is
  opinionated by design (it commits to one editing model).
- **`ff-*` (primitives)** own execution: decode, encode, filter, composite one frame, interpolate,
  probe, stream. They know nothing about editing.

Why this split, honestly:

1. **Primary justification — engine correctness.** The North Star (below) can only guarantee
   "preview == export" and be testable if the primitives are stateless executors of a per-frame
   description. Purifying `ff-*` is what makes the engine correct, not a courtesy to library users.
2. **De-risks the engine's opinion.** Because clean primitives remain, an app whose editing model
   does not fit avio's can drop to `ff-*` and build its own. The separation is what makes "be an
   opinionated engine" a safe bet.
3. **Bonus — a real, modest library audience.** Safe Rust media plumbing (decode/encode/transcode/
   stream) is a genuine `ff-*` use case (e.g. ascii-term). This is a bonus, not the reason.

**Guardrail against over-engineering:** purify `ff-*` only to the depth the engine's North Star
requires. Do not gold-plate the libraries for a speculative "build your own editor" audience. The
engine's needs bound the purification scope.

## 2. North Star (target architecture)

```
immutable/declarative editing model   (avio)
        │  derive(model, t)  — a pure function
        ▼
      Scene   (a flat, model-agnostic description of one frame's work)   (ff-* type)
        │  executed by
        ▼
stateless primitives: decode / filter / composite / encode   (ff-*)
```

- The **editing model** is an immutable value. Edits produce a new version.
- **`derive(model, t) -> Scene`** is a pure function: the single source of truth for a frame.
- **preview == export by construction**: both paths call the same `derive` and the same primitive
  executor. Equality is structural, not maintained by hand.
- **Undo/Redo** is a history of immutable model versions.

The model's immutable redesign, the pure `derive`, and Do/Undo are implemented in **#1327**.
Their shape is fixed here so that #1326 purifies toward the right target.

### 2.1 The immutable model and edit API — design for #1327

The model relocated in #1326 is a build-once value (`Timeline` via `TimelineBuilder`) with no edit
or history API. #1327 makes it an immutable, editable value with Do/Undo, per the North Star above.
Confirmed design (user-approved):

- **Immutable document.** The existing `Timeline` value is reused as the immutable document (already
  `Clone`; its fields *are* the document — canvas, fps, tracks, clips, animations). An edit produces a
  new `Timeline`; the `TimelineBuilder` probe path is not re-run (edits carry the already-resolved
  canvas/fps).
- **Edit API — Command/reducer.** A `Command` enum (`AddClip`, `RemoveClip`, `MoveClip`, `TrimClip`,
  `SetClipProperty`, `AddTrack`/`RemoveTrack`, `SetCanvas`, `SetFrameRate`, …) is applied by a pure
  `apply(&Timeline, &Command) -> Result<Timeline, EditError>`. Clips/tracks are addressed by a
  `TrackId` (kind + index) plus a clip index (index-based first; stable IDs are a later enhancement).
  Making each edit a value gives first-class Do/Undo/Redo and a path to serialization / collaboration.
- **History — snapshot versions.** An `Editor { history: Vec<Timeline>, cursor }` holds the version
  history: `apply` pushes a new version (truncating the redo tail), `undo`/`redo` move the cursor.
  This is the North Star's "history of immutable versions"; structural sharing / diffs (im/rpds) are a
  later memory optimisation, not needed first.
- **Unified `derive` — staged.** Today export (`Timeline::render` → `MultiTrackComposer`) and preview
  (`Timeline::to_scene` → runner → `RealtimeComposer`) are separate derivations that share
  `Clip::video_effect_chain` but build different layer types and composers, so "preview == export" is
  not yet structural. #1327 stages this: **first** the immutable model + edit API + Do/Undo (additive,
  executors unchanged, always-green); **then** a unified pure `derive(model, t) -> Scene` (per-frame)
  with converging executors so preview == export by construction — the hardest, riskiest part, split
  into its own child issues.

**Child issues (#1327):** **C1** `Command` + pure `apply`; **C2** `Editor` (history / undo / redo);
**C3** extract a shared `derive` core across export/preview; **C4** unified `derive(model, t) -> Scene`
+ executor convergence (its own design sub-cycle); **C5** structural-sharing history (memory, if
needed). C1/C2 are the immediate, high-value, low-risk work; C3–C5 follow.

**C4 decomposition (scope B — full visual parity):** C4 is an epic. Both compositors are FFmpeg
libavfilter graphs that already share the same lower-level builders, so "same executor" means the same
per-frame overlay/blend graph. Scope B closes the nine gaps below so preview is visually identical to
export while keeping the two executors (full per-frame-export unification, retiring `MultiTrackComposer`,
is out of scope). Children: **C4a** single `derive` backbone + video representation convergence (gaps 2, 3);
**C4b** `composite_op` in the preview executor (gap 1); **C4c** transition/xfade-kind parity (gap 8, last,
own sub-cycle); **C4d** `lavfi_overlay` in preview (gap 4); **C4e** audio derivation + preview parity
(gaps 5, 6, 7). Sequence: C4a → {C4b, C4d, C4e} → C4c.

#### C4 gap-list — where preview (`to_scene`) diverges from export (`render`) today

A field-by-field mapping (during C3) showed the two derivations diverge not just in *encoding* but in
*features*: the export path (`render` → `MultiTrackComposer`/`VideoLayer`) honours nine things the preview
path (`to_scene` → `RealtimeComposer`/`RealtimeLayerDescriptor`) silently drops. Closing these — plus
converging the two target types (`VideoLayer`'s `AnimatedValue<f64>` vs `RealtimeLayerDescriptor`'s
`value + Option<track>`, and the missing composite/scale/rotation fields) — is the concrete **C4** target
(and is a *behaviour change* for preview, so it cannot land under C3's "no behaviour change"):

1. `composite_op` (Porter-Duff) — export only.
2. `scale_x` / `scale_y` / `rotation` — export only (animation-driven).
3. Timeline-level `video_animations` / `audio_animations` maps — export only, incl. the track-level
   opacity/x/y/volume/pan fallbacks and the 3-way precedence merge (per-clip track > static > track-anim).
4. `lavfi_overlay` — export only.
5. Audio `speed` — export applies to audio; the preview audio placement has no speed.
6. `audio_effects` — export only.
7. `pan` — export only.
8. Transition **kind** (`XfadeTransition`) — export honours it; preview collapses to a duration-only fade.

The one extraction genuinely shared today is `Clip::video_effect_chain()` (eq + per-clip video effects),
consumed by export directly and by preview via `Clip::realtime_layer_descriptor`. **C3** extracted the
export per-clip interpretation into the pure `avio::derive` module (`video_layer` / `audio_track`) so it
has one testable home; **C4** makes preview derive from the same core, closing the gaps above.

**C4a (done):** both paths now build their video layer from one core. `avio::derive::video_transform`
computes the merged opacity/x/y/scale/rotation + blend + composite once; `video_layer` (export) and
`realtime_descriptor` (preview) both consume it, and `to_scene` routes through the latter. The
`RealtimeLayerDescriptor` converged to the `VideoLayer` shape (`AnimatedValue<f64>` opacity/x/y +
`scale_x`/`scale_y`/`rotation` + `composite_op`), and `build_realtime_composition` consumes the new
fields (rendering scale/rotation as static-at-t=0 nodes, matching export; `composite_op` is carried but
still rendered as `Over` until C4b). This closes gaps 2 and 3 for preview overlays. **Deferred (Q2):**
exact pixel-parity (the realtime `rgba`/base-size output vs export's `color`-canvas/`yuv420p`, and the
overlay force-scale) and base-track (V1) scale/rotation — both entangled with canvas semantics — are left
to a C4 canvas-reconciliation follow-up.

## 3. Boundary principle (model vs primitive)

**Litmus:** does this type/function need to know **TIME, TRACK, CLIP, EDIT, or HISTORY** to do
its job?

- **No** (it operates on the current frame's given data) → **primitive (`ff-*`)**.
- **Yes** → **model (`avio`)**.

| Belongs in `avio` (model) | Belongs in `ff-*` (primitive) |
|---|---|
| Timeline / Clip / tracks / clip effect stacks | FilterGraph / FilterStep |
| model → Scene derivation, preview/export orchestration | RealtimeComposer / compositor (executes a Scene) |
| TimelinePlayer / TimelineRunner (consume the model) | Keyframe / Easing / Lerp / AnimationTrack (interpolation math) |
| edit history (undo) — #1327 | BlendMode / CompositeOp / XfadeTransition (enums) |
| | decode / encode / probe / stream / EncoderConfig |
| | Pipeline / VideoPipeline / … (execution pipelines) |

## 4. The `Scene` seam

`Scene` is the clean seam between engine and primitives: a flat, model-agnostic description of
**what to render for one frame** — an ordered list of layers (each: a source frame + transform +
opacity + blend + effect steps) plus the output canvas.

- It is the **output** of `derive(model, t)` and the **input** of the primitive compositor.
- It is a **primitive type** (`ff-*` side): any engine could produce a `Scene`.
- Purification means moving timeline/editorial semantics **out** of primitive inputs. For example
  today's `VideoLayer` carries `time_offset` and `in_transition` (timeline concepts); those move
  into the engine's `derive`, and the primitive layer keeps only current-frame fields.

### 4.1 The preview Scene (runner-facing) — design for #1329

The `Scene` above is per-frame (the `RealtimeComposer` input, already model-free). The **real-time
preview runner** needs a **timeline-level** Scene, because it owns decode scheduling (opening/seeking a
decoder per source and mapping the playhead to each source's frame time). A mapping of
`ff-preview`'s `TimelineRunner`/`TimelinePlayer` established:

- It reads `Timeline` **only at init and `update_layout`** — via four accessors (`video_tracks()`,
  `audio_tracks()`, `frame_rate()`, `explicit_canvas()`); `run()` never touches `Timeline`.
- The deep coupling is `Clip`: it clones the whole `Clip` into `ClipState.clip` and calls
  `clip.realtime_layer(w, h, fmt)` + `clip.{opacity,volume}_track.value_at(t)` **every tick**. Everything
  `realtime_layer` needs is already primitive (`FilterStep` / `AnimationTrack` / `AnimatedValue` /
  `BlendMode`), and `RealtimeLayer` + `RealtimeComposer` are already the compositing seam.

**Decision (approved): decouple, do not move.** The runner **stays in `ff-preview`** and consumes a
primitive `Scene` instead of `Timeline`/`Clip`; only the model and its `Timeline → Scene` derivation move
to `avio`. This avoids exposing `ff-preview` internals (`MasterClock`, `SwsRgbaConverter`,
`PlayerHandle::for_timeline`) that a physical move would have forced public.

**Scene types (defined in `ff-preview`, primitive; `avio` constructs them from `Timeline`/`Clip`):**

```
Scene { fps, canvas: Option<(u32,u32)>, video_tracks: Vec<SceneVideoTrack>, audio_tracks: Vec<SceneAudioTrack> }
SceneVideoTrack { placements: Vec<ScenePlacement> }        // track 0 = V1 base, 1.. = overlays (index = composite order)
ScenePlacement  { source: PathBuf, timeline_offset: Duration, in_point: Duration,
                  out_point: Option<Duration>, speed: f64, transition_dur: Duration (V1 only),
                  opacity: f32, layer: RealtimeLayerDescriptor,
                  fade_in/fade_out: Duration, volume_db: f64,
                  volume_track: Option<AnimationTrack<f64>> }
SceneAudioTrack { placements: Vec<SceneAudioPlacement> }
SceneAudioPlacement { source: PathBuf, timeline_offset, in_point, out_point: Option<Duration>,
                      fade_in/fade_out: Duration, volume_db: f64,
                      volume_track: Option<AnimationTrack<f64>> }
```

- **Model projection, not media resolution.** `Scene` carries only the editing model's primitivised
  fields (`timeline_offset`, `out_point`, `speed`, …). Resolving them against the media — probing for
  duration / `has_audio` / frame size — stays in the preview runner's `open()`, exactly as today. So a
  Scene re-derived on every edit needs no re-probe, and behaviour is preserved (probe timing and error
  surface unchanged).
- `RealtimeLayerDescriptor` = `RealtimeLayer` minus `width`/`height`/`pixel_format` (a new `ff-filter`
  primitive); the runner builds the per-frame layer at decode time via
  `RealtimeLayer::with_dimensions(descriptor, w, h, fmt)`. `Clip::realtime_layer` splits into
  `Clip → RealtimeLayerDescriptor` (avio) + `descriptor → RealtimeLayer` (ff-filter).
- `PlayerCommand::UpdateLayout(Box<Timeline>)` → `UpdateLayout(Box<Scene>)`;
  `PlayerHandle::update_timeline` → `update_scene`. `avio` re-derives a `Scene` on edit and the runner
  reconciles it (as `update_layout_in_place` does today).
- After the move, `ff-preview`'s only `ff_pipeline` use is `proxy/` (primitive `Pipeline`/`EncoderConfig`)
  — it drops the model dependency entirely.

**Slices for #1329:** **A** (ff-filter) `RealtimeLayerDescriptor` + `with_dimensions`; **B** (ff-preview)
`Scene` types + runner consumes `Scene` (temporary `Timeline → Scene` adapter keeps it green); **C**
(avio) move `Timeline`/`Clip`/derivation + the two `PipelineError` model variants to `avio` and place the
`Timeline → Scene` derivation there; **D** close-out (folds into #1333).

## 5. Crate roles and dependency direction (new)

```
ff-sys → ff-common → ff-format → ff-probe / ff-decode / ff-encode → ff-filter
       → ff-pipeline → ff-stream / ff-preview / ff-render → avio (engine, top)
```

- **The editing model lives at the top (`avio`).** Nothing in `ff-*` depends on it, so `ff-*` are
  structurally model-free — the separation is enforced by dependency direction, not discipline.
- `avio` stops being a facade-only crate. It defines the editing-model types (and, in #1327, the
  derivation, immutable state, and undo). It still re-exports the `ff-*` primitives.
- Versioning stays **lockstep** (Bevy-style); `ff-*` are public but not yet semver-frozen.
  Independent per-crate versioning is deferred until real external demand appears.

## 6. Project decomposition

- **#1326 (first): relocation + purification.** Move the model to `avio`; purify `ff-*` into
  stateless `Scene` executors; behaviour preserved. Closeable when `ff-*` hold no editing
  semantics. Classification-first, always-green, verified by tests + the `avio-examples` harness.
- **#1327 (after): immutable model redesign.** Rebuild the now-relocated model as an immutable
  value with pure `derive` and Do/Undo. Its own design/spec cycle.

## 7. Non-goals

- Not a framework for **every** editing paradigm. avio commits to a track + per-clip effect stack
  + keyframe + compositing model. Node-based (Nuke-style) and magnetic-timeline (FCPX-style)
  models are out of scope for the engine; such apps use `ff-*` directly.
- No gold-plating of `ff-*` beyond what the engine needs (see the guardrail in §1).

## 8. CLAUDE.md changes (applied in P1-0)

- Remove "`avio` = facade / `pub use` only / no new types".
- State: `avio` = engine crate that owns the editing model; `ff-*` = model-agnostic primitives.
- Add the boundary principle (litmus) and the purification guardrail.
- Restate the dependency direction with the editing model at the top (`avio`).
- Note the "define types in the lowest applicable crate" rule now has one deliberate exception:
  the editing model is defined in `avio` (the top), because model-freeness of `ff-*` must be
  enforced by dependency direction.
