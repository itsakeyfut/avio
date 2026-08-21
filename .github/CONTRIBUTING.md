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
- [Architecture Decision Records (ADR)](#architecture-decision-records-adr)
- [Pull Requests](#pull-requests)
- [Commit Messages](#commit-messages)
- [Code Style](#code-style)
- [Testing](#testing)
- [Minimum Supported Rust Version (MSRV)](#minimum-supported-rust-version-msrv)
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

**FFmpeg development libraries** (version **7.x or 8.x required**)

FFmpeg 6.x is not supported. Since 7.x the scaling flags are a proper
`enum SwsFlags`, and `ff-sys` relies on the bindgen-generated `SwsFlags_SWS_*`
naming that only exists in 7.x and later. FFmpeg 8.0 is also supported; its
version-specific tokens are gated behind an `ffmpeg8` cfg in `ff-sys`.

| Platform | Command |
|---|---|
| Ubuntu / Debian | `sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev libswscale-dev libswresample-dev pkg-config` |
| macOS | `brew install ffmpeg pkg-config` |
| Windows | Install via [vcpkg](https://github.com/microsoft/vcpkg): `vcpkg install ffmpeg:x64-windows` |

Verify: `ffmpeg -version` (must show `7.x` or `8.x`)

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

## Architecture Decision Records (ADR)

Significant design and architecture decisions are recorded as ADRs in
[`docs/adr/`](../docs/adr/) using the [MADR 4.0](https://adr.github.io/madr/) format. Please write one
when:

- two or more implementations are possible and you are choosing between them,
- the choice shapes the editing model or crosses crate boundaries, or
- you are reversing an earlier decision (add a new record and mark the old one `superseded by ADR-NNNN`).

To add one, copy [`docs/adr/adr-template.md`](../docs/adr/adr-template.md), number it consecutively,
fill in the **Confirmation** section (which test fails if the decision is violated), and add a row to
the index in [`docs/adr/README.md`](../docs/adr/README.md). Keep the rationale in the ADR; the specs
and per-crate design docs state the outcome and link to it. Naming, formatting, and single-call-site
changes do not need an ADR.

---

## Pull Requests

1. **Open an issue first** for any non-trivial change (new features, API changes, or significant refactors).
2. Fork the repository and create a **topic branch** off `main`, named
   `<type>/issue-<N>-<slug>` (`feat/`, `fix/`, `docs/`, `chore/`, `perf/`):
   ```sh
   git checkout -b feat/issue-42-add-scale-filter
   ```
3. Make your changes. Each commit should build and pass tests independently.
4. Run the full check suite (see [Code Style](#code-style) and [Testing](#testing)).
5. Push your branch and open a PR against `main`.
6. Add new commits to address review feedback — do not force-push during review.

**PRs without tests will not be merged.** If your change is difficult to test automatically, explain why in the PR description.

---

## Commit Messages

Use [Conventional Commits](https://www.conventionalcommits.org/): `<type>(<scope>): <description>`,
where `<scope>` is the affected crate.

```
feat(ff-filter): add scale filter implementation

Wraps libavfilter's `scale` filter. Accepts width/height as either
pixel values or expressions (e.g., "iw/2").
```

Guidelines:

- Common types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `chore`.
- Scope is the crate name: `feat(ff-encode): ...`, `fix(ff-probe): ...`. For workspace-wide
  changes use `chore` or `docs` with no scope.
- Use the imperative mood: "add", "fix", "remove", not "added" or "fixes".
- First line ≤ 72 characters, no trailing period.
- Add a blank line and a body explaining what and why for non-trivial changes.
- Link issues from the pull request description (`Closes #N` / `Fixes #N` / `Resolves #N`),
  not in the commit subject.

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

Tests must pass under a plain `cargo test` on any machine. If a test drives an FFmpeg filter
graph, probe the graph first and skip gracefully when filters are unavailable (return early
with a `println!` note) instead of calling a bare `.expect()`: CI's Linux FFmpeg is built
without filters. If a test needs a real media file or a specific codec, detect its absence at
runtime and skip the same way.

For crates with feature flags, also test without default features:

```sh
cargo test -p ff-decode --no-default-features
```

---

## Minimum Supported Rust Version (MSRV)

Develop and test on the **latest stable** toolchain. The MSRV is the oldest Rust release avio
still compiles on, currently **1.93.0**, verified by CI.

- The MSRV is a floor, not the toolchain you build with. It does not track new compiler
  releases automatically; the project can stay on a given MSRV across many Rust releases.
- A recent MSRV is intentional: while the project is young it lets us use current APIs without
  policing per-version availability. The trade-off is that downstream users must be on a
  reasonably recent toolchain.
- The MSRV is raised only deliberately (adopting a newer std/language API, or a dependency
  requiring it), only in a minor release, and always recorded in the CHANGELOG. It may be
  lowered later if broader adoption warrants; lowering it is not a breaking change.

In a contribution, please avoid APIs newer than the MSRV so the CI MSRV job stays green.

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
/// # use ff_probe::open;
/// let info = open("video.mp4")?;
/// if let Some(v) = info.primary_video() {
///     println!("{}x{}", v.width(), v.height());
/// }
/// # Ok::<_, ff_probe::ProbeError>(())
/// ```
pub fn open(path: impl AsRef<Path>) -> Result<MediaInfo, ProbeError> { ... }
```

---

## FFmpeg Notes

**Crate layering**

```
ff-sys          raw bindgen FFI + safe thin wrappers (unsafe)
ff-common       shared memory abstractions (no FFmpeg dep)
ff-format       shared pure-Rust type system (no FFmpeg dep)
ff-probe        read-only metadata extraction
ff-decode       decode pipelines
ff-analysis     media analysis (scene / silence / BPM / keyframe / scopes)
ff-encode       encode pipelines
ff-remux        stream-copy remux (trim, audio replace / extract / add)
ff-filter       libavfilter graph construction
ff-pipeline     high-level decode -> filter -> encode pipeline
ff-stream       HLS / DASH adaptive streaming
ff-preview      real-time preview and proxy workflow
ff-render       GPU compositing (wgpu)
avio            engine: editing model + derivation + history; re-exports the primitives
```

Dependency order (no cycles):
`ff-sys -> ff-common -> ff-format -> ff-probe / ff-decode / ff-encode / ff-remux -> ff-filter -> ff-pipeline -> ff-stream / ff-preview / ff-render -> avio`, plus `ff-decode -> ff-analysis`.
Each crate depends only on lower layers.

**unsafe isolation**

All raw FFmpeg pointer operations live in `*_inner.rs` files (e.g., `decoder_inner.rs`,
`filter_inner.rs`), exposed only as `pub(crate)`. The safe public API lives in each module's
`mod.rs` and the crate `lib.rs`, and must be fully safe. Every `unsafe` block requires a
`// SAFETY:` comment.

**Linking**

`ff-sys/build.rs` uses `pkg-config` on Linux/macOS and `vcpkg` on Windows.
If you add a new `libav*` library dependency, update `build.rs` accordingly.

---

## License

By contributing to this project, you agree that your contributions will be licensed under
the same terms as the project: **MIT OR Apache-2.0**.

See [LICENSE-MIT](../LICENSE-MIT) and [LICENSE-APACHE](../LICENSE-APACHE) for details.
