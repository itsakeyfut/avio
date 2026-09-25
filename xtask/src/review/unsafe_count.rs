//! `cargo xtask unsafe-count`: how much `unsafe` a change carries.
//!
//! Emits `{total, changed_rs, per_file}` over the changed `.rs` files. The
//! workspace keeps `unsafe` isolated to the FFI modules, so the number is a
//! quick answer to "does this change need that scrutiny", and `per_file` says
//! where.
//!
//! ```text
//! cargo xtask unsafe-count                  uncommitted work against HEAD
//! cargo xtask unsafe-count <base>           <base>...HEAD
//! cargo xtask unsafe-count <base> <head>    an arbitrary range, read at <head>
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use crate::json;
use crate::proc;
use crate::repo;

pub fn run(args: &[String]) -> u8 {
    let root = repo::root();
    let (files, revision) = match args.first() {
        Some(base) => {
            let head = args.get(1).map_or("HEAD", String::as_str);
            let range = format!("{base}...{head}");
            (
                proc::stdout_lines("git", &["diff", "--name-only", &range, "--", "*.rs"]),
                Some(head.to_string()),
            )
        }
        None => {
            // Untracked files too: `git diff` hides them, which would
            // under-report the surface of a change that adds new FFI modules.
            let mut files =
                proc::stdout_lines("git", &["diff", "--name-only", "HEAD", "--", "*.rs"]);
            files.extend(proc::stdout_lines(
                "git",
                &["ls-files", "--others", "--exclude-standard", "--", "*.rs"],
            ));
            (files, None)
        }
    };

    let mut per_file: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    let mut changed = 0usize;
    for path in &files {
        if path.is_empty() || per_file.contains_key(path) {
            continue;
        }
        let source = match &revision {
            // Read the blob at that revision, so a historical range is measured
            // as it was rather than as the working tree is now.
            Some(rev) => match proc::stdout("git", &["show", &format!("{rev}:{path}")]) {
                Some(text) => text,
                None => continue,
            },
            None => {
                let absolute = root.join(path);
                if !Path::new(&absolute).is_file() {
                    continue;
                }
                repo::read(&absolute)
            }
        };
        changed += 1;
        let count = count_unsafe(&source);
        if count > 0 {
            per_file.insert(path.clone(), count);
            total += count;
        }
    }

    let per_file_json: Vec<String> = per_file
        .iter()
        .map(|(path, count)| format!("{}:{count}", json::quote(path)))
        .collect();
    println!(
        "{{\"total\":{total},\"changed_rs\":{changed},\"per_file\":{{{}}}}}",
        per_file_json.join(","),
    );
    0
}

/// Occurrences of `unsafe` as a whole word.
///
/// A word boundary is what keeps `unsafely` and `is_unsafe` out of the count.
/// This is a text scan, not a parse: a mention inside a comment or a string is
/// counted too, which is the conservative direction for a number whose job is to
/// decide how closely to look.
fn count_unsafe(source: &str) -> usize {
    const WORD: &str = "unsafe";
    let bytes = source.as_bytes();
    let mut count = 0;
    let mut start = 0;
    while let Some(offset) = source[start..].find(WORD) {
        let at = start + offset;
        let before_ok = at == 0 || !is_word_byte(bytes[at - 1]);
        let after = at + WORD.len();
        let after_ok = after >= bytes.len() || !is_word_byte(bytes[after]);
        if before_ok && after_ok {
            count += 1;
        }
        start = at + WORD.len();
    }
    count
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::count_unsafe;

    #[test]
    fn count_unsafe_should_count_whole_words_only() {
        assert_eq!(
            count_unsafe("unsafe { ptr } // unsafe\nfn unsafely() {}"),
            2
        );
        assert_eq!(count_unsafe("is_unsafe unsafely"), 0);
        assert_eq!(count_unsafe(""), 0);
    }
}
