//! `cargo xtask diff-scope`: what a change touches.
//!
//! Emits `{mode, file_count, files, crates, flags}`: the changed files, how many
//! of them fall in each crate, and a few flags for the paths that carry a
//! standing rule of their own.
//!
//! ```text
//! cargo xtask diff-scope                  uncommitted work against HEAD
//! cargo xtask diff-scope <base>           <base>...HEAD, the shape of a branch
//! cargo xtask diff-scope <base> <head>    an arbitrary range
//! ```

use std::collections::BTreeMap;

use crate::json;
use crate::proc;

pub fn run(args: &[String]) -> u8 {
    let (files, mode) = match args.first() {
        Some(base) => {
            let head = args.get(1).map_or("HEAD", String::as_str);
            let range = format!("{base}...{head}");
            (
                proc::stdout_lines("git", &["diff", "--name-only", &range]),
                format!("range:{range}"),
            )
        }
        None => {
            // `git diff` reports only tracked files, so a change whose substance
            // lives in NEW files would report an almost empty scope. Untracked,
            // non-ignored files are folded in so reviewing uncommitted work sees
            // everything.
            let mut files = proc::stdout_lines("git", &["diff", "--name-only", "HEAD"]);
            files.extend(proc::stdout_lines(
                "git",
                &["ls-files", "--others", "--exclude-standard"],
            ));
            (files, "worktree".to_string())
        }
    };

    let mut files = files;
    files.retain(|path| !path.is_empty());
    let mut seen = Vec::new();
    files.retain(|path| {
        let fresh = !seen.contains(path);
        if fresh {
            seen.push(path.clone());
        }
        fresh
    });

    let mut crates: BTreeMap<String, usize> = BTreeMap::new();
    let mut flags: Vec<&str> = Vec::new();
    for path in &files {
        let owner = if let Some(rest) = path.strip_prefix("crates/") {
            rest.split('/').next().unwrap_or("(root)").to_string()
        } else if path.starts_with("examples/") {
            "examples".to_string()
        } else {
            "(root)".to_string()
        };
        *crates.entry(owner).or_default() += 1;

        if !path.ends_with(".rs") {
            continue;
        }
        if path.starts_with("crates/ff-render/") {
            push_once(&mut flags, "gpu");
        }
        if path.starts_with("crates/ff-stream/") {
            push_once(&mut flags, "stream");
        }
        // `_inner.rs` is the FFmpeg-FFI isolation convention. `ff-render` reuses
        // the name for its GPU graph internals (`graph_inner.rs`), which is not
        // FFI, so it is excluded: a pure GPU change must not read as an FFI one.
        if (path.starts_with("crates/ff-sys/") || path.ends_with("_inner.rs"))
            && !path.starts_with("crates/ff-render/")
        {
            push_once(&mut flags, "ffi_paths");
        }
    }
    flags.sort_unstable();

    let files_json: Vec<String> = files.iter().map(|p| json::quote(p)).collect();
    let crates_json: Vec<String> = crates
        .iter()
        .map(|(name, count)| format!("{}:{count}", json::quote(name)))
        .collect();
    let flags_json: Vec<String> = flags
        .iter()
        .map(|flag| format!("{}:true", json::quote(flag)))
        .collect();

    println!(
        "{{\"mode\":{},\"file_count\":{},\"files\":[{}],\"crates\":{{{}}},\"flags\":{{{}}}}}",
        json::quote(&mode),
        files.len(),
        files_json.join(","),
        crates_json.join(","),
        flags_json.join(","),
    );
    0
}

fn push_once<'a>(flags: &mut Vec<&'a str>, flag: &'a str) {
    if !flags.contains(&flag) {
        flags.push(flag);
    }
}
