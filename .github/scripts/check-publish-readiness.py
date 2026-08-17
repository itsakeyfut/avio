#!/usr/bin/env python3
"""Publishing-readiness gate for the avio workspace.

Fails (non-zero exit) if any library crate is not ready to publish, or if a
known non-library member becomes publishable. Run in CI (see
`.github/workflows/ci.yml`) and locally with `python3 <this file>`.

Checks, over the root workspace members reported by `cargo metadata --no-deps`:

1. Every member expected to stay unpublished (`EXPECTED_NON_LIBRARY`) still has
   `publish = false`. This catches a non-library member accidentally becoming
   publishable.
2. Every publishable member (a library crate) declares the full publish
   metadata set: description, license (or license-file), repository, readme,
   keywords, categories. `cargo publish` only errors on a missing description or
   license, so keywords/categories are enforced here.
3. `cargo publish --dry-run --no-verify --workspace` packages every library
   crate. `--workspace` resolves the internal `ff-* = "0.16.0"` dependencies
   against the local members, so this passes before those versions exist on
   crates.io (a per-crate dry-run would fail there), and it skips the
   publish=false members automatically. `--no-verify` skips the compile step
   (already covered by the test/clippy/features jobs and matching release-plz's
   `publish_no_verify = true`), so this needs no FFmpeg.

The `tools` and `fuzz` crates live in separate workspaces, so they never appear
in this workspace's `--no-deps` metadata and are excluded automatically.
"""

import json
import subprocess
import sys

# Root-workspace members that are intentionally never published. Each must keep
# `publish = false`; removing it (making the member publishable) fails the gate.
EXPECTED_NON_LIBRARY = {"avio-examples"}

# Publish metadata every library crate must declare. `license` is satisfied by
# either `license` or `license-file`; the rest must be present and non-empty.
REQUIRED_FIELDS = ("description", "license", "repository", "readme", "keywords", "categories")


def load_members():
    """Return the root workspace's own packages (no dependencies)."""
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",  # cargo emits UTF-8 JSON regardless of the OS locale
    ).stdout
    return json.loads(out)["packages"]


def is_publishable(pkg):
    """A package is publishable unless its manifest sets `publish = false`.

    cargo metadata encodes `publish` as null (unrestricted) or a list of allowed
    registries; `publish = false` becomes an empty list.
    """
    return pkg.get("publish") is None


def missing_fields(pkg):
    """Return the required publish-metadata fields that `pkg` lacks."""
    missing = []
    for field in REQUIRED_FIELDS:
        if field == "license":
            if not (pkg.get("license") or pkg.get("license_file")):
                missing.append("license/license-file")
        else:
            value = pkg.get(field)
            if not value:  # None, "", or [] all count as missing
                missing.append(field)
    return missing


def main():
    packages = load_members()
    by_name = {p["name"]: p for p in packages}
    errors = []

    # 1. Known non-library members must stay unpublished.
    for name in sorted(EXPECTED_NON_LIBRARY):
        pkg = by_name.get(name)
        if pkg is None:
            errors.append(f"{name}: expected a non-library workspace member, but it was not found")
        elif is_publishable(pkg):
            errors.append(f"{name}: expected `publish = false`, but it is publishable")

    # 2 + 3. Every library crate needs complete metadata and a passing dry-run.
    libraries = sorted(
        p["name"] for p in packages if is_publishable(p) and p["name"] not in EXPECTED_NON_LIBRARY
    )
    for name in libraries:
        missing = missing_fields(by_name[name])
        if missing:
            errors.append(f"{name}: incomplete publish metadata, missing {', '.join(missing)}")

    print("cargo publish --dry-run --no-verify --workspace", flush=True)
    dry_run = subprocess.run(
        ["cargo", "publish", "--dry-run", "--no-verify", "--workspace"]
    )
    if dry_run.returncode != 0:
        errors.append("`cargo publish --dry-run --no-verify --workspace` failed")

    if errors:
        print("\nPublishing-readiness gate FAILED:", file=sys.stderr)
        for err in errors:
            print(f"  - {err}", file=sys.stderr)
        return 1

    print(f"\nPublishing-readiness gate passed: {len(libraries)} library crates ready to publish.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
