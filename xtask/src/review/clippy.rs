//! `cargo xtask clippy`: a clippy run, summarised as JSON.
//!
//! Emits `{ok, warning_count, error_count, log_tail}`. Any arguments are passed
//! through to cargo to scope the run (`cargo xtask clippy -p ff-render`); with
//! none, it covers the whole workspace, because an unscoped lint check is the
//! one the release gate cares about.

use crate::json;
use crate::proc;

pub fn run(args: &[String]) -> u8 {
    let mut cargo_args = vec!["clippy".to_string()];
    if args.is_empty() {
        cargo_args.push("--workspace".to_string());
    }
    cargo_args.push("--all-targets".to_string());
    cargo_args.push("--message-format=short".to_string());
    cargo_args.extend(args.iter().cloned());

    let captured = proc::capture("cargo", &cargo_args, None);

    // Count the per-diagnostic lines (`path:line:col: warning: ...`), not the
    // trailing `warning: \`crate\` generated N warnings` summary, which would
    // double-count every run that produced any.
    let warnings = captured
        .output
        .lines()
        .filter(|line| line.contains(": warning:"))
        .count();
    let errors = captured
        .output
        .lines()
        .filter(|line| line.contains(": error"))
        .count();

    println!(
        "{{\"ok\":{},\"warning_count\":{warnings},\"error_count\":{errors},\"log_tail\":{}}}",
        captured.success(),
        json::quote(&json::log_tail(&captured.output, 60)),
    );
    u8::from(!captured.success())
}
