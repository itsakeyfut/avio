//! Run the gate on every commit of a branch, not only on its tip.
//!
//! A branch whose tip is green can still contain a commit nobody can build, and
//! splitting a change into readable commits is exactly where that happens. The
//! cost is paid later, by whoever runs `git bisect` across the range and finds
//! that half the candidates fail for a reason unrelated to what they are hunting.
//!
//! **The test step is off by default.** The gate's own test run takes minutes per
//! crate here, because the GPU targets have to go one at a time (#1718), and
//! multiplying that by every commit of a branch buys an answer the tip's run
//! already gives. What stays on is the whole compile-and-lint gate, which is what
//! decides whether a commit can be built at all. `--tests` runs the full gate on
//! each commit when that is what is wanted.
//!
//! The commits are checked out into a throwaway worktree, so the working tree is
//! never touched and uncommitted work is safe. One target directory is shared
//! across the range, which is what keeps the second and later commits cheap.

use std::path::PathBuf;

use crate::dev::adr_check::report;
use crate::proc;

pub fn run(args: &[String]) -> u8 {
    let mut base: Option<String> = None;
    let mut crates: Vec<String> = Vec::new();
    let mut timeout: Option<String> = None;
    let mut tests = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" => {
                let Some(name) = args.get(i + 1) else {
                    eprintln!("error: {} needs a crate name", args[i]);
                    return 2;
                };
                crates.push(name.clone());
                i += 2;
            }
            "--timeout" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("error: --timeout needs a number of seconds");
                    return 2;
                };
                timeout = Some(value.clone());
                i += 2;
            }
            "--tests" => {
                tests = true;
                i += 1;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown argument `{other}`");
                return 2;
            }
            other => {
                if base.is_some() {
                    eprintln!("error: only one base may be given (got `{other}` as well)");
                    return 2;
                }
                base = Some(other.to_string());
                i += 1;
            }
        }
    }
    let base = base.unwrap_or_else(|| "main".to_string());

    let range = format!("{base}..HEAD");
    let commits = proc::stdout_lines("git", &["rev-list", "--reverse", "--no-merges", &range]);
    if commits.is_empty() {
        println!("no commits on top of {base}");
        return 0;
    }

    if crates.is_empty() {
        crates = changed_crates(&base);
    }
    if crates.is_empty() {
        eprintln!(
            "error: {range} touches no crate, so there is nothing to scope the gate to. \
             Pass -p <crate> if this range should still be checked."
        );
        return 2;
    }

    println!(
        "commits: {range} ({} commits, crates: {}{})",
        commits.len(),
        crates.join(" "),
        if tests { "" } else { ", tests skipped" }
    );

    let Some(work) = Worktree::create() else {
        eprintln!("error: could not create a worktree to check the commits in");
        return 2;
    };

    let mut problems: Vec<String> = Vec::new();
    for commit in &commits {
        let short: String = commit.chars().take(7).collect();
        let subject: String = proc::stdout("git", &["log", "--format=%s", "-1", commit])
            .unwrap_or_default()
            .chars()
            .take(56)
            .collect();

        let checkout = proc::capture(
            "git",
            &owned(&["-C", &work.path.to_string_lossy(), "checkout", "-q", commit]),
            None,
        );
        if !checkout.success() {
            println!("  FAIL  {short}  {subject}  (could not check it out)");
            problems.push(format!("{short} could not be checked out"));
            continue;
        }

        let gate = self_gate(&work, &crates, timeout.as_deref(), tests);
        if gate.success() {
            println!("  ok    {short}  {subject}");
        } else {
            println!("  FAIL  {short}  {subject}");
            for line in failing_steps(&gate.output) {
                println!("        {line}");
            }
            problems.push(format!("{short} {subject}"));
        }
    }

    report("commits", commits.len(), &problems)
}

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

/// The workspace crates a range touches, as `-p` names.
///
/// `crates/<name>/` is the crate of that name and `xtask/` is its own member; a
/// change confined to `docs/` or to the root manifest maps to nothing, which the
/// caller reports rather than guessing a scope for. `examples/` is the
/// `avio-examples` member, whose directory name and package name differ.
fn changed_crates(base: &str) -> Vec<String> {
    let range = format!("{base}...HEAD");
    crates_for_paths(&proc::stdout_lines("git", &["diff", "--name-only", &range]))
}

/// The mapping half of [`changed_crates`], separated so it can be tested without
/// a repository in a particular state.
fn crates_for_paths(paths: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for path in paths {
        let owner = if let Some(rest) = path.strip_prefix("crates/") {
            rest.split('/')
                .next()
                .filter(|name| !name.is_empty() && rest.contains('/'))
                .map(str::to_string)
        } else if path.starts_with("examples/") {
            Some("avio-examples".to_string())
        } else if path.starts_with("xtask/") {
            Some("xtask".to_string())
        } else {
            None
        };
        if let Some(name) = owner
            && !found.contains(&name)
        {
            found.push(name);
        }
    }
    found.sort();
    found
}

/// Runs this same binary's `gate` inside the worktree.
///
/// Re-executing rather than calling `gate::run` directly is what lets the gate run
/// against a different directory: its steps shell out to cargo, and cargo reads
/// the workspace from the directory it starts in. It also means every commit is
/// judged by *this* gate, which matters because a commit from before the gate
/// existed carries no copy of it.
fn self_gate(
    work: &Worktree,
    crates: &[String],
    timeout: Option<&str>,
    tests: bool,
) -> proc::Captured {
    let Ok(exe) = std::env::current_exe() else {
        return proc::Captured {
            output: "error: cannot locate this executable to re-run it".to_string(),
            code: None,
            timed_out: false,
        };
    };

    let mut args = vec!["gate".to_string()];
    for name in crates {
        args.push("-p".to_string());
        args.push(name.clone());
    }
    if !tests {
        args.push("--no-tests".to_string());
    }
    if let Some(secs) = timeout {
        args.push("--timeout".to_string());
        args.push(secs.to_string());
    }

    // One target directory for the whole range: without it every commit is a cold
    // build and the task is too slow to be run.
    let target = work.target.to_string_lossy().into_owned();
    proc::capture_in(
        &exe.to_string_lossy(),
        &args,
        None,
        &[("CARGO_TARGET_DIR", target.as_str())],
        Some(&work.path),
    )
}

/// The gate's own `FAIL` lines plus the first few error lines under them.
///
/// The gate already prints a tail per failing step; reprinting all of it once per
/// commit would bury the one line that says which commit broke.
fn failing_steps(output: &str) -> Vec<String> {
    let mut lines: Vec<String> = output
        .lines()
        .map(str::trim_end)
        .filter(|line| line.contains("FAIL") || line.trim_start().starts_with("error"))
        .map(str::to_string)
        .collect();
    lines.truncate(8);
    lines
}

/// A detached worktree and a target directory, both removed on drop.
struct Worktree {
    path: PathBuf,
    target: PathBuf,
    root: PathBuf,
}

impl Worktree {
    fn create() -> Option<Self> {
        let root = crate::repo::root();
        let scratch = std::env::temp_dir().join(format!("avio-commits-{}", std::process::id()));
        let path = scratch.join("wt");
        let target = scratch.join("target");
        std::fs::create_dir_all(&target).ok()?;

        let added = proc::capture(
            "git",
            &owned(&[
                "worktree",
                "add",
                "-q",
                "--detach",
                &path.to_string_lossy(),
                "HEAD",
            ]),
            None,
        );
        if !added.success() {
            eprintln!("{}", added.output.trim());
            return None;
        }
        Some(Self { path, target, root })
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        // In Drop so that a failing commit or a panic between commits still leaves
        // the repository without a registered worktree pointing at a temporary
        // directory that is about to be deleted.
        let _ = proc::capture(
            "git",
            &owned(&[
                "-C",
                &self.root.to_string_lossy(),
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ]),
            None,
        );
        if let Some(scratch) = self.path.parent() {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failing_steps_should_keep_the_gate_verdict_lines() {
        let output =
            "gate:\n  ok    fmt\n  FAIL  clippy\n\n----- clippy -----\n      error: unused\n";
        let kept = failing_steps(output);
        assert!(kept.iter().any(|line| line.contains("FAIL  clippy")));
        assert!(kept.iter().any(|line| line.contains("error: unused")));
        assert!(!kept.iter().any(|line| line.contains("ok    fmt")));
    }

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|p| (*p).to_string()).collect()
    }

    #[test]
    fn crates_for_paths_should_name_the_member_each_path_belongs_to() {
        let found = crates_for_paths(&paths(&[
            "crates/avio/src/timeline.rs",
            "crates/avio/tests/x.rs",
            "crates/ff-filter/src/lib.rs",
            "examples/src/lib.rs",
            "xtask/src/main.rs",
        ]));
        assert_eq!(
            found,
            paths(&["avio", "avio-examples", "ff-filter", "xtask"])
        );
    }

    #[test]
    fn crates_for_paths_should_ignore_what_is_not_a_member() {
        // A range confined to these has nothing to scope a gate to, and the caller
        // says so rather than inventing a scope.
        let found = crates_for_paths(&paths(&[
            "Cargo.toml",
            "docs/adr/0015-x.md",
            "README.md",
            ".github/workflows/ci.yml",
            "crates/README.md",
        ]));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn failing_steps_should_cap_what_it_prints() {
        let output = (0..40).map(|_| "  error: x\n").collect::<String>();
        assert_eq!(failing_steps(&output).len(), 8);
    }
}
