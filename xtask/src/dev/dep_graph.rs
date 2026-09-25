//! `cargo xtask dep-graph`: the workspace's internal dependency edges.
//!
//! The architecture rule this serves is that the crate graph is a DAG in one
//! fixed direction (`ff-sys` at the bottom, `avio` at the top). That rule is
//! stated in prose in several places; this command is where it is checked
//! against the manifests, so a review can cite an answer rather than a reading.
//!
//! Output: `{"crates":[...], "edges":[["from","to"],...], "has_cycle":bool, "cycle":[...]}`.
//! It exits non-zero when a cycle is found, so it can be used as a gate.

use std::collections::BTreeMap;

use crate::json;
use crate::repo;

pub fn run(_args: &[String]) -> u8 {
    let root = repo::root();
    let mut names: Vec<String> = std::fs::read_dir(root.join("crates"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();

    let mut edges: Vec<(String, String)> = Vec::new();
    for name in &names {
        let manifest = root.join("crates").join(name).join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let mut deps = manifest_deps(&repo::read(&manifest), &names);
        // A crate is not its own dependency, however its manifest is written.
        deps.retain(|dep| dep != name);
        deps.sort();
        deps.dedup();
        for dep in deps {
            edges.push((name.clone(), dep));
        }
    }

    let cycle = find_cycle(&names, &edges);

    let crates_json: Vec<String> = names.iter().map(|n| json::quote(n)).collect();
    let edges_json: Vec<String> = edges
        .iter()
        .map(|(from, to)| format!("[{},{}]", json::quote(from), json::quote(to)))
        .collect();
    let cycle_json: Vec<String> = cycle
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|n| json::quote(n))
        .collect();

    println!(
        "{{\"crates\":[{}],\"edges\":[{}],\"has_cycle\":{},\"cycle\":[{}]}}",
        crates_json.join(","),
        edges_json.join(","),
        cycle.is_some(),
        cycle_json.join(","),
    );
    u8::from(cycle.is_some())
}

/// The workspace crates a manifest names as dependencies.
///
/// A dependency appears as a table key at the start of a line, in either of the
/// two forms cargo accepts: `ff-common = { ... }` or `ff-common.workspace = true`.
/// Matching the key rather than parsing the whole TOML keeps this independent of
/// which dependency section it sits in, which is deliberate: a dev-dependency is
/// still an edge for the purpose of "does this crate know about that one".
///
/// `workspace` is the set of crates that exist, and it is what decides whether a
/// key is an edge at all. That check is what keeps `ff-syscall = "1"` from
/// counting, and it is also why a `name = "ff-filter"` line is harmless: the key
/// there is `name`, which is no crate.
fn manifest_deps(manifest: &str, workspace: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        let key: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .collect();
        if key.is_empty() || !workspace.contains(&key) {
            continue;
        }
        // Whatever follows the key has to end it, so that a longer key sharing a
        // crate's prefix cannot be read as that crate.
        let next = trimmed[key.len()..].chars().next();
        if matches!(next, None | Some(' ') | Some('.') | Some('=')) {
            found.push(key);
        }
    }
    found
}

/// The first cycle reachable from any crate, as the path that closes it.
///
/// Depth-first search with a three-state marking: unseen, on the current stack,
/// finished. Hitting a node that is on the stack is the cycle, and the returned
/// path repeats that node at the end so the loop is readable as written.
fn find_cycle(names: &[String], edges: &[(String, String)]) -> Option<Vec<String>> {
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in edges {
        adjacency.entry(from).or_default().push(to);
    }
    let mut state: BTreeMap<&str, u8> = BTreeMap::new();
    let mut path: Vec<&str> = Vec::new();
    for name in names {
        if state.get(name.as_str()).copied().unwrap_or(0) == 0
            && let Some(cycle) = visit(name, &adjacency, &mut state, &mut path)
        {
            return Some(cycle.into_iter().map(str::to_string).collect());
        }
    }
    None
}

fn visit<'a>(
    node: &'a str,
    adjacency: &BTreeMap<&'a str, Vec<&'a str>>,
    state: &mut BTreeMap<&'a str, u8>,
    path: &mut Vec<&'a str>,
) -> Option<Vec<&'a str>> {
    state.insert(node, 1);
    path.push(node);
    for next in adjacency.get(node).into_iter().flatten() {
        match state.get(next).copied().unwrap_or(0) {
            1 => {
                let start = path.iter().position(|n| n == next).unwrap_or(0);
                let mut cycle: Vec<&str> = path[start..].to_vec();
                cycle.push(next);
                return Some(cycle);
            }
            0 => {
                if let Some(cycle) = visit(next, adjacency, state, path) {
                    return Some(cycle);
                }
            }
            _ => {}
        }
    }
    state.insert(node, 2);
    path.pop();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_deps_should_read_both_dependency_spellings() {
        let manifest = "[package]
name = \"ff-filter\"

[dependencies]
ff-common = { workspace = true }
ff-format.workspace = true
log = \"0.4\"
";
        let workspace = vec_of(&["ff-common", "ff-filter", "ff-format"]);
        assert_eq!(
            manifest_deps(manifest, &workspace),
            vec_of(&["ff-common", "ff-format"])
        );
    }

    #[test]
    fn manifest_deps_should_not_match_a_key_that_merely_starts_with_a_crate_name() {
        let workspace = vec_of(&["ff-sys"]);
        assert!(
            manifest_deps(
                "ff-syscall = \"1\"
",
                &workspace
            )
            .is_empty()
        );
    }

    #[test]
    fn find_cycle_should_report_the_loop_it_closed() {
        let names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let edges = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
            ("c".to_string(), "a".to_string()),
        ];
        assert_eq!(
            find_cycle(&names, &edges),
            Some(vec_of(&["a", "b", "c", "a"]))
        );
    }

    #[test]
    fn find_cycle_should_accept_a_dag() {
        let names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let edges = vec![
            ("a".to_string(), "b".to_string()),
            ("a".to_string(), "c".to_string()),
            ("b".to_string(), "c".to_string()),
        ];
        assert_eq!(find_cycle(&names, &edges), None);
    }

    fn vec_of(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }
}
