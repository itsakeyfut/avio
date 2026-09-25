//! Locating the repository, and the small file walks the tasks share.
//!
//! `cargo xtask` can be run from any subdirectory, so nothing here may assume
//! the current directory is the repository root. The root is resolved once and
//! every path a task touches is built from it.

use std::path::{Path, PathBuf};

/// The workspace root.
///
/// `git` is asked first because it is right even in a worktree or a submodule.
/// The compile-time manifest directory is the fallback, and it is exact whenever
/// the binary is the one cargo just built from this workspace.
pub fn root() -> PathBuf {
    if let Some(top) = crate::proc::stdout("git", &["rev-parse", "--show-toplevel"])
        && !top.is_empty()
    {
        return PathBuf::from(top);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Every `.rs` file under `dir`, recursively, in a stable order.
///
/// Sorted because `read_dir` is not: an unsorted walk would make the task list
/// reorder between runs for no reason, which is noise in a diff of two JSON
/// results.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_rust_files(dir, &mut found);
    found.sort();
    found
}

fn collect_rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// Reads a file, or returns an empty string if it cannot be read.
///
/// Every caller here is scanning for a marker, where "unreadable" and "does not
/// contain the marker" lead to the same action.
pub fn read(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}
