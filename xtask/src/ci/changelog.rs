//! `cargo xtask changelog <version> [path]`: one version's section of the
//! changelog, for the GitHub Release body.
//!
//! Prints the body between the `## [<version>]` header and the next `## [`
//! header, with the trailing `---` separator and surrounding blank lines
//! stripped. It exits non-zero if the section is missing or empty, so a release
//! without a curated changelog entry fails loudly instead of cutting an empty
//! GitHub Release.

use crate::repo;

pub fn run(args: &[String]) -> u8 {
    let Some(version) = args.first() else {
        eprintln!("usage: cargo xtask changelog <version> [changelog_path]");
        return 2;
    };
    let path = args.get(1).map_or("CHANGELOG.md", String::as_str);
    let absolute = repo::root().join(path);
    let text = std::fs::read_to_string(&absolute);
    let text = match text {
        Ok(text) => text,
        Err(err) => {
            eprintln!("error: cannot read {}: {err}", absolute.display());
            return 1;
        }
    };

    let Some(body) = extract(&text, version) else {
        eprintln!("error: no `## [{version}]` section in {path}");
        return 1;
    };
    if body.trim().is_empty() {
        eprintln!("error: `## [{version}]` section in {path} is empty");
        return 1;
    }
    println!("{body}");
    0
}

/// The body of the `## [<version>]` section, or `None` if there is no such section.
fn extract(text: &str, version: &str) -> Option<String> {
    let header = format!("## [{version}]");
    let mut lines = text.lines();
    // The header may carry a date after the bracket, so match the prefix and not
    // the whole line.
    lines.by_ref().find(|line| starts_section(line, &header))?;

    let mut body: Vec<&str> = Vec::new();
    for line in lines {
        if is_any_section(line) {
            break;
        }
        body.push(line);
    }

    // Keep a Changelog puts a `---` rule between versions; it belongs to the
    // separation, not to this version's notes.
    while matches!(body.last().map(|line| line.trim()), Some("") | Some("---")) {
        body.pop();
    }
    while matches!(body.first().map(|line| line.trim()), Some("")) {
        body.remove(0);
    }
    Some(body.join("\n"))
}

fn starts_section(line: &str, header: &str) -> bool {
    normalise_heading(line).is_some_and(|heading| heading.starts_with(header))
}

fn is_any_section(line: &str) -> bool {
    normalise_heading(line).is_some_and(|heading| heading.starts_with("## ["))
}

/// Collapses the whitespace cargo-generated changelogs vary on: `##[1.0]`,
/// `##  [1.0]` and `## [1.0]` all name the same section.
fn normalise_heading(line: &str) -> Option<String> {
    let rest = line.strip_prefix("##")?;
    if rest.starts_with('#') {
        return None;
    }
    Some(format!("## {}", rest.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::extract;

    const CHANGELOG: &str = "\
# Changelog

## [0.18.0] - 2026-09-08

### Added
- GPU compositing.

---

## [0.17.0] - 2026-08-28

### Fixed
- A thing.

## [0.16.0]

---
";

    #[test]
    fn extract_should_return_one_sections_body() {
        assert_eq!(
            extract(CHANGELOG, "0.18.0").as_deref(),
            Some("### Added\n- GPU compositing.")
        );
    }

    #[test]
    fn extract_should_stop_at_the_next_version_header() {
        assert_eq!(
            extract(CHANGELOG, "0.17.0").as_deref(),
            Some("### Fixed\n- A thing.")
        );
    }

    #[test]
    fn extract_should_report_an_empty_section_as_empty() {
        assert_eq!(extract(CHANGELOG, "0.16.0").as_deref(), Some(""));
    }

    #[test]
    fn extract_should_return_none_for_a_missing_version() {
        assert!(extract(CHANGELOG, "9.9.9").is_none());
    }
}
