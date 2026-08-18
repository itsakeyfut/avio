#!/usr/bin/env python3
"""Extract one version's section body from CHANGELOG.md for GitHub Release notes.

Usage: extract-changelog.py <version> [changelog_path]

Prints the body between the `## [<version>]` header and the next `## [` header
(the trailing `---` separator and surrounding blank lines are stripped). Exits
non-zero if the section is missing or empty, so a release without a curated
changelog entry fails loudly instead of cutting an empty GitHub Release.
"""

import re
import sys


def extract(text: str, version: str) -> str | None:
    """Return the body of the `## [<version>]` section, or None if absent."""
    pattern = re.compile(
        r"^##\s*\[" + re.escape(version) + r"\][^\n]*\n(.*?)(?=^##\s*\[|\Z)",
        re.DOTALL | re.MULTILINE,
    )
    match = pattern.search(text)
    if match is None:
        return None
    body = match.group(1)
    # Drop a trailing `---` separator (Keep a Changelog uses it between versions).
    body = re.sub(r"\n+---\s*\n*\Z", "\n", body)
    return body.strip("\n")


def main() -> int:
    if len(sys.argv) < 2:
        sys.stderr.write("usage: extract-changelog.py <version> [changelog_path]\n")
        return 2
    version = sys.argv[1]
    path = sys.argv[2] if len(sys.argv) > 2 else "CHANGELOG.md"

    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except OSError as err:
        sys.stderr.write(f"error: cannot read {path}: {err}\n")
        return 1

    body = extract(text, version)
    if body is None:
        sys.stderr.write(f"error: no `## [{version}]` section in {path}\n")
        return 1
    if not body.strip():
        sys.stderr.write(f"error: `## [{version}]` section in {path} is empty\n")
        return 1

    sys.stdout.write(body + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
