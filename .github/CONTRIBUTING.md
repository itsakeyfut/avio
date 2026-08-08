# Contributing to avio

Thank you for your interest in contributing! No contribution is too small — bug reports,
documentation improvements, and typo fixes are all equally welcome.

If you're unsure where to start, feel free to open an issue and ask.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Prerequisites](#prerequisites)
- [Ways to Contribute](#ways-to-contribute)
- [Issue Labels](#issue-labels)
- [Reporting Bugs](#reporting-bugs)
- [Feature Requests](#feature-requests)
- [Pull Requests](#pull-requests)
- [Commit Messages](#commit-messages)
- [Code Style](#code-style)
- [Testing](#testing)
- [Documentation](#documentation)
- [FFmpeg Notes](#ffmpeg-notes)
- [License](#license)

---

## Code of Conduct

Please read and follow our [Code of Conduct](CODE_OF_CONDUCT.md).

---

## Prerequisites

Before contributing, make sure you have the following installed:

**Rust toolchain**

```sh
rustup toolchain install stable
rustup component add rustfmt clippy
```

Develop on the current stable toolchain. The MSRV (Minimum Supported Rust
Version) is **1.93.0**; CI verifies the workspace still compiles on it, so
avoid APIs newer than 1.93.0.

**FFmpeg development libraries** (version **7.x required**)

FFmpeg 6.x is not supported. In 7.x the scaling flags were converted from
`#define` macros to a proper `enum SwsFlags`, and `ff-sys` relies on the
bindgen-generated `SwsFlags_SWS_*` naming that only exists in 7.x.

| Platform | Command |
|---|---|
| Ubuntu / Debian | `sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswscale-dev libswresample-dev pkg-config` |
| macOS | `brew install ffmpeg pkg-config` |
| Windows | Install via [vcpkg](https://github.com/microsoft/vcpkg): `vcpkg install ffmpeg:x64-windows-static` |

Verify: `ffmpeg -version` (must show `7.x`)

---

## Ways to Contribute

- **Bug reports** — Something crashes or produces wrong output
- **Documentation** — Missing or incorrect rustdoc comments, examples, or guides
- **Examples** — Realistic usage examples in the `examples/` directory of each crate
- **FFmpeg API coverage** — New codec support, filter implementations, format handling
- **Platform testing** — Verifying builds and tests on macOS, Windows, or with hardware encoders (NVENC, VideoToolbox, VAAPI, AMF)
- **Performance** — Profiling and reducing unnecessary copies or allocations

Looking for a starting point? Check issues labeled [`good first issue`](https://github.com/itsakeyfut/avio/issues?q=is%3Aopen+label%3A%22good+first+issue%22) or [`help wanted`](https://github.com/itsakeyfut/avio/issues?q=is%3Aopen+label%3A%22help+wanted%22).

---

## Issue Labels

Issues and PRs are organised with a single-letter prefix system. Each label belongs to one family:

| Prefix | Meaning | Examples |
|---|---|---|
| `T-` | **Type** of work | `T-Bug`, `T-Feat`, `T-Doc`, `T-Perf`, `T-Refactor`, `T-Maintenance` |
| `A-` | **Area** / affected crate | `A-ff-filter`, `A-ff-decode`, `A-avio`, `A-ci` |
| `P-` | **Priority** | `P-Critical`, `P-High`, `P-Medium`, `P-Low` |
| `S-` | **Status** in the workflow | `S-Needs-Triage`, `S-Needs-Design`, `S-Ready-For-Implementation`, `S-In-Progress`, `S-Blocked` |
| `D-` | **Difficulty** / domain | `D-Trivial`, `D-Straightforward`, `D-Modest`, `D-Complex`, `D-Unsafe`, `D-FFmpeg` |

Finding work to pick up:

- New to the project? Start with [`good first issue`](https://github.com/itsakeyfut/avio/issues?q=is%3Aopen+label%3A%22good+first+issue%22), [`help wanted`](https://github.com/itsakeyfut/avio/issues?q=is%3Aopen+label%3A%22help+wanted%22), `D-Trivial`, or `D-Straightforward`.
- `S-Ready-For-Implementation` means the design is settled — safe to start an implementation PR. `S-Needs-Design` means it needs discussion first.
- `D-Unsafe` (touches `unsafe` / FFmpeg FFI) and `D-FFmpeg` (requires FFmpeg / codec / container knowledge) flag issues that need deeper domain familiarity.

---

## Reporting Bugs

Before filing a bug, search existing issues to avoid duplicates.

A good bug report includes:

1. **Description** — What happened and what you expected to happen
2. **Minimal reproduction** — The smallest code that reproduces the issue
3. **Versions**:
   - `rustc --version`
   - `ffmpeg -version`
   - Operating system and architecture
   - The `ff-*` crate version(s)
4. **Error output** — Full error message or panic backtrace (`RUST_BACKTRACE=1`)

---

## Feature Requests

Open an issue describing:

- The use case or problem you're trying to solve
- Which FFmpeg API or concept is involved
- Any API design ideas you have in mind

For changes that touch multiple crates or the public API surface, please discuss in an issue
before starting implementation.

---

## Pull Requests

1. **Open an issue first** for any non-trivial change (new features, API changes, or significant refactors).
2. Fork the repository and create a **topic branch** off `main`:
   ```sh
   git checkout -b ff-filter/add-scale-filter
   ```
3. Make your changes. Each commit should build and pass tests independently.
4. Run the full check suite (see [Code Style](#code-style) and [Testing](#testing)).
5. Push your branch and open a PR against `main`.
6. Add new commits to address review feedback — do not force-push during review.

**PRs without tests will not be merged.** If your change is difficult to test automatically, explain why in the PR description.

---

## Commit Messages

Use the `<crate>: <description>` prefix format so that changes are easy to identify:

```
ff-filter: add scale filter implementation

Wraps libavfilter's `scale` filter. Accepts width/height as either
pixel values or expressions (e.g., "iw/2").

Fixes: #42
```

Guidelines:

- Prefix with the crate name: `ff-filter:`, `ff-encode:`, `ff-probe:`, etc.
  For workspace-wide changes use `workspace:` or `docs:`.
- Use the imperative mood: "add", "fix", "remove" — not "added" or "fixes"
- First line ≤ 72 characters
- No trailing period on the first line
- Reference issues with `Fixes: #N` or `Refs: #N` in the footer

---

## Code Style

Before submitting, run:

```sh
# Format
cargo fmt --all

# Lint (must pass with no warnings)
cargo clippy --all --all-features -- -D warnings

# Check docs compile
cargo doc --all-features --no-deps
```

Key style rules enforced by the workspace `Cargo.toml`:

- `clippy::pedantic` is enabled — please fix all warnings rather than suppressing them without justification
- `clippy::unwrap_used` and `clippy::expect_used` are denied — use `?` or proper error handling
- No panics in library code
- All `unsafe` blocks must be contained in `*_inner.rs` modules with a `// SAFETY:` comment explaining the invariants

---

## Testing

Run the full test suite:

```sh
cargo test --all --all-features
```

If a test requires a real media file or a specific codec, gate it with `#[ignore]` and document
what is needed to run it manually.

For crates with feature flags, also test without default features:

```sh
cargo test -p ff-decode --no-default-features
```

---

## Documentation

Public API items must have rustdoc comments. Include at least:

- A one-line summary
- A short example if the usage is not obvious

```rust
/// Extracts metadata from a media file without decoding any frames.
///
/// # Example
///
/// ```no_run
/// # use ff_probe::Probe;
/// let info = Probe::open("video.mp4")?;
/// println!("duration: {:?}", info.duration());
/// # Ok::<_, ff_common::Error>(())
/// ```
pub fn open(path: impl AsRef<Path>) -> Result<Self> { ... }
```

---

## FFmpeg Notes

**Crate layering**

```
ff-sys          raw bindgen bindings (unsafe)
ff-common       shared types and error handling
ff-format       container demux / mux
ff-probe        metadata extraction
ff-decode       decoding
ff-encode       encoding
ff-filter       filter graphs
```

Each crate depends only on lower layers. No circular dependencies.

**unsafe isolation**

All raw FFmpeg pointer operations live in `*_inner.rs` files (e.g., `decoder_inner.rs`,
`filter_inner.rs`). Public-facing structs in `*_api.rs` or `lib.rs` must be fully safe.
Every `unsafe` block requires a `// SAFETY:` comment.

**Linking**

`ff-sys/build.rs` uses `pkg-config` on Linux/macOS and `vcpkg` on Windows.
If you add a new `libav*` library dependency, update `build.rs` accordingly.

---

## License

By contributing to this project, you agree that your contributions will be licensed under
the same terms as the project: **MIT OR Apache-2.0**.

See [LICENSE-MIT](../LICENSE-MIT) and [LICENSE-APACHE](../LICENSE-APACHE) for details.
