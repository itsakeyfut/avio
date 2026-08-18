# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.16.0] - 2026-08-18

This release splits the project into an editing **engine** (`avio`) and a family of **model-free primitive libraries** (the `ff-*` crates). The editing model moves up into `avio`, the primitives are purified so they impose no editing vocabulary, crate versions are decoupled for independent per-crate versioning, and every crate is prepared for its first standalone crates.io publish. Two new primitive crates are carved out of the existing ones, and a shared error-classification contract is added across the family.

### Added

#### ff-analysis (new crate)
- Media-analysis suite (scene / silence / black-frame / keyframe detection, histogram and waveform extraction, `BpmResult`, and video scopes) extracted from `ff-decode` into its own crate ([#1391](https://github.com/itsakeyfut/avio/issues/1391))

#### ff-remux (new crate)
- Stream-copy remuxing (trim, and audio replace / extract / add) without re-encoding, extracted from `ff-encode` ([#1393](https://github.com/itsakeyfut/avio/issues/1393))

#### ff-format
- `MediaError` trait and `ErrorSeverity` classification (`Fatal` / `Recoverable` / `Other`), implemented by every `ff-*` error type so callers can branch on recoverability generically ([#1392](https://github.com/itsakeyfut/avio/issues/1392))

#### avio
- The editing model now lives in the engine: an `Editor` with snapshot history for Do/Undo/Redo, and a Command edit API with a pure apply step ([#1327](https://github.com/itsakeyfut/avio/issues/1327))

#### ff-filter
- Raw-filter escape hatch (`FilterStep::Raw`) for passing an arbitrary filter string
- Animated rotation via a self-animating expression, supersampled and alpha-premultiplied to remove edge halos
- `pitch_shift()` extended to +/-24 semitones via an atempo chain

#### ff-preview
- Render `xfade` transition kinds in preview
- Converge audio derivation and apply per-clip volume / speed / fades in preview
- Per-clip volume automation track

#### CI / release
- Per-crate, change-driven releases via release-plz ([#1369](https://github.com/itsakeyfut/avio/issues/1369))
- Publishing-readiness gate validating metadata for all library crates ([#1371](https://github.com/itsakeyfut/avio/issues/1371))

### Changed

- **Engine / primitive split:** the editing model (Timeline / Clip, model-to-scene derivation, edit history) was relocated into `avio`, and the `ff-*` crates were purified into model-free primitives; the boundary is now enforced by dependency direction ([#1326](https://github.com/itsakeyfut/avio/issues/1326))
- Crate versions are decoupled from the shared workspace version, so each crate is now versioned independently
- De-editorialized public names on the primitive surface so the primitives impose no editing vocabulary

#### ff-decode
- `DecodeError` drops the analysis-only `AnalysisFailed` / `BpmDetectionFailed` variants (moved to `ff-analysis`'s `AnalysisError`) and gains `ExtractionFailed` for the frame / thumbnail extractor ([#1391](https://github.com/itsakeyfut/avio/issues/1391))

#### ff-encode
- `EncodeError` drops `MediaOperationFailed`; remux operations move to `ff-remux`'s `RemuxError`, and the preview-image encoders are gated behind a `preview-image` feature ([#1393](https://github.com/itsakeyfut/avio/issues/1393))

#### ff-filter
- Extensible public enums are marked `#[non_exhaustive]` so future variants stay non-breaking

### Removed

#### ff-filter
- Editorial `dissolve` / `join` `FilterStep`s and the timeline / editorial fields on `VideoLayer` / `AudioTrack`, so the primitives no longer carry model concepts

### Fixed

#### ff-encode
- Free the `AVFormatContext` / filter graph on early-return error paths in the preview-image encoders ([#1399](https://github.com/itsakeyfut/avio/issues/1399))

### Docs

- Published the engine / primitive architecture and coding rules, and reframed the crate READMEs so `avio` reads as an editing engine and the `ff-*` crates as standalone primitive libraries ([#1373](https://github.com/itsakeyfut/avio/issues/1373))
- Aligned the `ff-render` README with the crate's actual implementation status ([#1394](https://github.com/itsakeyfut/avio/issues/1394))

### Internal

- `MediaError::severity()` classification is now unit-tested per crate ([#1398](https://github.com/itsakeyfut/avio/issues/1398))

---

## [0.15.1] - 2026-08-07

This release unifies the preview and export compositing paths on a single `RealtimeComposer` and ships the fixes uncovered along the way, plus the `wgpu` 30 upgrade for the GPU renderer.

### Changed

#### ff-render
- Update `wgpu` from 29 to 30: `RequestAdapterOptions` gains an `apply_limit_buckets` field and `BufferSlice::get_mapped_range` now returns a `Result` ([#1256](https://github.com/itsakeyfut/avio/issues/1256))

### Added

#### ff-filter / ff-pipeline / ff-preview
- Unify preview and export compositing on a single `RealtimeComposer`, so `Timeline::render()` and the timeline preview no longer drift between two independent multi-track compositor implementations ([#1215](https://github.com/itsakeyfut/avio/issues/1215))

#### avio
- Re-export the `render` feature surface on the `avio` facade under the `render` namespace ([#1170](https://github.com/itsakeyfut/avio/issues/1170))

### Fixed

#### ff-filter
- `add_and_link_step` failed on compound `FilterStep`s, so the realtime compositor could not build `Glow` ([#1252](https://github.com/itsakeyfut/avio/issues/1252))
- `MultiTrackComposer` non-`Normal` blend modes failed to configure when layers differed in size ([#1248](https://github.com/itsakeyfut/avio/issues/1248))
- Multi-track export hung forever because `blend`/`composite` layers omitted `eof_action` against the infinite canvas ([#1250](https://github.com/itsakeyfut/avio/issues/1250))
- `MultiTrackComposer` canvas was hardcoded to 30 fps, stretching non-30fps exports and desyncing audio and video ([#1251](https://github.com/itsakeyfut/avio/issues/1251))

#### ff-preview
- Timeline preview dropped dimension-changing base-layer effects (`Crop` / `Scale` / `Pad`) by pushing the decoded `width` / `height` for a resized frame, freezing the monitor while audio kept playing ([#1260](https://github.com/itsakeyfut/avio/issues/1260))
- Timeline preview overlay and gap-fill layers played too fast — one source frame was consumed per present instead of by PTS ([#1249](https://github.com/itsakeyfut/avio/issues/1249))

---

## [0.15.0] - 2026-06-13

This release canonicalizes avio's public enums to FFmpeg's filter-argument tokens. **Breaking:** the colour metadata types and `BlendMode` are redesigned, and Porter-Duff compositing is separated into a new `CompositeOp` type.

### Changed

#### ff-format
- **Breaking:** `ColorSpace`, `ColorPrimaries`, and `ColorTransfer` redesigned to curated FFmpeg-canonical variants, each with a 1:1 `FfmpegToken` (e.g. `Bt601` split into `Bt470bg` / `Smpte170m`, `Bt2020` into `Bt2020Ncl` / `Bt2020Cl`, transfer `arib-std-b67` for HLG and `smpte2084` for PQ) ([#1217](https://github.com/itsakeyfut/avio/issues/1217), [#1198](https://github.com/itsakeyfut/avio/issues/1198))

#### ff-filter
- **Breaking:** `BlendMode` now maps 1:1 onto FFmpeg's `blend` `all_mode` token set (40 modes, `FfmpegToken` all-`Some`); the Porter-Duff operators and the unimplemented HSL modes are removed (Porter-Duff moves to `CompositeOp`) ([#1219](https://github.com/itsakeyfut/avio/issues/1219), [#1218](https://github.com/itsakeyfut/avio/issues/1218), [#1220](https://github.com/itsakeyfut/avio/issues/1220))

### Added

#### ff-filter
- `CompositeOp` enum (`Over` / `Under` / `In` / `Out` / `Atop` / `Xor`) and `FilterStep::Composite` for Porter-Duff alpha compositing (built via `overlay` / expression-based `blend`) ([#1221](https://github.com/itsakeyfut/avio/issues/1221))
- `FilterStep::SetParams` to tag colorspace / primaries / transfer / range on a stream from `ColorPrimaries` / `ColorTransfer` tokens ([#1227](https://github.com/itsakeyfut/avio/issues/1227))
- `AlphaMode` wired to the `overlay` `alpha=` option for compositing alpha format selection ([#1225](https://github.com/itsakeyfut/avio/issues/1225))

#### ff-pipeline
- `Clip::composite_op` and `Clip::with_composite_op()` (default `Over`), routed through `Timeline` compositing; orthogonal to the colour `blend_mode` ([#1222](https://github.com/itsakeyfut/avio/issues/1222))
- `VideoEffectRenderer`: reusable per-clip effect-chain renderer that avoids rebuilding the filter graph every frame during preview ([#1213](https://github.com/itsakeyfut/avio/issues/1213))
- preview-render path (`Clip::apply_video_effects()`) matching `Timeline::render()` so a host can apply a clip's effect chain to a single frame ([#1202](https://github.com/itsakeyfut/avio/issues/1202))

#### avio
- `CompositeOp` and `BlendMode` re-exported; `ClipTransition` added to the `filter` feature re-export block ([#1169](https://github.com/itsakeyfut/avio/issues/1169))

### Fixed

#### ff-filter
- `format` filter emitted human-readable `name()` values instead of canonical `ffmpeg_token()`, producing invalid filter arguments ([#1212](https://github.com/itsakeyfut/avio/issues/1212))
- `format()` emitted an unsupported `alpha_modes=` option into the `format` filter; alpha is now wired to the `overlay` filter instead ([#1223](https://github.com/itsakeyfut/avio/issues/1223))

### Internal
- ff-decode: deduplicate RAII guards across the audio, image, and video `decoder_inner` files ([#1168](https://github.com/itsakeyfut/avio/issues/1168))
- ff-render: split oversized `nodes/composite.rs` into per-node-type submodules ([#1166](https://github.com/itsakeyfut/avio/issues/1166))
- ff-preview: split oversized player, timeline, and clock files ([#1165](https://github.com/itsakeyfut/avio/issues/1165))
- ff-encode / ff-preview: fix bench compilation against updated APIs ([#1189](https://github.com/itsakeyfut/avio/issues/1189), [#1190](https://github.com/itsakeyfut/avio/issues/1190))
- docs.rs: build documentation with all features enabled ([#1206](https://github.com/itsakeyfut/avio/issues/1206))

---

## [0.14.4] - 2026-06-04

### Added

#### ff-pipeline
- `Clip::opacity` and `Clip::with_opacity()`: per-clip video opacity applied during timeline compositing ([#1157](https://github.com/itsakeyfut/avio/issues/1157))
- `Clip::blend_mode` and `Clip::with_blend_mode()`: per-clip photographic blend mode (Multiply, Screen, Overlay, etc.) for export compositing ([#1157](https://github.com/itsakeyfut/avio/issues/1157))
- `Clip::proxy` and `Clip::proxy()`: low-resolution proxy file to decode from during export; frames are scaled up to the original source resolution automatically ([#1178](https://github.com/itsakeyfut/avio/issues/1178))
- `Clip::video_effects` and `Clip::with_video_effect()`: per-clip ordered video filter chain (e.g. `Lut3d`, `ChromaKey`, `GBlur`) applied after built-in steps during `Timeline::render()` ([#1186](https://github.com/itsakeyfut/avio/issues/1186))
- `Clip::audio_effects` and `Clip::with_audio_effect()`: per-clip ordered audio filter chain (e.g. `ACompressor`, `ParametricEq`, `NoiseReduce`) applied after built-in steps in the audio mix ([#1186](https://github.com/itsakeyfut/avio/issues/1186))

#### ff-filter
- `ProxySource { path, width, height }`: describes a low-resolution proxy substitute for a `VideoLayer` source; frames are decoded from `path` and scaled up to `width × height` before any processing ([#1178](https://github.com/itsakeyfut/avio/issues/1178))
- `VideoLayer::proxy`: per-layer proxy support wired into the `movie` filter + immediate `scale` upscale node ([#1178](https://github.com/itsakeyfut/avio/issues/1178))
- `VideoLayer::input_format`: optional FFmpeg input format name; when `Some("lavfi")`, `source` is interpreted as a `lavfi` filtergraph string (e.g. `drawtext`) instead of a file path ([#1162](https://github.com/itsakeyfut/avio/issues/1162))
- `VideoLayer::blend_mode`: photographic blend modes on video layers use FFmpeg's `blend` filter; Normal mode continues to use `overlay` ([#1157](https://github.com/itsakeyfut/avio/issues/1157))
- `AudioTrack::in_point` / `AudioTrack::out_point`: source-window trim fields; when set, an `atrim + asetpts` chain is inserted after the `amovie` node so the correct audio window is exported ([#1176](https://github.com/itsakeyfut/avio/issues/1176))
- `TimelineBuilder::lavfi_overlay()`: composites a `lavfi` filtergraph string as the topmost video layer during rendering without requiring a pre-encoded title clip ([#1162](https://github.com/itsakeyfut/avio/issues/1162))
- `escape_filter_path()` (internal): normalises Windows paths for FFmpeg's filter-argument parser; `FilterStep::Lut3d` now uses it to prevent drive-colon parse errors on Windows ([#1182](https://github.com/itsakeyfut/avio/issues/1182))

#### avio
- `ProxySource` re-exported from the `filter` feature block ([#1191](https://github.com/itsakeyfut/avio/pull/1191))

### Fixed

#### ff-preview
- V2/V3 overlay layers were invisible when `Clip::opacity < 1.0`; root causes: inverted NLE track order (V1 rendered on top of V2), missing opacity wiring, and `SwsRgbaConverter` always outputting at source resolution so the size-equality guard silently skipped compositing ([#1157](https://github.com/itsakeyfut/avio/issues/1157), [#1158](https://github.com/itsakeyfut/avio/issues/1158), [#1159](https://github.com/itsakeyfut/avio/issues/1159), [#1160](https://github.com/itsakeyfut/avio/issues/1160))
- `SwsRgbaConverter::convert_to()` added: accepts explicit `(dst_w, dst_h)` for bilinear scaling so overlay frames always match V1's canvas size ([#1160](https://github.com/itsakeyfut/avio/issues/1160))

#### ff-filter
- `build_audio_graph` applied video-only `FilterStep` variants (Scale, Crop, Eq, etc.) to the audio buffersrc, causing "Media type mismatch" failures; an audio-only allowlist now skips non-audio steps ([#1179](https://github.com/itsakeyfut/avio/issues/1179))
- `FilterGraphInner::ensure_audio_graph` freed the audio `AVFilterGraph` while its `AVFilterContext` pointers were still live, causing `STATUS_ACCESS_VIOLATION` crashes; the audio graph is now stored in a separate `audio_graph` field and freed in `Drop` ([#1180](https://github.com/itsakeyfut/avio/issues/1180))
- `FilterStep::Lut3d` generated unescaped Windows paths (e.g. `D:\luts\look.cube`) as filter arguments, causing parse failures; backslashes are now normalised to forward slashes and the drive colon is escaped ([#1182](https://github.com/itsakeyfut/avio/issues/1182))

#### ff-pipeline
- `Timeline::render()` did not forward `clip.in_point` / `clip.out_point` to `AudioTrack`, so audio always played from the beginning of the source file regardless of loop-region trim bounds ([#1176](https://github.com/itsakeyfut/avio/issues/1176))

---

## [0.14.3] - 2026-05-05

### Added

#### ff-pipeline
- `Clip::volume_db` and `Clip::with_volume_db()`: per-clip audio gain in decibels applied at both export and real-time preview ([#1142](https://github.com/itsakeyfut/avio/issues/1142))
- `Clip::fade_in`, `Clip::fade_out`, and `Clip::with_audio_fades()`: per-clip audio fade-in/fade-out with live preview envelope ([#1144](https://github.com/itsakeyfut/avio/issues/1144), [#1145](https://github.com/itsakeyfut/avio/issues/1145), [#1146](https://github.com/itsakeyfut/avio/issues/1146))
- `Clip::brightness`, `Clip::contrast`, `Clip::saturation`, and `Clip::with_color_correction()`: per-clip RGB color correction wired through the `eq` filter on export and `VideoLayer::effects` in preview ([#1150](https://github.com/itsakeyfut/avio/issues/1150))
- `Clip::speed` and `Clip::with_speed()`: per-clip variable-speed playback; video uses `setpts+fps`, audio uses an `asetrate+aresample+aeval` chain to avoid `atempo` NaN artifacts at high speeds ([#1153](https://github.com/itsakeyfut/avio/issues/1153))

#### ff-filter
- `VideoLayer::effects`: per-layer `Vec<FilterStep>` applied before the xfade compositor node, enabling per-clip GPU effects in multi-track export ([#1150](https://github.com/itsakeyfut/avio/issues/1150))
- `FilterStep::Speed { factor }`: new variant; video path inserts `setpts=PTS/factor` + `fps=30`, audio path uses the new asetrate/aresample chain ([#1153](https://github.com/itsakeyfut/avio/issues/1153))
- `add_asetrate_resample_chain`: new `pub(crate)` helper that builds an `apad→asetrate→aresample→aeval` chain for NaN-free high-speed audio; `decompose_atempo` max raised from 2.0 to 100.0 (FFmpeg 4.0+ cap) ([#1153](https://github.com/itsakeyfut/avio/issues/1153))

#### ff-preview
- `PlayerHandle::set_rate()` with negative values: reverse playback via per-frame `seek_coarse`, muted audio, and seamless forward-recovery on direction change ([#1136](https://github.com/itsakeyfut/avio/issues/1136), [#1137](https://github.com/itsakeyfut/avio/issues/1137), [#1138](https://github.com/itsakeyfut/avio/issues/1138))
- `update_timeline()` on `PlayerHandle`: hot-reload a modified `Timeline` layout without restarting the runner ([#1131](https://github.com/itsakeyfut/avio/issues/1131))
- `ClipState::speed` in `TimelineRunner`: PTS remapping for variable-speed clips with per-chunk linear audio resampling ([#1154](https://github.com/itsakeyfut/avio/issues/1154))

### Fixed

#### ff-preview
- Frame-stepping while paused: a single decoded frame is now presented to the sink after a seek command arrives while the player is paused ([#1140](https://github.com/itsakeyfut/avio/issues/1140))
- `SwrContext` was re-allocated on every audio frame, causing crackling; it is now cached and reused across frames ([#1116](https://github.com/itsakeyfut/avio/issues/1116))
- `MasterClock` was not re-anchored on `Play`, causing accumulated pause-drift to skew A/V sync after long pauses ([#1130](https://github.com/itsakeyfut/avio/issues/1130))
- `MasterClock` rate scaling was applied twice in `pop_audio_samples`, causing double-speed audio at rates other than 1× ([#1118](https://github.com/itsakeyfut/avio/issues/1118), [#1119](https://github.com/itsakeyfut/avio/issues/1119))
- `TimelinePlayer` did not composite secondary video tracks (V2/V3) or mix audio-only tracks (A1/A2); both are now handled ([#1121](https://github.com/itsakeyfut/avio/issues/1121), [#1122](https://github.com/itsakeyfut/avio/issues/1122))
- Overlay z-order was inverted: V1 (primary) is now rendered as foreground, V2/V3 as background ([#1124](https://github.com/itsakeyfut/avio/issues/1124))
- `TimelineRunner` froze when a gap before the first clip was encountered, stale audio threads were not cancelled on seek, and audio started prematurely for clips beginning after a pre-roll gap ([#1148](https://github.com/itsakeyfut/avio/issues/1148))
- Partial `in_point`/`out_point` trim bounds caused filter graph build failures; stale audio decoder threads were not cancelled on clip transition ([#1133](https://github.com/itsakeyfut/avio/issues/1133), [#1134](https://github.com/itsakeyfut/avio/issues/1134))

#### ff-encode
- Audio stream in exported files was missing extradata, causing some players to reject the file; `avcodec_parameters_from_context` is now used after codec open ([#1126](https://github.com/itsakeyfut/avio/issues/1126))
- Encoder produced truncated audio because sample frames were pushed without buffering; `AVAudioFifo` now accumulates samples to exactly `frame_size` before encoding ([#1127](https://github.com/itsakeyfut/avio/issues/1127), [#1128](https://github.com/itsakeyfut/avio/issues/1128))

#### ff-decode
- `SwrContext` was re-created on every audio frame, causing audio crackling during `AudioDecoder` iteration ([#1116](https://github.com/itsakeyfut/avio/issues/1116))

---

## [0.14.2] - 2026-04-22

### Fixed

- `MasterClock::Audio` used the file's native sample rate (e.g. 44,100 Hz) instead of the decoder's fixed output rate (48,000 Hz), causing audio to play at ~1.088× speed ([#1110](https://github.com/itsakeyfut/avio/issues/1110))
- `MasterClock::Audio` froze when the audio track ended before the video track, stalling `PlayerRunner::run()` indefinitely; fixed via wall-clock fallback re-arm on stall detection ([#1112](https://github.com/itsakeyfut/avio/issues/1112))
- `spawn_audio_thread` silently dropped samples when the ring buffer was full on Windows (where `sleep(1 ms)` ≈ 10 ms), causing audio to play at ~2.3× speed; fixed by retrying until all samples are pushed ([#1114](https://github.com/itsakeyfut/avio/issues/1114))

---

## [0.14.1] - 2026-04-21

### Fixed

- `MasterClock::Audio::should_sync()` returned `false` indefinitely when no audio consumer was connected, causing video to play at decoder speed instead of real time ([#1105](https://github.com/itsakeyfut/avio/issues/1105))
- `avio` facade was missing re-exports for `PlayerCommand`, `HardwareAccel` (preview-only), `TimelinePlayer`, and `TimelineRunner` under the `preview` feature ([#1106](https://github.com/itsakeyfut/avio/issues/1106))

---

## [0.14.0] - 2026-04-20

### Added

#### ff-render — new crate
- New `ff-render` crate: GPU compositing pipeline for real-time video preview, built on [wgpu] ([#1085](https://github.com/itsakeyfut/avio/issues/1085))
- `RenderContext`: headless wgpu device/queue initialisation (no window required) ([#1085](https://github.com/itsakeyfut/avio/issues/1085))
- `RenderGraph`: linear chain of render nodes; CPU fallback path always available without `wgpu` feature ([#1085](https://github.com/itsakeyfut/avio/issues/1085))
- `GpuFrameSink`: implements `ff_preview::FrameSink` for zero-copy wiring with `PlayerRunner` ([#1085](https://github.com/itsakeyfut/avio/issues/1085))
- `ColorGradeNode`: brightness, saturation, contrast, hue shift, colour temperature ([#1085](https://github.com/itsakeyfut/avio/issues/1085))
- `ScaleNode`: GPU-accelerated resize (Bilinear / Nearest); CPU path is passthrough ([#1085](https://github.com/itsakeyfut/avio/issues/1085))
- `OverlayNode`, `CrossfadeNode`: static overlay composite and crossfade transition ([#1085](https://github.com/itsakeyfut/avio/issues/1085))
- `YuvUploadNode`: native YUV plane upload (4:2:0 / 4:2:2 / 4:4:4) bypassing `sws_scale` ([#1086](https://github.com/itsakeyfut/avio/issues/1086))
- `BlendModeNode`: 12 Photoshop-style blend modes (Multiply, Screen, Overlay, Darken, Lighten, ColorDodge, ColorBurn, HardLight, SoftLight, Difference, Exclusion, Normal) with per-node opacity ([#1087](https://github.com/itsakeyfut/avio/issues/1087))
- `TransformNode`: UV-space translate / rotate / scale ([#1087](https://github.com/itsakeyfut/avio/issues/1087))
- `ChromaKeyNode`: chroma key (green screen) with tolerance and softness ([#1087](https://github.com/itsakeyfut/avio/issues/1087))
- `ShapeMaskNode`, `LumaMaskNode`, `AlphaMatteNode`: shape mask, luma-derived mask, and alpha-composite over background ([#1087](https://github.com/itsakeyfut/avio/issues/1087))
- `Compositor`: stateful high-level multi-layer GPU compositor — sorts layers by `z_order`, applies per-layer transforms and blend modes, returns composited `wgpu::Texture` ([#1042](https://github.com/itsakeyfut/avio/issues/1042))
- `FrameLayer` and `LayerTransform`: layer definition with transform, blend mode, opacity, and z-order ([#1042](https://github.com/itsakeyfut/avio/issues/1042))

#### ff-preview
- Split `PreviewPlayer` into `PlayerRunner` (owns the decode thread) and `PlayerHandle` (cross-thread control) for safe cross-thread use ([#1021](https://github.com/itsakeyfut/avio/issues/1021))
- `PlayerEvent`: new `PositionUpdate` and `Error` variants for real-time status monitoring ([#1022](https://github.com/itsakeyfut/avio/issues/1022))
- `TimelinePlayer`: real-time timeline-driven playback with per-clip ordering ([#1023](https://github.com/itsakeyfut/avio/issues/1023))
- `FrameCache`: instant seek scrubbing via pre-decoded frame cache ([#1024](https://github.com/itsakeyfut/avio/issues/1024))
- `AudioMixer` and `AudioTrackHandle`: multi-track audio mixing with per-track volume and mute control ([#1025](https://github.com/itsakeyfut/avio/issues/1025))
- `HardwareAccel` integration in `PlayerRunner` and `DecodeBufferBuilder` ([#1026](https://github.com/itsakeyfut/avio/issues/1026))

#### ff-decode
- `ScopeAnalyzer::waveform()`: luminance waveform monitor — Y values per column, normalised `[0.0, 1.0]` ([#408](https://github.com/itsakeyfut/avio/issues/408))
- `ScopeAnalyzer::vectorscope()`: Cb/Cr chroma scatter data for 4:2:0, 4:2:2, and 4:4:4 frames ([#409](https://github.com/itsakeyfut/avio/issues/409))
- `ScopeAnalyzer::rgb_parade()`: per-channel RGB waveform (parade) — column-major normalised values ([#410](https://github.com/itsakeyfut/avio/issues/410))
- `ScopeAnalyzer::histogram()`: 256-bin luminance and per-channel RGB histogram ([#411](https://github.com/itsakeyfut/avio/issues/411))
- `AsyncVideoDecoderBuilder` and `AsyncAudioDecoderBuilder`: async builder API with `output_format`, `output_size`, `output_sample_rate`, and `output_channels` options ([#1005](https://github.com/itsakeyfut/avio/issues/1005))

#### ff-filter
- `Stabilizer::analyze()`: two-pass video stabilization — pass 1 via `vidstabdetect` ([#392](https://github.com/itsakeyfut/avio/issues/392))
- `Stabilizer::transform()`: two-pass video stabilization — pass 2 via `vidstabtransform` ([#393](https://github.com/itsakeyfut/avio/issues/393))
- `StabilizeOptions`: zoom, optzoom, and interpol builder methods ([#394](https://github.com/itsakeyfut/avio/issues/394))
- `FilterGraph::motion_blur()`: motion blur via `tblend` shutter-angle simulation ([#395](https://github.com/itsakeyfut/avio/issues/395))
- `FilterGraph::film_grain()`: per-frame random grain via `noise` filter with temporal seed ([#396](https://github.com/itsakeyfut/avio/issues/396))
- `FilterGraph::lens_correction()`: radial lens distortion correction via `lenscorrection` ([#397](https://github.com/itsakeyfut/avio/issues/397))
- `LensProfile` enum and `FilterGraph::lens_profile()`: preset and custom lens correction profiles ([#398](https://github.com/itsakeyfut/avio/issues/398))
- `FilterGraph::fix_chromatic_aberration()`: lateral chromatic aberration correction via `rgbashift` ([#399](https://github.com/itsakeyfut/avio/issues/399))
- `FilterGraph::glow()`: bloom / glow around bright areas via `split+curves+gblur+blend` chain ([#400](https://github.com/itsakeyfut/avio/issues/400))
- `FilterGraph::reverb_ir()`: convolution reverb via `amovie+afir` impulse response ([#401](https://github.com/itsakeyfut/avio/issues/401))
- `FilterGraph::reverb_echo()`: echo / reverb via `aecho` with configurable delay taps and decay ([#402](https://github.com/itsakeyfut/avio/issues/402))
- `FilterGraph::pitch_shift()`: pitch shift in semitones via `asetrate+atempo` ([#403](https://github.com/itsakeyfut/avio/issues/403))
- `FilterGraph::time_stretch()`: time stretch without pitch change via `atempo` chain ([#404](https://github.com/itsakeyfut/avio/issues/404))
- `FilterGraph::speed_change()`: playback speed change via `asetrate` ([#405](https://github.com/itsakeyfut/avio/issues/405))
- `FilterGraph::noise_reduce()` and `noise_reduce_profile()`: broadband noise reduction via `afftdn` ([#406](https://github.com/itsakeyfut/avio/issues/406))
- `FilterGraph::duck()`: audio ducking via `sidechaincompress` ([#407](https://github.com/itsakeyfut/avio/issues/407))

---

## [0.13.1] - 2026-04-16

### Added

#### ff-pipeline
- `Clip::transition` field and xfade filter wiring in `Timeline::render` ([#1015](https://github.com/itsakeyfut/avio/issues/1015))
- `Timeline::render_with_progress()` with per-frame progress callback and cancellation support ([#1016](https://github.com/itsakeyfut/avio/issues/1016))

#### ff-preview
- Audio-only file support in `PreviewPlayer::open()` and `run()` ([#1009](https://github.com/itsakeyfut/avio/issues/1009))
- `set_rate()` and `rate_handle()` for runtime playback speed control ([#1014](https://github.com/itsakeyfut/avio/issues/1014))
- `av_offset_handle()`: cloneable `Arc<AtomicI64>` handle for A/V offset from outside the player ([#1013](https://github.com/itsakeyfut/avio/issues/1013))
- `current_pts()` and `duration()` for real-time position and file length queries ([#1012](https://github.com/itsakeyfut/avio/issues/1012))
- `seek_coarse()` exposed at both `PreviewPlayer` and `AsyncPreviewPlayer` level ([#1011](https://github.com/itsakeyfut/avio/issues/1011))
- `pause_handle()`: cloneable `Arc<AtomicBool>` handle for external pause control ([#1008](https://github.com/itsakeyfut/avio/issues/1008))

### Changed

#### ff-preview
- `pause()` and `play()` now take `&self` instead of `&mut self` ([#1008](https://github.com/itsakeyfut/avio/issues/1008))
- `pop_audio_samples()` receiver changed from `&mut self` to `&self` ([#1007](https://github.com/itsakeyfut/avio/issues/1007))

### Fixed

#### ff-decode
- `into_stream` now terminates after the first error; subsequent polls return `None` ([#1006](https://github.com/itsakeyfut/avio/issues/1006))

#### ff-encode
- `PixelFormat::Other(v)` now passes through `pixel_format_to_av` without silently falling back to `yuv420p` ([#1018](https://github.com/itsakeyfut/avio/issues/1018))

#### ff-filter
- Insert explicit `format=yuv420p` filter before video buffersinks to prevent `gbrp` format negotiation on newer FFmpeg builds ([#1017](https://github.com/itsakeyfut/avio/issues/1017))

---

## [0.13.0] - 2026-04-13

### Added

#### ff-preview — new crate
- New `ff-preview` crate: real-time video preview and proxy workflow ([#369](https://github.com/itsakeyfut/avio/issues/369))
- `PlaybackClock`: start / stop / pause / resume with nanosecond resolution ([#370](https://github.com/itsakeyfut/avio/issues/370))
- Playback rate control (0.25×, 0.5×, 1×, 2×, arbitrary fractional) via `set_rate` ([#371](https://github.com/itsakeyfut/avio/issues/371))
- `current_pts` and `set_position` for real-time position query and seek support ([#372](https://github.com/itsakeyfut/avio/issues/372))
- `DecodeBuffer`: configurable frame decode-ahead ring buffer (default 8 frames) ([#373](https://github.com/itsakeyfut/avio/issues/373))
- Background decode thread with bounded channel and back-pressure ([#374](https://github.com/itsakeyfut/avio/issues/374))
- Frame-accurate seek: I-frame seek + forward decode to target PTS ([#375](https://github.com/itsakeyfut/avio/issues/375))
- Coarse seek: nearest I-frame only (fast path for scrub-bar drag) ([#376](https://github.com/itsakeyfut/avio/issues/376))
- `FrameResult` enum and non-blocking `seek_async`: returns `Seeking` placeholder while decoder catches up ([#377](https://github.com/itsakeyfut/avio/issues/377))
- `SeekEvent` channel: seek completion notification for UI synchronization ([#378](https://github.com/itsakeyfut/avio/issues/378))
- `PreviewPlayer` with A/V sync using audio master clock ([#379](https://github.com/itsakeyfut/avio/issues/379))
- `MasterClock::System` fallback for video-only files ([#380](https://github.com/itsakeyfut/avio/issues/380))
- `set_av_offset`: configurable ±ms A/V offset correction ([#381](https://github.com/itsakeyfut/avio/issues/381))
- Audio PCM delivery aligned to presentation clock via background decode thread and ring buffer ([#382](https://github.com/itsakeyfut/avio/issues/382))
- `FrameSink` trait (`Send`, called on dedicated thread) for custom frame consumers ([#383](https://github.com/itsakeyfut/avio/issues/383))
- `RgbaFrame` and `RgbaSink`: reference `FrameSink` implementation using `sws_scale` to deliver contiguous RGBA `Vec<u8>` ([#384](https://github.com/itsakeyfut/avio/issues/384))
- `ProxyGenerator`: generates down-scaled proxy files at 1/2, 1/4, or 1/8 resolution with configurable codec ([#385](https://github.com/itsakeyfut/avio/issues/385))
- `use_proxy_if_available` and `active_source` for transparent proxy substitution during playback ([#386](https://github.com/itsakeyfut/avio/issues/386))
- `ProxyJob` and `generate_async` for non-blocking background proxy generation ([#387](https://github.com/itsakeyfut/avio/issues/387))
- `AsyncPreviewPlayer` behind `tokio` feature flag ([#388](https://github.com/itsakeyfut/avio/issues/388))
- `stop_handle() -> Arc<AtomicBool>`: cloneable stop signal for use inside `FrameSink::push_frame`

#### avio — facade additions
- `RgbaSink` and `RgbaFrame` re-exported under the `preview` feature ([#999](https://github.com/itsakeyfut/avio/issues/999))
- `AsyncPreviewPlayer` re-exported under `preview + tokio`; `ff-preview/tokio` wired into the `tokio` feature ([#1000](https://github.com/itsakeyfut/avio/issues/1000))
- `preview-proxy` feature: re-exports `ProxyGenerator`, `ProxyJob`, and `ProxyResolution` ([#1001](https://github.com/itsakeyfut/avio/issues/1001))

### Fixed

- `SeekEvent` race condition: event sent before the frame is pushed to avoid `try_recv` miss on the receiver side ([#379](https://github.com/itsakeyfut/avio/issues/379))
- Circular dev-dependency between `ff-decode` and `ff-encode`: replaced cross-crate asset generation with pre-committed test assets ([#970](https://github.com/itsakeyfut/avio/issues/970))
- Broken intra-doc links in `sink.rs` after module split ([#992](https://github.com/itsakeyfut/avio/issues/992))

### Tests

- Integration test: frame-accurate seek to t=30s returns frame within ±1 frame period (±34 ms at 30 fps) ([#996](https://github.com/itsakeyfut/avio/issues/996))
- Integration test: `RgbaSink` decodes ≥10 real frames to correctly-sized, non-blank RGBA buffers ([#997](https://github.com/itsakeyfut/avio/issues/997))
- Integration test: proxy generation at 1/4 resolution produces 480×270 output; half-resolution substitution delivers 960×540 frames ([#998](https://github.com/itsakeyfut/avio/issues/998))
- Integration test: A/V sync consecutive-frame jitter ≤67 ms over 60-second playback ([#390](https://github.com/itsakeyfut/avio/issues/390))
- Criterion benchmark: 1080p/30 fps playback loop frames delivered on time ([#389](https://github.com/itsakeyfut/avio/issues/389))

---

## [0.12.0] - 2026-04-12

### Added

#### ff-filter — keyframe animation types
- `Keyframe<T>` — timestamp + value + per-segment easing, ordered by timestamp ([#349](https://github.com/itsakeyfut/avio/issues/349))
- `AnimationTrack<T>` — sorted keyframe storage with `value_at(Duration)` interpolation ([#350](https://github.com/itsakeyfut/avio/issues/350))
- `Lerp` trait implementations for `(f64, f64)` and `(f64, f64, f64)` tuple types ([#351](https://github.com/itsakeyfut/avio/issues/351))
- `AnimationTrack::<f64>::fade()` — two-keyframe convenience constructor for volume fades, opacity ramps, and position sweeps ([#362](https://github.com/itsakeyfut/avio/issues/362))
- `AnimatedValue<T>` — enum wrapping either a static `f64` or a live `AnimationTrack<f64>` ([#358](https://github.com/itsakeyfut/avio/issues/358))

#### ff-filter — easing modes
- `Easing::Hold` — step (snap-at-boundary) interpolation ([#352](https://github.com/itsakeyfut/avio/issues/352))
- `Easing::Linear` — linear interpolation ([#353](https://github.com/itsakeyfut/avio/issues/353))
- `Easing::EaseIn` — cubic ease-in (t³) ([#354](https://github.com/itsakeyfut/avio/issues/354))
- `Easing::EaseOut` — cubic ease-out (1−(1−t)³) ([#355](https://github.com/itsakeyfut/avio/issues/355))
- `Easing::EaseInOut` — cubic ease-in-out (smoothstep 3t²−2t³) ([#356](https://github.com/itsakeyfut/avio/issues/356))
- `Easing::Bezier { p1, p2 }` — CSS cubic-bezier via Newton-Raphson root finding ([#357](https://github.com/itsakeyfut/avio/issues/357))

#### ff-filter — animated filter parameters
- `VideoLayer` fields `x`, `y`, `scale_x`, `scale_y`, `rotation`, `opacity` replaced with `AnimatedValue<f64>` ([#358](https://github.com/itsakeyfut/avio/issues/358))
- `FilterGraphBuilder::crop_animated()` and `gblur_animated()` — animated crop rectangle and blur radius ([#359](https://github.com/itsakeyfut/avio/issues/359))
- `FilterGraphBuilder::eq_animated()` and `colorbalance_animated()` — animated brightness, contrast, saturation, and lift/gamma/gain ([#360](https://github.com/itsakeyfut/avio/issues/360))
- `AudioTrack` fields `volume` and `pan` replaced with `AnimatedValue<f64>` ([#361](https://github.com/itsakeyfut/avio/issues/361))
- `FilterGraph::tick(t: Duration)` — applies all registered `AnimationEntry` values at time `t` via `avfilter_graph_send_command`; call before each `pull_video` / `pull_audio` on source-only graphs ([#363](https://github.com/itsakeyfut/avio/issues/363))
- `AnimationEntry::suffix` field — appends a unit string (e.g. `"dB"`) to the formatted value sent to FFmpeg, required for filters whose options accept expression strings ([#363](https://github.com/itsakeyfut/avio/issues/363))

#### ff-pipeline — Timeline animation
- `TimelineBuilder::video_animation()` and `audio_animation()` — attach `AnimationTrack<f64>` keyed by `"video_{idx}_{prop}"` / `"audio_{idx}_{prop}"` ([#364](https://github.com/itsakeyfut/avio/issues/364))
- `Timeline::render()` now calls `tick(pts)` before every `pull_video` and `pull_audio`, activating all registered animation tracks during render ([#968](https://github.com/itsakeyfut/avio/issues/968))

#### ff-filter — serde
- Optional `serde` feature flag: `Serialize` / `Deserialize` for `Keyframe<T>`, `AnimationTrack<T>`, `AnimatedValue<T>`, and all six `Easing` variants ([#365](https://github.com/itsakeyfut/avio/issues/365))

### Fixed

- `Timeline::render()` never called `FilterGraph::tick()` — all animation tracks passed via `video_animation` / `audio_animation` were silently ignored; every frame rendered with t=0 parameter values ([#968](https://github.com/itsakeyfut/avio/issues/968))
- Animated `volume` filter sent plain float values (e.g. `"-60.000000"`) instead of dB-suffixed expressions (e.g. `"-60.000000dB"`), causing incorrect gain interpretation ([#363](https://github.com/itsakeyfut/avio/issues/363))
- Animated overlay opacity required `format=yuva420p` conversion and `overlay format=auto` to correctly read the alpha plane ([#358](https://github.com/itsakeyfut/avio/issues/358))
- `avfilter_graph_send_command` declared via local `extern` block for cross-platform compatibility (Windows MSVC linkage) ([#363](https://github.com/itsakeyfut/avio/issues/363))

### Tests

- Frame-accurate unit tests for all six `Easing` modes including Bezier Newton-Raphson convergence ([#366](https://github.com/itsakeyfut/avio/issues/366))
- Integration test: Bezier-eased x-position animation verified against a standalone reference curve within ±2 px per frame ([#367](https://github.com/itsakeyfut/avio/issues/367))
- Integration test: animated opacity fade darkens composite output over 30 frames ([#966](https://github.com/itsakeyfut/avio/pull/966))
- Integration test: volume automation increases audio RMS amplitude from near-silence to full level ([#966](https://github.com/itsakeyfut/avio/pull/966))
- Integration test: `Timeline` with a volume fade track encodes without error ([#967](https://github.com/itsakeyfut/avio/pull/967))

---

## [0.11.0] - 2026-04-10

### Added

#### ff-filter — blend modes
- `BlendMode` enum with 14 photographic blend modes (Normal, Multiply, Screen, Overlay, SoftLight, HardLight, ColorDodge, ColorBurn, Darken, Lighten, Difference, Exclusion, Add, Subtract) and 4 HSL-space variants (accepted but unsupported by bundled FFmpeg) ([#327](https://github.com/itsakeyfut/avio/issues/327)–[#334](https://github.com/itsakeyfut/avio/issues/334))
- `FilterGraphBuilder::blend()` — layer a top `FilterGraphBuilder` over the main stream with a specified `BlendMode` and opacity ([#327](https://github.com/itsakeyfut/avio/issues/327))
- Porter-Duff Over compositing (`BlendMode::PorterDuffOver`) via `overlay=format=auto:shortest=1` ([#335](https://github.com/itsakeyfut/avio/issues/335))
- Porter-Duff Under, In, Out operations ([#336](https://github.com/itsakeyfut/avio/issues/336))
- Porter-Duff Atop and XOR operations ([#337](https://github.com/itsakeyfut/avio/issues/337))

#### ff-filter — keying
- `FilterGraphBuilder::chromakey()` — YCbCr chroma key with similarity and blend parameters ([#338](https://github.com/itsakeyfut/avio/issues/338))
- `FilterGraphBuilder::colorkey()` — RGB-space color removal ([#339](https://github.com/itsakeyfut/avio/issues/339))
- `FilterGraphBuilder::spill_suppress()` — reduce chroma spill on keyed subject edges ([#340](https://github.com/itsakeyfut/avio/issues/340))
- `FilterGraphBuilder::lumakey()` — key out bright or dark regions by luminance ([#341](https://github.com/itsakeyfut/avio/issues/341))

#### ff-filter — masking
- `FilterGraphBuilder::alpha_matte()` — merge a grayscale matte stream as the alpha channel of the main video ([#342](https://github.com/itsakeyfut/avio/issues/342))
- `FilterGraphBuilder::rect_mask()` — rectangular alpha mask via `geq` filter ([#343](https://github.com/itsakeyfut/avio/issues/343))
- `FilterGraphBuilder::polygon_matte()` — polygon mask with up to 16 vertices in normalised coordinates, using a crossing-number point-in-polygon test ([#344](https://github.com/itsakeyfut/avio/issues/344))
- `FilterGraphBuilder::feather_mask()` — Gaussian blur of the alpha channel edges for smooth compositing ([#345](https://github.com/itsakeyfut/avio/issues/345))

#### avio (facade)
- `BlendMode` re-exported under the `filter` feature flag ([#930](https://github.com/itsakeyfut/avio/issues/930))
- New compositing examples: `chroma_key_green_screen`, `blend_modes_demo`, `luma_key_title_card`, `polygon_garbage_matte`, `mask_feathering`, `alpha_matte_external` ([#932](https://github.com/itsakeyfut/avio/issues/932)–[#937](https://github.com/itsakeyfut/avio/issues/937))

### Fixed

- `BlendMode::ColorDodge` mapped to wrong FFmpeg mode name (`colordodge` → `dodge`) ([#347](https://github.com/itsakeyfut/avio/issues/347))
- `BlendMode::ColorBurn` mapped to wrong FFmpeg mode name (`colorburn` → `burn`) ([#347](https://github.com/itsakeyfut/avio/issues/347))

### Tests

- Golden-image regression test for all 18 photographic blend modes against committed reference PNGs within ±2 per-channel tolerance ([#347](https://github.com/itsakeyfut/avio/issues/347))
- End-to-end compositing pipeline integration test: polygon garbage matte → chroma key → Porter-Duff Over blend ([#346](https://github.com/itsakeyfut/avio/issues/346))

---

## [0.10.0] - 2026-04-08

### Added

#### ff-pipeline
- `Clip` value type with source path, in/out trim, timeline offset, and duration methods ([#673](https://github.com/itsakeyfut/avio/issues/673))
- `Timeline` and `TimelineBuilder` for ordered video and audio track layout built from `Clip` instances ([#674](https://github.com/itsakeyfut/avio/issues/674))
- `Timeline::render()` — composite and encode a `Timeline` to a video output file ([#675](https://github.com/itsakeyfut/avio/issues/675))
- `PipelineError::ClipNotFound` and `PipelineError::TimelineRenderFailed` variants ([#826](https://github.com/itsakeyfut/avio/issues/826))

#### ff-filter — multi-track composition
- `MultiTrackComposer` for layered video composition with per-track position, scale, and opacity ([#296](https://github.com/itsakeyfut/avio/issues/296))
- `MultiTrackAudioMixer` for multi-track audio mixing with per-track volume and time offset ([#296](https://github.com/itsakeyfut/avio/issues/296))
- Auto-resample and reformat mismatched `AudioTrack` sources ([#299](https://github.com/itsakeyfut/avio/issues/299))
- Per-track effects chain to `AudioTrack` ([#679](https://github.com/itsakeyfut/avio/issues/679))
- Track time offset via `adelay` for `MultiTrackAudioMixer` ([#300](https://github.com/itsakeyfut/avio/issues/300))
- Non-zero canvas dimension validation in `MultiTrackComposer` ([#297](https://github.com/itsakeyfut/avio/issues/297))
- `FilterError::CompositionFailed` and `FilterError::AnalysisFailed` variants ([#824](https://github.com/itsakeyfut/avio/issues/824))

#### ff-filter — clip operations
- `VideoConcatenator` for multi-clip seamless video concatenation ([#301](https://github.com/itsakeyfut/avio/issues/301))
- `AudioConcatenator` for multi-clip seamless audio concatenation ([#302](https://github.com/itsakeyfut/avio/issues/302))
- `ClipJoiner` for cross-dissolve transition between two clips ([#304](https://github.com/itsakeyfut/avio/issues/304))

#### ff-filter — analysis
- `LoudnessMeter` for EBU R128 integrated loudness, true peak, and loudness range measurement ([#309](https://github.com/itsakeyfut/avio/issues/309))
- `QualityMetrics::ssim` for SSIM video quality measurement ([#313](https://github.com/itsakeyfut/avio/issues/313))
- `QualityMetrics::psnr` for PSNR video quality measurement ([#314](https://github.com/itsakeyfut/avio/issues/314))

#### ff-decode — analysis
- `WaveformAnalyzer` for per-channel waveform data extraction ([#308](https://github.com/itsakeyfut/avio/issues/308))
- `SilenceDetector` for audio silence interval detection ([#312](https://github.com/itsakeyfut/avio/issues/312))
- `KeyframeEnumerator` for keyframe timestamp enumeration ([#311](https://github.com/itsakeyfut/avio/issues/311))
- `SceneDetector` for scene change detection ([#310](https://github.com/itsakeyfut/avio/issues/310))
- `BlackFrameDetector` for black interval detection in video ([#316](https://github.com/itsakeyfut/avio/issues/316))
- `HistogramExtractor` for per-frame RGB and luma histogram extraction ([#315](https://github.com/itsakeyfut/avio/issues/315))
- `DecodeError::NoFrameAtTimestamp` and `DecodeError::AnalysisFailed` variants ([#823](https://github.com/itsakeyfut/avio/issues/823))

#### ff-decode — frame operations
- `VideoDecoder::extract_frame` for single frame extraction at a timestamp ([#317](https://github.com/itsakeyfut/avio/issues/317))
- `FrameExtractor` for batch frame extraction at regular intervals ([#318](https://github.com/itsakeyfut/avio/issues/318))
- `ThumbnailSelector` for automatic best-frame selection (skips black/white/blurry frames) ([#321](https://github.com/itsakeyfut/avio/issues/321))

#### ff-encode — media operations
- `StreamCopyTrim` for stream-copy clip trimming with `Duration` API ([#303](https://github.com/itsakeyfut/avio/issues/303))
- `AudioReplacement` for stream-copy audio track replacement ([#305](https://github.com/itsakeyfut/avio/issues/305))
- `AudioExtractor` for stream-copy audio track extraction ([#306](https://github.com/itsakeyfut/avio/issues/306))
- `AudioAdder` for muxing audio into silent video with optional looping ([#307](https://github.com/itsakeyfut/avio/issues/307))
- `SpriteSheet` for thumbnail sprite sheet generation ([#319](https://github.com/itsakeyfut/avio/issues/319))
- `GifPreview` for animated GIF generation via two-pass palettegen ([#320](https://github.com/itsakeyfut/avio/issues/320))
- `EncodeError::MediaOperationFailed` variant ([#825](https://github.com/itsakeyfut/avio/issues/825))

#### ff-encode — export presets
- `ExportPreset` with YouTube 1080p and YouTube 4K presets and platform constraint validation ([#322](https://github.com/itsakeyfut/avio/issues/322), [#324](https://github.com/itsakeyfut/avio/issues/324), [#325](https://github.com/itsakeyfut/avio/issues/325))
- Additional presets: Twitter, Instagram, Blu-ray, Podcast, Lossless, Web ([#323](https://github.com/itsakeyfut/avio/issues/323))

#### ff-format
- `VideoFrame` and `AudioFrame` public clone API for multi-track and effect pipeline use ([#672](https://github.com/itsakeyfut/avio/issues/672))

#### avio (facade)
- Re-exported all v0.10.0 public types under feature flags ([#827](https://github.com/itsakeyfut/avio/issues/827))

### Refactored

- Split large source files into per-concern submodules across `ff-decode`, `ff-encode`, `ff-filter`, and `ff-format` (issues [#884](https://github.com/itsakeyfut/avio/issues/884)–[#895](https://github.com/itsakeyfut/avio/issues/895)) — no public API changes
- Moved `SubtitleError` from `subtitle/mod.rs` to `error.rs` ([#894](https://github.com/itsakeyfut/avio/issues/894))

---

## [0.9.0] - 2026-03-30

### Added

#### ff-format
- Subtitle format parser for SRT, ASS/SSA, and WebVTT ([#676](https://github.com/itsakeyfut/avio/issues/676))
- Subtitle format writer for SRT, ASS, and WebVTT ([#677](https://github.com/itsakeyfut/avio/issues/677))

#### ff-filter — color grading & image quality
- `lut3d` filter step for 3D LUT colour grading from `.cube`/`.3dl` files ([#237](https://github.com/itsakeyfut/avio/issues/237))
- `eq` filter step for brightness, contrast, and saturation adjustment ([#239](https://github.com/itsakeyfut/avio/issues/239))
- `curves` filter step for per-channel RGB tone curves ([#240](https://github.com/itsakeyfut/avio/issues/240))
- `white_balance` filter step for colour temperature correction ([#241](https://github.com/itsakeyfut/avio/issues/241))
- `hue` filter step for hue rotation ([#242](https://github.com/itsakeyfut/avio/issues/242))
- `gamma` filter step for per-channel gamma correction ([#243](https://github.com/itsakeyfut/avio/issues/243))
- `three_way_cc` filter step for lift/gamma/gain colour grading ([#244](https://github.com/itsakeyfut/avio/issues/244))
- `vignette` filter step for configurable radius and strength ([#245](https://github.com/itsakeyfut/avio/issues/245))
- `gblur` filter step for Gaussian blur ([#252](https://github.com/itsakeyfut/avio/issues/252))
- `unsharp` filter step for sharpening and blurring ([#253](https://github.com/itsakeyfut/avio/issues/253))
- `hqdn3d` filter step for temporal and spatial noise reduction ([#254](https://github.com/itsakeyfut/avio/issues/254))
- `nlmeans` filter step for non-local means noise reduction ([#255](https://github.com/itsakeyfut/avio/issues/255))

#### ff-filter — geometry & transforms
- `ScaleAlgorithm` enum for `scale` step: `Fast`, `Bilinear`, `Bicubic`, `Lanczos` ([#249](https://github.com/itsakeyfut/avio/issues/249))
- `hflip` and `vflip` filter steps ([#248](https://github.com/itsakeyfut/avio/issues/248))
- Configurable fill color for `rotate` step ([#247](https://github.com/itsakeyfut/avio/issues/247))
- Zero-dimension validation for `crop` step ([#246](https://github.com/itsakeyfut/avio/issues/246))
- `pad` filter step for letterbox/pillarbox with configurable color ([#250](https://github.com/itsakeyfut/avio/issues/250))
- `fit_to_aspect` filter step for automatic letterbox/pillarbox to target resolution ([#251](https://github.com/itsakeyfut/avio/issues/251))

#### ff-filter — transitions & temporal
- `yadif` deinterlacing filter step with `YadifMode` enum ([#256](https://github.com/itsakeyfut/avio/issues/256))
- `fade_in` / `fade_out` now accept a configurable `start_sec` parameter ([#257](https://github.com/itsakeyfut/avio/issues/257))
- `fade_in_white` / `fade_out_white` filter steps for white fade ([#258](https://github.com/itsakeyfut/avio/issues/258))
- `xfade` cross-dissolve transition step with `XfadeTransition` enum ([#259](https://github.com/itsakeyfut/avio/issues/259))
- `reverse` (video) and `areverse` (audio) playback reversal steps ([#267](https://github.com/itsakeyfut/avio/issues/267))
- `speed` filter step for playback speed change via `setpts` + `atempo` ([#266](https://github.com/itsakeyfut/avio/issues/266))
- `freeze_frame` step to hold a frame for a configurable duration ([#268](https://github.com/itsakeyfut/avio/issues/268))
- `concat_video` step for multi-clip video concatenation ([#279](https://github.com/itsakeyfut/avio/issues/279))
- `concat_audio` step for multi-clip audio concatenation ([#280](https://github.com/itsakeyfut/avio/issues/280))
- `join_with_dissolve` step for cross-dissolve clip transitions ([#282](https://github.com/itsakeyfut/avio/issues/282))

#### ff-filter — text & overlay
- `drawtext` filter step for text overlay with `DrawTextOptions` ([#260](https://github.com/itsakeyfut/avio/issues/260))
- `DrawTextOptions` background box support ([#261](https://github.com/itsakeyfut/avio/issues/261))
- `ticker` filter step for scrolling news-ticker text overlay ([#265](https://github.com/itsakeyfut/avio/issues/265))
- `subtitles_srt` filter step for hard SRT subtitle burn-in ([#262](https://github.com/itsakeyfut/avio/issues/262))
- `subtitles_ass` filter step for hard ASS/SSA subtitle burn-in ([#263](https://github.com/itsakeyfut/avio/issues/263))
- `overlay_image` filter step for PNG watermark compositing ([#264](https://github.com/itsakeyfut/avio/issues/264))

#### ff-filter — audio effects
- `afade_in` / `afade_out` audio fade steps ([#272](https://github.com/itsakeyfut/avio/issues/272))
- `loudness_normalize` step for EBU R128 two-pass loudness normalization ([#269](https://github.com/itsakeyfut/avio/issues/269))
- `normalize_peak` step for two-pass peak level normalization ([#270](https://github.com/itsakeyfut/avio/issues/270))
- `equalizer` replaced by multi-band `ParametricEq` with `EqBand` type ([#273](https://github.com/itsakeyfut/avio/issues/273))
- `agate` noise gate step with threshold, attack, and release ([#274](https://github.com/itsakeyfut/avio/issues/274))
- `compressor` dynamic range compressor step ([#275](https://github.com/itsakeyfut/avio/issues/275))
- `stereo_to_mono` downmix step ([#276](https://github.com/itsakeyfut/avio/issues/276))
- `channel_map` step for arbitrary audio channel remapping ([#277](https://github.com/itsakeyfut/avio/issues/277))
- `audio_delay` step for A/V sync correction via `adelay`/`atrim` ([#278](https://github.com/itsakeyfut/avio/issues/278))

#### ff-encode
- `StreamCopyTrimmer` for lossless clip trimming via stream copy ([#281](https://github.com/itsakeyfut/avio/issues/281))
- `EncodeError::InvalidDimensions` and `EncodeError::InvalidBitrate` error variants ([#284](https://github.com/itsakeyfut/avio/issues/284))
- Input validation for frame dimensions (2–32768 px), bitrate (≤800 Mbps), and fps (≤1000) ([#283](https://github.com/itsakeyfut/avio/issues/283))
- `EncodeError::InvalidChannelCount` and `EncodeError::InvalidSampleRate` with audio encoder validation ([#286](https://github.com/itsakeyfut/avio/issues/286))

#### ff-decode
- Corrupt stream recovery: skip `AVERROR_INVALIDDATA` packets with warn log ([#287](https://github.com/itsakeyfut/avio/issues/287))
- `DecodeError::StreamCorrupted` after 32 consecutive invalid packets ([#288](https://github.com/itsakeyfut/avio/issues/288))
- `DecodeError::UnsupportedResolution` when decoded frame dimensions exceed limits ([#285](https://github.com/itsakeyfut/avio/issues/285))

#### Security & reliability
- `cargo-fuzz` target for `VideoDecoder::open` with arbitrary bytes ([#289](https://github.com/itsakeyfut/avio/issues/289))
- `cargo-fuzz` target for `ff_probe::open` with arbitrary bytes ([#290](https://github.com/itsakeyfut/avio/issues/290))
- `cargo-fuzz` target for `VideoEncoder` with arbitrary frame data ([#291](https://github.com/itsakeyfut/avio/issues/291))
- CI fuzz job: 60 seconds per target on every PR ([#292](https://github.com/itsakeyfut/avio/issues/292))

#### Integration tests
- LUT application verified against reference image ([#293](https://github.com/itsakeyfut/avio/issues/293))
- Audio effects on reference sine wave ([#294](https://github.com/itsakeyfut/avio/issues/294))
- Full filter chain (colour grade + overlay + audio) ([#295](https://github.com/itsakeyfut/avio/issues/295))

#### avio — examples
- `video_effects.rs`: fade, rotate, tone mapping, xfade, speed, freeze
- `color_grade.rs`: LUT3D, eq, curves, white balance, hue, gamma, three-way CC
- `text_overlay.rs`: drawtext, ticker, subtitle burn-in
- `audio_filters.rs`: volume, parametric EQ, noise gate, compressor
- `clip_operations.rs`: concat, join with dissolve, reverse, StreamCopyTrimmer
- `deinterlace.rs`: yadif modes
- `noise_reduction.rs`: gblur, unsharp, hqdn3d, nlmeans

### Changed

#### ff-encode — breaking
- `Container` renamed to `OutputContainer` to avoid confusion with `ff_format::ContainerInfo` ([#716](https://github.com/itsakeyfut/avio/issues/716))

#### avio — breaking
- Re-export updated: `Container` → `OutputContainer` under the `encode` feature

---

## [0.8.0] - 2026-03-24

### Added

#### ff-format
- `NetworkOptions` struct: `user_agent`, `referer`, `headers`, `timeout_ms`, `reconnect_delay_ms`, `listen` for network-backed sources ([#219](https://github.com/itsakeyfut/avio/issues/219))

#### ff-decode — network input
- `VideoDecoder::open_url()` and `VideoDecoder::open()` now accept HTTP, HLS, DASH, RTMP, SRT, and UDP URLs via the existing builder API ([#220](https://github.com/itsakeyfut/avio/issues/220))
- `AudioDecoder` network URL support with typed errors ([#221](https://github.com/itsakeyfut/avio/issues/221))
- HLS/M3U8 input: `is_live()` detection and seek guard to prevent seeks on live streams ([#222](https://github.com/itsakeyfut/avio/issues/222))
- DASH/MPD input support with integration tests ([#223](https://github.com/itsakeyfut/avio/issues/223))
- UDP/MPEG-TS: buffer-size and FIFO-size options; documented live-stream behaviour ([#224](https://github.com/itsakeyfut/avio/issues/224))
- SRT protocol input behind `srt` feature flag; `DecodeError::ProtocolUnavailable` returned when `FFmpeg` lacks libsrt ([#225](https://github.com/itsakeyfut/avio/issues/225))
- Auto-reconnect with exponential backoff for live streams; `VideoDecoderBuilder::reconnect()` / `max_reconnect_attempts()` / `reconnect_delay_ms()` ([#226](https://github.com/itsakeyfut/avio/issues/226))

#### ff-encode
- fMP4/CMAF container: `Container::Fmp4` and `Container::Cmaf` variants for low-latency streaming output ([#678](https://github.com/itsakeyfut/avio/issues/678))

#### ff-stream — live output
- `LiveHlsOutput`: frame-push live HLS encoder; writes `index.m3u8` + `.ts` segments with sliding window ([#229](https://github.com/itsakeyfut/avio/issues/229))
- `LiveDashOutput`: frame-push live DASH encoder; writes `manifest.mpd` + `.m4s` segments ([#230](https://github.com/itsakeyfut/avio/issues/230))
- `RtmpOutput`: frame-push H.264/AAC encoder over RTMP/FLV ([#231](https://github.com/itsakeyfut/avio/issues/231))
- `SrtOutput`: frame-push H.264/AAC encoder over SRT/MPEG-TS behind `srt` feature flag; `StreamError::ProtocolUnavailable` returned when `FFmpeg` lacks libsrt ([#232](https://github.com/itsakeyfut/avio/issues/232))
- `FanoutOutput`: `StreamOutput` wrapper that fans each frame to multiple targets simultaneously; collects all errors into `StreamError::FanoutFailure` ([#233](https://github.com/itsakeyfut/avio/issues/233))
- `LiveAbrLadder`: multi-rendition ABR output; pushes each frame to N rendition encoders, writes per-rendition HLS playlists and a master playlist or DASH manifest ([#234](https://github.com/itsakeyfut/avio/issues/234))

#### avio — examples
- `decode_from_url.rs`: open and decode HTTP/HLS/RTMP/SRT URLs
- `live_hls_output.rs`: decode a local file and push frames to `LiveHlsOutput`
- `live_dash_output.rs`: decode a local file and push frames to `LiveDashOutput`
- `rtmp_output.rs`: decode and push to an RTMP ingest endpoint
- `fanout_output.rs`: fan frames to `LiveHlsOutput` + `LiveDashOutput` via `FanoutOutput`
- `live_abr_ladder.rs`: multi-rendition `LiveAbrLadder` with configurable ladder
- `srt_output.rs`: decode and push to an SRT endpoint (skips gracefully when libsrt absent)

### Fixed

#### ff-sys
- `docsrs_stubs`: add `open_input_url` stub, fixing docs.rs build for network-input additions
- `docsrs_stubs`: add `AVFormatContext.priv_data` field, fixing docs.rs build for `ff-stream`

### Tests

- HTTP URL integration tests with in-process server; skip gracefully when `FFmpeg` lacks the HTTP protocol ([#235](https://github.com/itsakeyfut/avio/issues/235))
- `LiveHlsOutput` integration test: synthetic frame push, playlist and segment assertions ([#236](https://github.com/itsakeyfut/avio/issues/236))
- `LiveDashOutput`, `FanoutOutput`, `RtmpOutput`, `SrtOutput` integration tests

### CI

- `docsrs-stubs` job (`DOCS_RS=1 cargo build --workspace`) added to catch missing `ff-sys` stub symbols before publishing

---

## [0.7.3] - 2026-03-22

### Fixed

#### ff-sys
- `docsrs_stubs`: add `AVFormatContext.priv_data` field, fixing the docs.rs build failure for `ff-stream` (used by `av_opt_set` calls in `hls_inner.rs` and `dash_inner.rs`)

### Changed

#### CI
- Add `docsrs-stubs` job (`DOCS_RS=1 cargo build --workspace`) to catch missing `ff-sys` stub symbols before publishing

---

## [0.7.2] - 2026-03-22

### Fixed

#### ff-sys
- `docsrs_stubs`: add `av_rescale_q`, `AVPictureType_AV_PICTURE_TYPE_NONE`, and `AVPictureType_AV_PICTURE_TYPE_I`, fixing the docs.rs build failure for `ff-stream`

---

## [0.7.1] - 2026-03-22

### Fixed

#### ff-sys
- `docsrs_stubs`: add all symbols referenced by v0.7.0 feature additions that were missing, causing docs.rs build failures for `ff-encode`, `ff-pipeline`, `ff-stream`, and `avio`:
  - `AVAudioFifo` opaque struct and `swresample::audio_fifo` module (`alloc`, `free`, `write`, `read`, `size`)
  - `AVCodec.sample_fmts` and `AVCodec.capabilities` fields (struct was previously opaque)
  - `AVFilterContext.hw_device_ctx` field (struct was previously opaque)
  - `AVCodecContext` fields: `frame_size`, `color_range`, `refs`, `rc_max_rate`, `rc_buffer_size`, `flags`, `stats_out`, `stats_in`
  - `AVPixelFormat_AV_PIX_FMT_YUVJ422P` and `AVPixelFormat_AV_PIX_FMT_YUVJ444P` pixel format constants

---

## [0.7.0] - 2026-03-22

### Added

#### ff-encode — per-codec video options
- `VideoCodecOptions` enum with codec-specific option structs applied via `av_opt_set` before `avcodec_open2` ([#190](https://github.com/itsakeyfut/avio/issues/190))
- `H264Options`: profile (`H264Profile`), level, B-frames, GOP size, reference frames ([#191](https://github.com/itsakeyfut/avio/issues/191))
- `H264Preset` and `H264Tune` enums; libx264 preset/tune applied via `av_opt_set` ([#192](https://github.com/itsakeyfut/avio/issues/192))
- `H265Options`: profile (`H265Profile`, `H265Tier`), level ([#193](https://github.com/itsakeyfut/avio/issues/193))
- `H265Options::preset` and `x265_params` passthrough ([#194](https://github.com/itsakeyfut/avio/issues/194))
- `Av1Options`: `cpu_used`, tile layout, `Av1Usage` mode ([#195](https://github.com/itsakeyfut/avio/issues/195))
- `VideoCodec::Av1Svt` and `SvtAv1Options`: SVT-AV1 encoder (`libsvtav1`) support with preset 0–13, tile layout, `svtav1_params` passthrough; requires `--enable-libsvtav1` ([#196](https://github.com/itsakeyfut/avio/issues/196))
- `Vp9Options`: `cpu_used`, `cq_level` constrained-quality mode, tile layout, `row_mt` ([#197](https://github.com/itsakeyfut/avio/issues/197))

#### ff-encode — per-codec audio options
- `AudioCodecOptions` enum with codec-specific option structs ([#198](https://github.com/itsakeyfut/avio/issues/198))
- `OpusOptions`: application mode (`OpusApplication`) and frame duration ([#199](https://github.com/itsakeyfut/avio/issues/199))
- `AacOptions`: profile (`AacProfile`: LC / HE / HEv2) and optional VBR quality ([#200](https://github.com/itsakeyfut/avio/issues/200))
- `Mp3Options` with `Mp3Quality` (VBR scale 0–9 or CBR bitrate) ([#201](https://github.com/itsakeyfut/avio/issues/201))
- `FlacOptions`: compression level 0–12 with validation ([#202](https://github.com/itsakeyfut/avio/issues/202))

#### ff-encode — professional video formats
- ProRes encoding: `VideoCodec::ProRes`, `ProResOptions`, `ProResProfile` (Proxy / Lt / Standard / Hq / P4444 / P4444Xq); pixel format auto-selected per profile ([#203](https://github.com/itsakeyfut/avio/issues/203))
- DNxHD/DNxHR encoding: `VideoCodec::DnxHd`, `DnxhdOptions`, `DnxhdVariant` covering all standard bitrate classes and DNxHR variants ([#204](https://github.com/itsakeyfut/avio/issues/204))

#### ff-encode / ff-format — HDR and high-bit-depth
- 10-bit pixel format encode and decode: `PixelFormat::Yuv420p10le`, `Yuv422p10le`, `Yuv444p10le`, `P010Le` ([#205](https://github.com/itsakeyfut/avio/issues/205))
- HDR10 static metadata: `Hdr10Metadata`, `MasteringDisplay` embedded as `AV_PKT_DATA_MASTERING_DISPLAY_METADATA` and `AV_PKT_DATA_CONTENT_LIGHT_LEVEL` side data on key-frame packets ([#206](https://github.com/itsakeyfut/avio/issues/206))
- HLG color transfer and color space tagging: `ColorTransfer::Hlg`, `ColorSpace::Bt2020`, `ColorPrimaries::Bt2020`; `.color_transfer()`, `.color_space()`, `.color_primaries()` setters on `VideoEncoderBuilder` ([#207](https://github.com/itsakeyfut/avio/issues/207))

#### ff-encode — container variants
- MKV binary attachment muxing: `VideoEncoderBuilder::add_attachment()` embeds arbitrary binary blobs (ICC profiles, fonts, thumbnails) as MKV file attachments ([#208](https://github.com/itsakeyfut/avio/issues/208))
- WebM container codec enforcement: `Container::Webm` restricts video to VP8/VP9/AV1 and audio to Vorbis/Opus; auto-defaults applied when codec is unset ([#209](https://github.com/itsakeyfut/avio/issues/209))
- AVI and MOV container enforcement: `Container::Avi` and `Container::Mov` with codec allow-lists; `AudioCodec::Pcm16` (`pcm_s16le`) and `AudioCodec::Pcm24` (`pcm_s24le`) added ([#210](https://github.com/itsakeyfut/avio/issues/210))
- FLAC and OGG standalone audio containers: `Container::Flac` and `Container::Ogg` for `AudioEncoder` ([#211](https://github.com/itsakeyfut/avio/issues/211))

#### ff-decode — image sequences and OpenEXR
- Image sequence decode via `image2` demuxer: `%`-pattern paths (e.g. `frame%04d.png`) auto-select the demuxer; `VideoDecoderBuilder::frame_rate()` setter overrides the default 25 fps ([#212](https://github.com/itsakeyfut/avio/issues/212))
- OpenEXR sequence decode: EXR files decode as `gbrpf32le` (32-bit float, G/B/R plane order); requires `--enable-decoder=exr` in the FFmpeg build; returns `DecodeError::DecoderUnavailable` when absent ([#214](https://github.com/itsakeyfut/avio/issues/214))

#### ff-encode — image sequence encode
- Image sequence encode via `image2` muxer: `%`-pattern output paths produce numbered still-image files (PNG, JPEG, BMP, TIFF) ([#213](https://github.com/itsakeyfut/avio/issues/213))

#### ff-decode / ff-format — decoder ergonomics
- `ContainerInfo` exposed on `VideoDecoder` and `AudioDecoder`: `container_info()` returns format name, bit rate, and `duration_opt`; `duration_opt()` shorthand ([#619](https://github.com/itsakeyfut/avio/issues/619))
- `Timestamp::invalid()` and `Timestamp::is_valid()` for distinguishing missing/unknown timestamps ([#618](https://github.com/itsakeyfut/avio/issues/618))
- `VideoDecoder` and `AudioDecoder` implement `Iterator<Item = Result<Frame, DecodeError>>` and `FusedIterator` ([#616](https://github.com/itsakeyfut/avio/issues/616))
- `VideoDecoderBuilder::output_size(width, height)` / `output_width()` / `output_height()`: built-in scale + pixel-format conversion via `swscale` in one pass ([#629](https://github.com/itsakeyfut/avio/issues/629))
- `AudioFrame` PCM conversion methods: `to_i16_interleaved()`, `to_f32_interleaved()`, `to_f64_interleaved()` ([#609](https://github.com/itsakeyfut/avio/issues/609))

#### avio — feature flags
- `gpl` feature flag forwarding (`ff-encode/gpl`): enables libx264 and libx265; disabled by default
- `hwaccel` feature flag forwarding (`ff-encode/hwaccel`): enables hardware encoder detection (NVENC, QSV, AMF, VideoToolbox, VA-API); enabled by default

#### Examples (v0.7.0)
- `codec_options`: `VideoCodecOptions` with H264/H265/AV1/SVT-AV1/VP9 options
- `audio_codec_options`: `AudioCodecOptions` with Opus/AAC/MP3/FLAC; `Container::Ogg` and `Container::Flac`
- `professional_formats`: ProRes HQ and DNxHR SQ round-trip encode
- `hdr10_encode`: HDR10 static metadata in MKV with `Hdr10Metadata` and `MasteringDisplay`
- `image_sequence`: PNG sequence decode and encode via `%`-pattern paths
- `openexr_sequence`: OpenEXR sequence decode with `gbrpf32le` plane access
- `hwaccel_encode`: `HardwareEncoder::available()`, `is_available()`, `actual_video_codec()`, `is_lgpl_compliant()`
- `gpl_encode`: GPL/LGPL encoder selection with and without the `gpl` feature; `VideoCodecEncodeExt::is_lgpl_compatible()`

### Changed

#### ff-format
- `AudioFrame::data()` return type changed from `Option<&[u8]>` to `&[u8]`; returns an empty slice when no data is present ([#612](https://github.com/itsakeyfut/avio/issues/612))

#### ff-decode
- `DecodeError::EndOfStream` removed; end-of-stream is now signalled uniformly as `Ok(None)` from `decode_one()` ([#624](https://github.com/itsakeyfut/avio/issues/624))

### Fixed

#### ff-encode
- `VideoCodecEncodeExt::is_lgpl_compatible()` now correctly returns `true` for `VideoCodec::Av1Svt` (libsvtav1 is BSD-3-Clause)

#### ff-sys
- `docsrs_stubs` module: added missing symbols (`avformat_find_input_format`, `av_dict_free`, `open_input_image_sequence`) that caused `docs.rs` build failures

---

## [0.6.0] - 2026-03-18

### Added

#### ff-decode — async API (tokio feature)
- `tokio` feature flag: opt-in async support that does not affect the default sync API ([#176](https://github.com/itsakeyfut/avio/issues/176))
- `AsyncVideoDecoder`: async wrapper around `VideoDecoder` backed by `tokio::task::spawn_blocking`; exposes `open()`, `decode_frame()`, and `into_stream()` returning `impl Stream<Item = Result<VideoFrame, DecodeError>> + Send` ([#177](https://github.com/itsakeyfut/avio/issues/177), [#178](https://github.com/itsakeyfut/avio/issues/178))
- `AsyncAudioDecoder`: async wrapper around `AudioDecoder` with the same three-method API ([#179](https://github.com/itsakeyfut/avio/issues/179))
- `AsyncImageDecoder`: async wrapper around `ImageDecoder`; exposes `open()` and `decode()` (no stream — single-frame by design)
- Compile-time `Send` bounds asserted for all async decoder types ([#180](https://github.com/itsakeyfut/avio/issues/180))
- Integration tests: async video decode frame count matches sync; async audio decode sample count matches sync ([#185](https://github.com/itsakeyfut/avio/issues/185), [#186](https://github.com/itsakeyfut/avio/issues/186))
- CI job `no-tokio-feature`: builds, tests, and lints the workspace with `--no-default-features` to verify the sync API is unaffected ([#188](https://github.com/itsakeyfut/avio/issues/188))

#### ff-encode — async API (tokio feature)
- `tokio` feature flag in `ff-encode` ([#181](https://github.com/itsakeyfut/avio/issues/181))
- `AsyncVideoEncoder`: frames are queued into a bounded `tokio::sync::mpsc` channel (capacity 8) consumed by a dedicated worker thread; `push().await` suspends automatically when the channel is full (back-pressure); `finish().await` sends a `WorkerMsg::Finish` sentinel, drops the sender, and joins the worker via `spawn_blocking` ([#182](https://github.com/itsakeyfut/avio/issues/182), [#184](https://github.com/itsakeyfut/avio/issues/184))
- `AsyncAudioEncoder`: identical pattern for audio frames ([#183](https://github.com/itsakeyfut/avio/issues/183))
- Integration tests: async video/audio encode produces valid output; back-pressure test ([#187](https://github.com/itsakeyfut/avio/issues/187))

#### avio — tokio feature
- `tokio` feature flag forwarding to `ff-decode/tokio` and `ff-encode/tokio`; re-exports all five async types (`AsyncVideoDecoder`, `AsyncAudioDecoder`, `AsyncImageDecoder`, `AsyncVideoEncoder`, `AsyncAudioEncoder`) under `#[cfg(feature = "tokio")]` ([#593](https://github.com/itsakeyfut/avio/issues/593))

#### Examples (async)
- `async_decode_video`: frame-by-frame, `into_stream()` combinators, and `tokio::spawn` patterns ([#594](https://github.com/itsakeyfut/avio/issues/594))
- `async_decode_audio`: same three patterns for audio, including sample counting via `fold` ([#595](https://github.com/itsakeyfut/avio/issues/595))
- `async_decode_image`: single decode and parallel multi-image decode via `futures::future::join_all` ([#596](https://github.com/itsakeyfut/avio/issues/596))
- `async_encode_video`: basic async encode loop and producer/consumer pattern with `mpsc` channel ([#597](https://github.com/itsakeyfut/avio/issues/597))
- `async_encode_audio`: same two patterns for audio, with real-time audio source motivation ([#598](https://github.com/itsakeyfut/avio/issues/598))
- `async_transcode`: end-to-end `AsyncVideoDecoder` → `AsyncVideoEncoder` in sequential and concurrent flavours; flagship async example ([#599](https://github.com/itsakeyfut/avio/issues/599))

### Changed

#### avio — documentation
- `## Encode` section in crate-level docs expanded into a three-part decision guide: when to use `Pipeline`, `VideoEncoder`/`AudioEncoder` directly, or the async encoders; each section includes bullet criteria, a code snippet, and cross-references to relevant examples

---

## [0.5.0] - 2026-03-16

### Added

#### ff-stream (new crate)
- `StreamError` enum with `InvalidConfig`, `Io`, `Encode`, and `Ffmpeg { code, message }` variants ([#69](https://github.com/itsakeyfut/avio/issues/69))
- `HlsOutput` consuming builder: configures and writes an HLS segmented stream via the FFmpeg HLS muxer, producing `playlist.m3u8` and numbered `.ts` segments ([#70](https://github.com/itsakeyfut/avio/issues/70), [#71](https://github.com/itsakeyfut/avio/issues/71))
- `DashOutput` consuming builder: configures and writes a DASH segmented stream via the FFmpeg DASH muxer, producing `manifest.mpd` and `.m4s` segments ([#72](https://github.com/itsakeyfut/avio/issues/72), [#73](https://github.com/itsakeyfut/avio/issues/73))
- `Rendition` struct describing a single resolution/bitrate quality level ([#74](https://github.com/itsakeyfut/avio/issues/74))
- `AbrLadder` builder: multi-rendition HLS output via `hls()` (produces master playlist + per-rendition subdirectories) and multi-representation DASH output via `dash()` (single `manifest.mpd` with multiple `Representation` elements) ([#75](https://github.com/itsakeyfut/avio/issues/75), [#76](https://github.com/itsakeyfut/avio/issues/76), [#77](https://github.com/itsakeyfut/avio/issues/77))
- Integration tests: `HlsOutput::write()` verifying playlist tags, segment naming, and target duration ([#84](https://github.com/itsakeyfut/avio/issues/84))
- Integration tests: `AbrLadder::hls()` verifying master playlist, per-rendition playlists, and `.ts` segments; `AbrLadder::dash()` verifying single-manifest DASH output ([#85](https://github.com/itsakeyfut/avio/issues/85))

#### avio (new crate)
- Facade crate that re-exports the public APIs of all `ff-*` crates behind feature flags (`probe`, `decode`, `encode`, `filter`, `pipeline`, `stream`) ([#78](https://github.com/itsakeyfut/avio/issues/78)–[#82](https://github.com/itsakeyfut/avio/issues/82))
- Integration tests verifying the facade compiles and resolves symbols for each feature combination ([#86](https://github.com/itsakeyfut/avio/issues/86))

#### ff-common
- `VecPool`: canonical `Arc`-based frame buffer pool backed by a `Mutex<Vec<Vec<u8>>>`; exposes `capacity()` and `available()` ([#502](https://github.com/itsakeyfut/avio/issues/502))
- `SimpleFramePool` type alias for `VecPool` for backwards compatibility

#### ff-format
- `DnxHd` variant added to `VideoCodec` ([#497](https://github.com/itsakeyfut/avio/issues/497))

### Changed

#### ff-encode
- `Progress` renamed to `EncodeProgress`; `ProgressCallback` renamed to `EncodeProgressCallback` to avoid collision with `ff-pipeline`'s `Progress`/`ProgressCallback` ([#500](https://github.com/itsakeyfut/avio/issues/500))
- `VideoCodec` and `AudioCodec` are no longer defined in `ff-encode`; they are re-exported from `ff-format` ([#498](https://github.com/itsakeyfut/avio/issues/498))

#### ff-pipeline
- `EncoderConfig` fields `video_codec` and `audio_codec` now use the canonical `ff-format` types ([#499](https://github.com/itsakeyfut/avio/issues/499))

#### ff-decode
- `SimpleFramePool` moved from `ff-decode` to `ff-common`; `ff-decode` re-exports it for backwards compatibility ([#502](https://github.com/itsakeyfut/avio/issues/502))

### Fixed

#### All crates
- `Ffmpeg` error variant unified to `Ffmpeg { code: i32, message: String }` (struct variant) across `ff-probe`, `ff-decode`, `ff-encode`, `ff-filter`, `ff-pipeline`, and `ff-stream` ([#486](https://github.com/itsakeyfut/avio/issues/486))

---

## [0.4.0] - 2026-03-16

### Added

#### ff-pipeline
- `Pipeline::run()` for single-input transcode: decode → optional filter graph → encode with progress tracking ([#58](https://github.com/itsakeyfut/avio/issues/58))
- Multi-input concatenation in `Pipeline::run()`: sequential inputs with PTS offset stitching ([#59](https://github.com/itsakeyfut/avio/issues/59))
- Audio stream handling in `Pipeline::run()`: audio decoded and encoded in parallel with video; silently skipped when no audio stream is present ([#60](https://github.com/itsakeyfut/avio/issues/60))
- Cancellation via `ProgressCallback`: returning `false` from the callback aborts the pipeline and returns `PipelineError::Cancelled` ([#61](https://github.com/itsakeyfut/avio/issues/61))
- `ThumbnailPipeline`: extracts a `VideoFrame` at each caller-specified timestamp using `VideoDecoder::seek` + `decode_one` ([#62](https://github.com/itsakeyfut/avio/issues/62))
- `parallel` Cargo feature for `ThumbnailPipeline`: when enabled, each timestamp is decoded in its own rayon thread; results are returned in ascending timestamp order ([#63](https://github.com/itsakeyfut/avio/issues/63))
- Integration tests: single-input transcode verifying non-zero output duration and progress callback invocation ([#64](https://github.com/itsakeyfut/avio/issues/64))
- Integration tests: multi-input concatenation verifying output duration ≈ sum of input durations ([#65](https://github.com/itsakeyfut/avio/issues/65))
- Integration tests: cancellation after first progress callback ([#66](https://github.com/itsakeyfut/avio/issues/66))
- Integration tests: `ThumbnailPipeline` frame count and dimension verification, sequential and parallel ([#67](https://github.com/itsakeyfut/avio/issues/67))

---

## [0.3.0] - 2026-03-14

### Added

#### ff-encode
- `ImageEncoder`: encodes a single `VideoFrame` to JPEG, PNG, or BMP using the FFmpeg image2 muxer ([#152](https://github.com/itsakeyfut/avio/issues/152))
- `ImageEncoderBuilder` options: `width`, `height`, `quality`, and `pixel_format` ([#153](https://github.com/itsakeyfut/avio/issues/153))
- `ImageEncoderInner` with RAII `Drop` for safe cleanup of FFmpeg resources ([#154](https://github.com/itsakeyfut/avio/issues/154))
- `BitrateMode` enum (`Cbr` / `Vbr`) for video bitrate control; `Vbr` wired to `AVCodecContext` ([#44](https://github.com/itsakeyfut/avio/issues/44), [#46](https://github.com/itsakeyfut/avio/issues/46))
- Two-pass video encoding via `VideoEncoderBuilder::two_pass()` ([#43](https://github.com/itsakeyfut/avio/issues/43))
- Metadata write support via `VideoEncoderBuilder::metadata()` ([#45](https://github.com/itsakeyfut/avio/issues/45))
- Chapter write support via `VideoEncoderBuilder::chapters()` ([#47](https://github.com/itsakeyfut/avio/issues/47))
- Subtitle passthrough via `VideoEncoderBuilder::subtitle_passthrough()` ([#48](https://github.com/itsakeyfut/avio/issues/48))
- Integration tests: `ImageEncoder` file creation, round-trip decode, quality, and drop-without-encode ([#155](https://github.com/itsakeyfut/avio/issues/155))
- Integration tests: chapter round-trip ([#52](https://github.com/itsakeyfut/avio/issues/52)) and subtitle passthrough round-trip ([#53](https://github.com/itsakeyfut/avio/issues/53))

#### ff-probe
- `SubtitleCodec` enum covering common subtitle codecs (ASS, SRT, WebVTT, HDMV PGS, DVB, MOV text, TTML) ([#49](https://github.com/itsakeyfut/avio/issues/49))
- `SubtitleStreamInfo` struct with `codec`, `language`, `title`, and `default_stream` accessors ([#50](https://github.com/itsakeyfut/avio/issues/50))
- `subtitle_streams()`, `has_subtitles()`, and `subtitle_stream_count()` on `MediaInfo` ([#51](https://github.com/itsakeyfut/avio/issues/51))
- Integration tests for subtitle stream probing

### Changed

#### ff-encode
- Module structure restructured to match the `ff-decode` pattern (`video/`, `audio/`, `image/`) ([#151](https://github.com/itsakeyfut/avio/issues/151))

### Fixed

#### ff-encode
- Subtitle packets with `AV_NOPTS_VALUE` DTS now have DTS mirrored from PTS before `av_interleaved_write_frame`; prevents silent packet drops by the Matroska muxer

---

## [0.2.0] - 2026-03-14

### Added

#### ff-filter (new crate)
- Full filter graph implementation: `FilterGraph`, `FilterGraphBuilder`, `FilterGraphInner`, and `FilterError` types ([#15](https://github.com/itsakeyfut/avio/issues/15)–[#20](https://github.com/itsakeyfut/avio/issues/20))
- `AVFilterGraph` lifecycle management with RAII `Drop` ([#16](https://github.com/itsakeyfut/avio/issues/16))
- `buffersrc` / `buffersink` argument helpers for both video and audio ([#21](https://github.com/itsakeyfut/avio/issues/21))
- Multi-input slot management for video and audio filter chains ([#22](https://github.com/itsakeyfut/avio/issues/22))
- Graph validation via `avfilter_graph_config` with propagated error codes and messages ([#23](https://github.com/itsakeyfut/avio/issues/23))
- `push_video` / `pull_video` and `push_audio` / `pull_audio` API with correct PTS/DTS timestamp handling ([#18](https://github.com/itsakeyfut/avio/issues/18), [#19](https://github.com/itsakeyfut/avio/issues/19))
- Filter name validation at `build()` time ([#17](https://github.com/itsakeyfut/avio/issues/17))
- Video filters: `scale`, `crop`, `trim` (with `setpts=PTS-STARTPTS` timestamp reset), `overlay`, `fade_in`, `fade_out`, `rotate`, `tone_map` ([#24](https://github.com/itsakeyfut/avio/issues/24)–[#30](https://github.com/itsakeyfut/avio/issues/30))
- Audio filters: `volume`, `amix`, `equalizer` ([#31](https://github.com/itsakeyfut/avio/issues/31)–[#33](https://github.com/itsakeyfut/avio/issues/33))
- Hardware-accelerated filter chains: CUDA (`hwupload_cuda` / `hwdownload`), VideoToolbox, VAAPI ([#34](https://github.com/itsakeyfut/avio/issues/34)–[#36](https://github.com/itsakeyfut/avio/issues/36))
- Integration tests covering all single-stream, multi-stream, and hardware filter paths ([#40](https://github.com/itsakeyfut/avio/issues/40), [#41](https://github.com/itsakeyfut/avio/issues/41))

#### ff-decode
- `ImageDecoder`: decodes still images (JPEG, PNG, BMP, TIFF, WebP) into `VideoFrame` using FFmpeg ([#37](https://github.com/itsakeyfut/avio/issues/37)–[#39](https://github.com/itsakeyfut/avio/issues/39))
- Builder pattern: `ImageDecoder::open(path)` → `ImageDecoderBuilder` → `ImageDecoder` ([#38](https://github.com/itsakeyfut/avio/issues/38))
- `decode()` (consuming), `decode_one()` (incremental), and `frames()` (iterator) APIs ([#39](https://github.com/itsakeyfut/avio/issues/39))
- `ImageFrameIterator` for API consistency with `VideoFrameIterator` and `AudioFrameIterator`
- `ImageDecoder`, `ImageDecoderBuilder`, and `ImageFrameIterator` re-exported from crate root and `prelude`
- Integration tests covering JPEG and PNG decode (width, height, pixel format) and error handling ([#42](https://github.com/itsakeyfut/avio/issues/42))

### Fixed

#### ff-filter
- `AV_BUFFERSRC_FLAG_KEEP_REF` type normalized across platforms to prevent Windows/Linux build divergence
- Null `avfilter_get_by_name` return treated as unverifiable (skipped) rather than a build error at startup

#### ff-sys
- `docsrs_stubs.rs` completed for `ff-probe`, `ff-decode`, and `ff-encode` crates ([#127](https://github.com/itsakeyfut/avio/issues/127))

---

## [0.1.3] - 2026-03-13

### Fixed

#### ff-sys
- docs.rs builds now succeed for all crates. `build.rs` detects the `DOCS_RS`
  environment variable and writes empty bindings, emitting `cfg(docsrs)`.
  A new `docsrs_stubs.rs` file provides shape-compatible stub types, constants,
  functions, and wrapper modules so that `ff-probe`, `ff-decode`, and `ff-encode`
  compile on docs.rs without any changes to those crates ([#125](https://github.com/itsakeyfut/avio/pull/125))
- All crates now carry `[package.metadata.docs.rs]` with
  `rustdoc-args = ["--cfg", "docsrs"]` ([#125](https://github.com/itsakeyfut/avio/pull/125))

---

## [0.1.2] - 2026-03-12

### Added

#### ff-format
- New `ChapterInfo` and `ChapterInfoBuilder` types for representing chapter markers within a media container (MKV, MP4, M4A, etc.). Fields: `id`, `title`, `start`, `end`, `time_base`, `metadata` ([#12](https://github.com/itsakeyfut/avio/pull/123))
- `MediaInfo` now exposes `chapters()`, `has_chapters()`, and `chapter_count()` accessors, plus corresponding builder methods `.chapter()` / `.chapters()` ([#13](https://github.com/itsakeyfut/avio/pull/123))

#### ff-probe
- Chapter extraction from `AVFormatContext`: iterates `nb_chapters`, converts each `AVChapter` (including `AVRational`-based timestamps) into `ChapterInfo` and populates `MediaInfo::chapters()` ([#14](https://github.com/itsakeyfut/avio/pull/123))
- Re-exports `ChapterInfo` and `ChapterInfoBuilder` from the public API and prelude

### Fixed

#### ff-encode
- `av_opt_set` return values for CRF and preset options are now checked; a `log::warn!` is emitted and the encode continues with the encoder default when an option is unsupported ([#10](https://github.com/itsakeyfut/avio/pull/122))

#### ff-decode
- Channel layout detection now reads `AVChannelLayout.u.mask` when `order == AV_CHANNEL_ORDER_NATIVE`, correctly distinguishing layouts that share a channel count (e.g. `Stereo2_1` vs `Surround3_0`). Falls back to channel-count heuristic with a `log::warn!` for unrecognised masks or non-native order ([#11](https://github.com/itsakeyfut/avio/pull/119))

### Changed

#### ff-decode, ff-encode, ff-probe, ff-format
- Silent fallbacks that previously discarded unsupported values without notice now emit `log::warn!` with `key=value` diagnostics ([#5–#9](https://github.com/itsakeyfut/avio/pull/116))

---

## [0.1.1] - 2026-03-11

### Fixed

#### ff-sys
- Raise minimum required FFmpeg version from 6.0 to 7.0. FFmpeg 7.x converted
  scaling flags from `#define` macros to `enum SwsFlags`, making 6.x
  incompatible with the bindgen-generated `SwsFlags_SWS_*` constants ([#1](https://github.com/itsakeyfut/avio/pull/1))

---

## [0.1.0] - 2026-03-11

### Added

#### ff-sys
- Raw FFmpeg FFI bindings via bindgen
- Platform-specific build support: pkg-config (Linux), Homebrew (macOS), vcpkg (Windows)

#### ff-common
- Shared error types and common utilities

#### ff-format
- Container demux and mux support

#### ff-probe
- Media metadata extraction (codec, resolution, duration, bitrate, streams)

#### ff-decode
- Video and audio decoding
- Hardware acceleration support (NVENC, QSV, VideoToolbox, VAAPI)

#### ff-encode
- Video and audio encoding
- Hardware acceleration support
- Thumbnail generation

#### ff-filter
- Placeholder crate (filter graph not yet implemented)
