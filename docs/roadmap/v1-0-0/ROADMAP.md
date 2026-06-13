# v1.0.0 — Stable API

**Goal**: Semver-stable public API suitable for production use across all crates.

**Prerequisite**: v0.17.0 complete, with demonstrated real-world adoption (at least one publicly released application built on this library).

---

## Tasks

### API Stability

- [ ] Semver stability guarantee documented for all crates
- [ ] MSRV (Minimum Supported Rust Version) policy documented in `README.md`
- [ ] Breaking change process defined in `CONTRIBUTING.md`

### Security & Maintenance

- [ ] `SECURITY.md` — vulnerability reporting policy
- [ ] `CHANGELOG.md` — complete history from v0.1.0 through v1.0.0

### Quality Gates

- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] `cargo test --workspace` passes on Linux, macOS, Windows
- [ ] No `unwrap()` or `expect()` in library code (only in tests and examples)

---

## Definition of Done

- All checkboxes above checked
- Crates published to crates.io with `version = "1.0.0"`
- docs.rs renders all public items with documentation
