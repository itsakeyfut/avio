//! Check the ADRs against the tree, mechanically.
//!
//! `docs/adr/README.md` makes four promises that rot silently: every record has a
//! row and every row a record, a record's status matches its row and the by-status
//! line, and **Confirmation** says what guards the decision *now*. The last one is
//! the one that has already gone wrong here: #1819's review found ADR-0017's
//! Confirmation claiming more than the measurement supported, and a renamed test
//! leaves a record pointing at a name that is gone.
//!
//! What this cannot check is whether the mutation a record names still fails the
//! test it names. That part is a person's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::repo;

pub fn run(args: &[String]) -> u8 {
    if !args.is_empty() {
        eprintln!("error: adr-check takes no arguments");
        return 2;
    }

    let root = repo::root();
    let adr = root.join("docs").join("adr");
    let mut problems: Vec<String> = Vec::new();

    let mut records: Vec<String> = std::fs::read_dir(&adr)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.len() > 5 && name[..4].chars().all(|c| c.is_ascii_digit()))
        .filter(|name| name.ends_with(".md"))
        .collect();
    records.sort();

    let index_path = adr.join("README.md");
    let index = repo::read(&index_path);
    if index.is_empty() {
        eprintln!("error: cannot read {}", index_path.display());
        return 2;
    }

    // 1. Every record has a row, and every linked row has a record.
    for name in &records {
        if !index.contains(&format!("({name})")) && !index.contains(&format!("(./{name})")) {
            problems.push(format!("{name} has no row in the index"));
        }
    }
    for target in index_links(&index) {
        if !adr.join(&target).is_file() {
            problems.push(format!("the index links {target}, which does not exist"));
        }
    }

    // 2. The front-matter status matches the row and the by-status line.
    let listed = by_status_line(&index);
    let mut statuses: BTreeMap<String, String> = BTreeMap::new();
    for name in &records {
        let text = repo::read(&adr.join(name));
        let Some(status) = front_matter_status(&text) else {
            problems.push(format!("{name} has no status in its front matter"));
            continue;
        };
        let id = name[..4].to_string();
        statuses.insert(id.clone(), status.clone());

        let row = index.lines().find(|line| {
            line.contains(&format!("({name})")) || line.contains(&format!("(./{name})"))
        });
        if row.is_some_and(|row| !row.contains(&status)) {
            problems.push(format!(
                "{name} is {status:?} but its index row does not say so"
            ));
        }
        match listed.get(&id) {
            None => problems.push(format!(
                "{id} is {status} and is missing from the by-status line"
            )),
            Some(where_listed) if where_listed != &status => problems.push(format!(
                "{id} is {status} and the by-status line lists it under {where_listed}"
            )),
            Some(_) => {}
        }
    }

    // 3. Every test a Confirmation names still exists somewhere in the source.
    let source = source_text(&root);
    for name in &records {
        let text = repo::read(&adr.join(name));
        let Some(body) = confirmation(&text) else {
            problems.push(format!("{name} has no Confirmation section"));
            continue;
        };
        for ident in identifiers(&body) {
            if !source.contains(&ident) {
                problems.push(format!(
                    "{name} names `{ident}` in Confirmation, and nothing in the tree says it"
                ));
            }
        }
    }

    report("records", records.len(), &problems)
}

/// The record filenames the index links, as `[0001](./0001-….md)`.
fn index_links(index: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in index.lines() {
        let mut rest = line;
        while let Some(open) = rest.find("](") {
            let after = &rest[open + 2..];
            let Some(close) = after.find(')') else { break };
            let target = after[..close].trim_start_matches("./");
            if target.len() > 5 && target[..4].chars().all(|c| c.is_ascii_digit()) {
                found.push(target.to_string());
            }
            rest = &after[close..];
        }
    }
    found
}

/// The `**By status** - accepted: 0001, 0002 · proposed: none` line, as id -> status.
///
/// Read as clauses rather than as one line: asking only whether a number appears
/// anywhere answers yes for a record listed under the wrong status, which is the
/// drift this is here to catch.
fn by_status_line(index: &str) -> BTreeMap<String, String> {
    let mut listed = BTreeMap::new();
    let Some(line) = index.lines().find(|l| l.starts_with("**By status**")) else {
        return listed;
    };
    let body = line
        .trim_start_matches("**By status**")
        .trim()
        .trim_start_matches(['-', ':'])
        .trim();
    for clause in body.split('·') {
        let Some((name, ids)) = clause.split_once(':') else {
            continue;
        };
        let status = name.trim().to_string();
        for token in ids.split(',') {
            let id: String = token
                .trim()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if id.len() == 4 {
                listed.insert(id, status.clone());
            }
        }
    }
    listed
}

fn front_matter_status(text: &str) -> Option<String> {
    text.lines()
        .take_while(|line| !line.starts_with("# "))
        .find_map(|line| line.strip_prefix("status:"))
        .map(|value| {
            value
                .trim()
                .trim_matches('"')
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .filter(|status| !status.is_empty())
}

/// The Confirmation section's body, up to the next `##`-level heading.
fn confirmation(text: &str) -> Option<String> {
    let start = text.find("### Confirmation")?;
    let rest = &text[start + "### Confirmation".len()..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Backticked identifiers long enough to be a test or item name.
///
/// Anything with a path separator, a `::`, a `.` or a space is a path or prose
/// rather than a name the tree has to define, and a short one is too generic to
/// search for without matching something unrelated.
fn identifiers(body: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = body;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        let token = &after[..close];
        rest = &after[close + 1..];
        let plain = token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit());
        if plain && token.len() >= 13 && token.contains('_') {
            found.insert(token.to_string());
        }
    }
    found
}

/// Every Rust source file in the workspace, concatenated once.
fn source_text(root: &Path) -> String {
    let mut text = String::new();
    for dir in ["crates", "xtask", "examples", "tools"] {
        for path in repo::rust_files(&root.join(dir)) {
            text.push_str(&repo::read(&path));
            text.push('\n');
        }
    }
    text
}

pub(crate) fn report(kind: &str, count: usize, problems: &[String]) -> u8 {
    if problems.is_empty() {
        println!("  (nothing)");
    } else {
        for problem in problems {
            println!("  {problem}");
        }
    }
    println!("\n{kind}: {count}  problems: {}", problems.len());
    u8::from(!problems.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_status_line_should_read_each_clause_separately() {
        let index = "**By status** - accepted: 0001, 0002 · proposed: 0003 · superseded: none";
        let listed = by_status_line(index);
        assert_eq!(listed.get("0001").map(String::as_str), Some("accepted"));
        assert_eq!(listed.get("0003").map(String::as_str), Some("proposed"));
        assert_eq!(listed.get("0004"), None);
    }

    #[test]
    fn by_status_line_should_not_keep_the_separator_in_the_first_clause() {
        // `**By status** - accepted: …`: stripping the separator before trimming the
        // space left it on the name, so every record read as listed under "- accepted".
        let listed = by_status_line("**By status** - accepted: 0001 · proposed: none");
        assert_eq!(listed.get("0001").map(String::as_str), Some("accepted"));
    }

    #[test]
    fn front_matter_status_should_drop_the_quotes() {
        let text = "---\nstatus: \"accepted\"\ndate: 2026-09-03\n---\n\n# Title\n";
        assert_eq!(front_matter_status(text).as_deref(), Some("accepted"));
    }

    #[test]
    fn front_matter_status_should_ignore_a_later_body_line() {
        let text = "---\ndate: 2026-09-03\n---\n\n# Title\n\nstatus: accepted in prose\n";
        assert_eq!(front_matter_status(text), None);
    }

    #[test]
    fn confirmation_should_stop_at_the_next_section() {
        let text =
            "### Confirmation\n\nthe test `a_test_name_here` guards it\n\n## More\n\nnot this\n";
        let body = confirmation(text).expect("a Confirmation section");
        assert!(body.contains("a_test_name_here"));
        assert!(!body.contains("not this"));
    }

    #[test]
    fn identifiers_should_take_test_names_and_leave_paths_and_prose() {
        let body = "`a_retimed_clip_should_keep_it` and `crates/avio/src/edit.rs` and `Timeline::render` and `fps` and `some words`";
        let found = identifiers(body);
        assert!(found.contains("a_retimed_clip_should_keep_it"));
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn index_links_should_find_record_targets_only() {
        let index = "| [0009](./0009-transition.md) | x | accepted | y |\nsee [MADR](https://adr.github.io/madr/)";
        assert_eq!(index_links(index), vec!["0009-transition.md".to_string()]);
    }
}
