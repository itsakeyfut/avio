# v0.16.0 (minor feature) — 音MAD Support (Pitch Shift & BPM Detection)

> This is a **minor feature** included in the v0.16.0 release. The release's headline theme is
> the engine / library split (see `ROADMAP.md`); this feature is additive and already implemented,
> and ships as part of the same cut.

**Goal**: Extend audio processing capabilities to cover the key primitives required for Oto-MAD production on Niconico Douga: a wider pitch-shift range for mapping voice clips to musical notes across two octaves, and a BPM/beat detector that returns per-beat timestamps suitable for direct placement of clips on a `Timeline`.

**Prerequisite**: v0.15.0 complete.

**Crates in scope**: `ff-filter`, `ff-decode`, `avio`

---

## Requirements

### Pitch Shift — ±24 Semitone Range

- `FilterGraph::pitch_shift(semitones)` accepts values in `[-24.0, 24.0]` (±2 octaves).
- Pitch shifting is performed without changing playback duration (pitch-only, not tape-speed).
- The implementation chains multiple `atempo` filter instances when the compensation factor falls outside the single-instance range `[0.5, 2.0]`, reusing the same `add_atempo_chain` helper used by `TimeStretch`.
- Values outside `[-24.0, 24.0]` return `Err(FilterError::Ffmpeg { .. })`.
- Audio quality is comparable to the existing ±12 semitone range (WSOLA via `atempo`).

### BPM Detection and Beat Timestamps

- A `BpmDetector` struct detects the tempo and per-beat timestamps from an audio file.
- The public API follows the consuming-builder pattern used by `SilenceDetector`:

  ```rust
  BpmDetector::new("track.mp3")
      .bpm_range(60.0, 200.0)
      .run()  →  Result<BpmResult, DecodeError>
  ```

- `BpmResult` contains:
  - `bpm: f64` — detected tempo in beats per minute
  - `beats: Vec<Duration>` — timestamp of each detected beat from the start of the file
  - `confidence: f32` — detection confidence in `[0.0, 1.0]`; values below `0.4` indicate ambiguous rhythm

- The algorithm is pure Rust (no additional C dependencies):
  1. Decode the audio file to mono f32 PCM at 22 050 Hz via the existing FFmpeg decode path
  2. Detect onsets using **spectral flux** (half-wave rectified frame-energy derivative with adaptive threshold)
  3. Estimate BPM via **normalized autocorrelation** of the onset envelope, searching within `[bpm_min, bpm_max]`
  4. Generate beat timestamps by stepping forward from the first onset at the estimated interval, snapping to nearby onset peaks

- If the pure-Rust implementation does not meet the ±2 BPM accuracy threshold established by the integration tests, the algorithm internals (`detect_onsets`, `estimate_bpm`) are replaced with `aubio` C library bindings. The public API (`BpmDetector` / `BpmResult`) remains unchanged.

- `BpmDetector` and `BpmResult` are re-exported from `avio` under the `decode` feature flag.

- Primary use case: map detected beat timestamps to `Clip::timeline_offset` values for beat-synchronized video cutting.

---

## Design Decisions

| Topic | Decision |
|---|---|
| Pitch shift range | ±24 semitones (2 octaves); covers typical Oto-MAD note range without needing external quality libraries |
| Pitch shift implementation | Extend existing `asetrate` + `add_atempo_chain` path; no rubberband dependency |
| BPM algorithm | Pure Rust spectral flux + autocorrelation; no C dependency beyond FFmpeg |
| BPM fallback | If accuracy < ±2 BPM on reference track, migrate internals to aubio (API unchanged) |
| BPM output granularity | `bpm` + `Vec<Duration>` beats + `confidence`; beat array enables direct `Timeline` placement |
| Analysis sample rate | 22 050 Hz mono f32; sufficient for onset detection, reduces memory and CPU vs 44 100 Hz |

---

## Definition of Done

- `pitch_shift(24.0)` and `pitch_shift(-24.0)` succeed; `pitch_shift(24.5)` returns `Err`
- Integration test: `pitch_shift(+24.0)` on a 220 Hz sine wave produces output frequency within ±5% of 880 Hz
- `BpmDetector` returns BPM within ±2 on a reference 120 BPM click-track fixture
- `BpmDetector::run()` returns `Err(DecodeError::BpmDetectionFailed { .. })` for a missing file
- `avio::BpmDetector` and `avio::BpmResult` are accessible under the `decode` feature flag
- `cargo clippy --workspace -- -D warnings` clean
- `cargo test --workspace` passes
