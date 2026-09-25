//! The repository's task runner: `cargo xtask <command>`.
//!
//! This crate is where the workspace's own automation lives. A task written here
//! is compiled, linted and tested by the same CI run as the library crates, and
//! it behaves the same on Windows as on Linux, which is what makes it safe to
//! point contributors and workflows at the same command.
//!
//! The commands are grouped by who runs them, because that is what decides how
//! each one should behave:
//!
//! * `dev`: run by a contributor at a terminal. Their output is for a person.
//! * `ci`: run by a GitHub workflow. Their exit code is the gate.
//! * `review`: answer a question a reviewer asks about a change, as JSON on
//!   stdout. Useful read by a person and read by a tool alike.
//!
//! There is no argument-parsing dependency. Each command reads the arguments it
//! needs and forwards the rest to cargo verbatim, which is what lets
//! `cargo xtask test -p avio --features preview` work without this crate having
//! to know cargo's flags.

mod ci;
mod dev;
mod json;
mod proc;
mod repo;
mod review;

use std::process::ExitCode;

const USAGE: &str = "\
cargo xtask <command> [args]

Development tasks (run these yourself):
  test -p <crate> [-p <crate>...] [--timeout <secs>] [cargo args]
        Run a crate's tests with the GPU targets serialised (#1718).
  dep-graph
        Print the workspace's internal dependency edges and check for cycles.

CI tasks (run by .github/workflows):
  publish-readiness
        Check every library crate is ready for crates.io.
  changelog <version> [path]
        Print one version's section of CHANGELOG.md.

Review aids (one JSON object on stdout):
  build [cargo args]              cargo build, summarised
  clippy [cargo args]             cargo clippy, summarised
  diff-scope [base [head]]        changed files, crates and path role flags
  unsafe-count [base [head]]      `unsafe` occurrences in changed .rs files
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];

    let code = match command.as_str() {
        "test" => dev::test::run(rest),
        "dep-graph" => dev::dep_graph::run(rest),
        "publish-readiness" => ci::publish_readiness::run(rest),
        "changelog" => ci::changelog::run(rest),
        "build" => review::build::run(rest),
        "clippy" => review::clippy::run(rest),
        "diff-scope" => review::diff_scope::run(rest),
        "unsafe-count" => review::unsafe_count::run(rest),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            0
        }
        other => {
            eprintln!("error: unknown command `{other}`");
            eprint!("{USAGE}");
            2
        }
    };
    ExitCode::from(code)
}
