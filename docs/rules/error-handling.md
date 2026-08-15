# Error Handling

> Related: [rust.md](./rust.md) (summary of the error policy). This is the detailed reference.

## Principles

- Implement error types with **`thiserror`** in every library crate.
- **Do not use `anyhow` in library crates.** A consuming application (an editor built on avio, a
  CLI tool) may use `anyhow` for contextual propagation.
- Separate **recoverable failures (`Result`)** from **invariant violations (panic)**.

---

## Result or panic

- **`Result` (recoverable)**: opening files, decode / encode / probe failures, any FFmpeg call
  error, unsupported formats, network I/O.
- **panic (invariant violation)**: a "cannot happen by construction" state. Even then, do not panic
  on a path a caller drives frame-by-frame (the decode / filter / encode loop); return `Result` and
  let the caller decide.
- `unwrap()` / `expect()` are forbidden in library code and allowed only under `#[cfg(test)]`. The
  one exception is a true, construction-guaranteed invariant, using `expect` with an
  `// INVARIANT:` comment stating why it always holds (the same discipline as `// SAFETY:`; do not
  abuse it).

```rust
// Bad — forbidden in library code
let stream = info.primary_video().unwrap();

// Good — propagate with ?
let stream = info.primary_video().ok_or(ProbeError::NoVideoStream)?;

// OK — a true invariant only, with a reason
// INVARIANT: pushed just above, so the last element always exists.
let last = self.steps.last_mut().expect("step just pushed");
```

---

## Representing FFmpeg errors

The negative integer error codes FFmpeg returns are kept in this form. Convert the code into a
message with `av_strerror` in the inner layer (`*_inner.rs`).

```rust
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg { code: i32, message: String },
    // ...
}
```

---

## Nesting errors

### Wrap a lower crate's error with `#[from]`

```rust
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("decode failed: {0}")]
    Decode(#[from] ff_decode::DecodeError),

    #[error("encode failed: {0}")]
    Encode(#[from] ff_encode::EncodeError),
}
```

### Carry context in fields

To make it clear where a failure happened, put information such as file paths and stream indices
in the variant's fields.

```rust
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("failed to open file: path={path}")]
    OpenFailed { path: String },

    #[error("stream not found: index={index}")]
    StreamNotFound { index: usize },

    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg { code: i32, message: String },
}
```

---

## Exposing error types

- Each crate re-exports its own error type from the crate root with `pub use`.
- The `avio` facade re-exports every crate's error type (e.g. `avio::DecodeError`,
  `avio::FilterError`).

---

## Do not

- Do not use `unwrap()` / `expect()` in library code (tests, and a true `// INVARIANT:` `expect`,
  excepted).
- Do not silently swallow errors. If you must ignore one, emit `log::warn!()`
  (see [logging.md](./logging.md)).
- Do not return string-only errors (`"something went wrong"`). Use a meaningful variant.
