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
Scene { fps, canvas: Option<(u32,u32)>, video_layers: Vec<SceneVideoLayer>, audio_tracks: Vec<SceneAudioTrack> }
SceneVideoLayer { placements: Vec<ScenePlacement> }        // layer 0 = V1 base, 1.. = overlays
ScenePlacement  { source: PathBuf, timeline_start/timeline_end: Duration, in_point: Duration,
                  out_point: Option<Duration>, speed: f64, transition_dur: Duration (V1 only),
                  opacity: f32, layer: RealtimeLayerSpec }
SceneAudioTrack { placements: Vec<SceneAudioPlacement> }
SceneAudioPlacement { source, timeline_start/end, in_point, clip_dur, fade_in, fade_out, speed,
                      volume_db: f64, volume_track: Option<AnimationTrack<f64>>, has_audio: bool }
```

- `RealtimeLayerSpec` = `RealtimeLayer` minus `width`/`height`/`pixel_format` (a new `ff-filter`
  primitive); the runner builds the per-frame layer at decode time via
  `RealtimeLayer::with_dimensions(spec, w, h, fmt)`. `Clip::realtime_layer` splits into
  `Clip → RealtimeLayerSpec` (avio) + `spec → RealtimeLayer` (ff-filter).
- `PlayerCommand::UpdateLayout(Box<Timeline>)` → `UpdateLayout(Box<Scene>)`;
  `PlayerHandle::update_timeline` → `update_scene`. `avio` re-derives a `Scene` on edit and the runner
  reconciles it (as `update_layout_in_place` does today).
- After the move, `ff-preview`'s only `ff_pipeline` use is `proxy/` (primitive `Pipeline`/`EncoderConfig`)
  — it drops the model dependency entirely.

**Slices for #1329:** **A** (ff-filter) `RealtimeLayerSpec` + `with_dimensions`; **B** (ff-preview)
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
