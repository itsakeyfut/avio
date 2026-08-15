# Logging

> avio uses the **`log`** crate; the backend is chosen by the consumer. Related: [rust.md](./rust.md).

## Principles

- Use only the `log` crate (v0.4). Do not initialize a backend (`env_logger`, etc.) inside the
  library — that is the consuming application's job.
- Prefer a structured message format that includes `key=value` pairs.
- Keep per-frame / hot-path work out of `info!` / `debug!` (see "Hot paths" below).

---

## Choosing a log level

### `log::error!`

Fatal errors where the process cannot reasonably continue. Generally not used in library code;
return `Result::Err` and leave the decision to the caller.

### `log::warn!`

- When processing continues but a default value was used as a fallback.
- When an implicit conversion happens, e.g. converting to an unsupported pixel format.

```rust
log::warn!(
    "pixel_format unsupported, falling back to yuv420p \
     requested={:?} fallback=yuv420p",
    requested_fmt
);
```

### `log::info!`

Lifecycle events: successful codec open, hardware-acceleration initialization, pipeline start/finish.

```rust
log::info!("codec opened codec={} width={} height={}", codec_name, width, height);
```

### `log::debug!`

- Immediately before/after an FFmpeg API call (arguments and return values).
- When adding a filter to a filter graph.

```rust
log::debug!("filter added name={} args={}", filter_name, filter_args);
```

Do not log per frame here (the volume becomes enormous).

### `log::trace!`

Not used.

---

## Hot paths: validate at the boundary, do not log per frame

The decode / filter / encode loop runs once per frame. Emitting `warn!` / `error!` from inside it
on a "should not happen" condition (a missing stream, an unmapped token) floods the log and hurts
throughput. Observe such conditions in three layers instead:

1. **Validate at the boundary and reject with `Result`.** Check when data enters (open / parse /
   `build`), and return a typed error to the caller. Keep the hot path a total function that
   assumes validated input.
2. **Debug-visible fallback.** If something still slips through, the hot path returns an obvious
   default rather than panicking (do not poison the output).
3. **Do not log** (at most one `trace!` line).

```rust
// Good: validate on build, keep the per-frame path total and quiet
let mut graph = builder.build()?;   // boundary: invalid args rejected here
let frame = graph.pull_video()?;    // per-frame: no warn!, no error!
```

Non-hot-path fallbacks (open / init / hardware recovery) may still `warn!` as usual.

---

## Message format

**Prose + `key=value` pairs.** Key names are snake_case; values are printed as-is (no quotes).

```
// Good
"codec opened codec=h264 width=1920 height=1080 fps=30"
"filter added name=scale args=w=1280:h=720"

// Bad (no key=value)
"Codec opened successfully"
"Failed to open file"
```

---

## Do not

- Do not use `println!` / `eprintln!` in library code.
- Do not initialize a logging backend inside the library (that is the app's / test's job).
- Do not swallow an error and log nothing. If you ignore one intentionally, emit `log::warn!`.
- Do not emit heavy per-frame logs (`info!` / `debug!` inside the frame loop). Keep those to
  `trace!`, or prefer boundary validation (above).
